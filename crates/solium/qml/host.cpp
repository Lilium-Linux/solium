/*
 * Qt Quick, hosted in the compositor process.
 *
 * Why in-process: the other model was tried and measured. A shell painting
 * decorations out-of-process and shipping frames over a protocol reached ~15
 * fps at 39% CPU after optimisation, and the process boundary was the ceiling,
 * not the encoding. So the scene graph runs here, in the compositor, with no
 * IPC — the model KWin uses for Aurorae.
 *
 * Why the *software* scene graph, though, rather than rendering straight into
 * one of the compositor's GL textures:
 *
 *   Qt can only be handed an existing GL context through
 *   QNativeInterface::QEGLContext::fromNative, and that call is implemented by
 *   the QPA platform plugin, not by Qt Gui. Measured on this machine, Qt 6.11:
 *   `offscreen` and `eglfs` both return null, so there is no plugin available
 *   that will adopt a foreign EGL context. Without adoption, Qt renders on a
 *   context of its own and the texture it produces is not one the compositor
 *   can sample — the two contexts share nothing.
 *
 *   The software rasteriser has no such requirement: QML draws into a QImage,
 *   and the compositor uploads it. For chrome — a bar, a titlebar — that is a
 *   few hundred kilobytes per change, and it is only redone when something
 *   actually changes. See docs/spikes for the numbers and for the two ways back
 *   to the GPU path (a plugin that adopts contexts, or a dmabuf-backed target).
 *
 * Two things about this file that are easy to get wrong:
 *
 *   * There is no Qt event loop. Nothing calls exec(), so Qt's timers never
 *     fire on their own; the compositor pumps events once per frame.
 *   * QML animations are driven by an explicit animation driver fed from the
 *     compositor's clock. Left to itself Qt would animate off its own timer and
 *     drift against every transform around it.
 */

#include "host.h"

#include <QtCore/QAbstractAnimation>
#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QUrl>
#include <QtCore/QVariant>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtCore/QString>
#include <QtGui/QMouseEvent>
#include <QtQml/QQmlComponent>
#include <QtQml/QQmlEngine>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickRenderControl>
#include <QtQuick/QQuickRenderTarget>
#include <QtQuick/QQuickWindow>

namespace {

/*
 * An animation driver the compositor advances by hand.
 *
 * No Q_OBJECT: nothing here needs signals, slots or properties, so the build
 * needs no moc step.
 */
class CompositorAnimationDriver : public QAnimationDriver
{
public:
    qint64 elapsed() const override { return m_elapsed; }

    void advanceTo(qint64 elapsed)
    {
        m_elapsed = elapsed;
        advanceAnimation();
    }

private:
    qint64 m_elapsed = 0;
};

QGuiApplication *g_app = nullptr;
CompositorAnimationDriver *g_driver = nullptr;
int g_argc = 1;
char g_arg0[] = "solium";
char *g_argv[] = { g_arg0, nullptr };

} // namespace

struct SoliumQmlScene
{
    QQuickRenderControl *control = nullptr;
    QQuickWindow *window = nullptr;
    QQmlEngine *engine = nullptr;
    QQmlComponent *component = nullptr;
    QQuickItem *root = nullptr;
    QImage image;
    int width = 0;
    int height = 0;
    /* Whether the scene has changed since it was last rendered. */
    bool dirty = true;
    /* Backing store for the last value handed out by take_string. */
    QByteArray taken;
};

extern "C" int solium_qml_start(void)
{
    if (g_app != nullptr) {
        return 1;
    }

    // No windows are ever created: the scene renders into an image. The
    // offscreen platform is the one that does not expect a display server.
    qputenv("QT_QPA_PLATFORM", "offscreen");

    // The software rasteriser is a scene *graph adaptation*, not an RHI
    // backend, so it is selected by name here rather than through
    // setGraphicsApi -- which selects between OpenGL, Vulkan, Metal and D3D and
    // will happily accept `Software` while leaving the adaptation unchanged.
    // Getting that wrong fails later and unhelpfully, in
    // QQuickRenderControl::initialize.
    qputenv("QT_QUICK_BACKEND", "software");
    QQuickWindow::setSceneGraphBackend(QStringLiteral("software"));

    g_app = new QGuiApplication(g_argc, g_argv);
    if (g_app == nullptr) {
        return 0;
    }

    g_driver = new CompositorAnimationDriver();
    g_driver->install();
    return 1;
}

extern "C" SoliumQmlScene *solium_qml_scene_new(const char *qml_path, int width, int height,
                                                const char **error)
{
    const auto fail = [error](const char *message) -> SoliumQmlScene * {
        if (error != nullptr) {
            *error = message;
        }
        return nullptr;
    };

    if (g_app == nullptr) {
        return fail("qt was not started");
    }

    auto *scene = new SoliumQmlScene();
    scene->width = width > 0 ? width : 1;
    scene->height = height > 0 ? height : 1;

    scene->control = new QQuickRenderControl();
    scene->window = new QQuickWindow(scene->control);
    // Transparent, because the scene is composited over the desktop rather
    // than being a window in its own right.
    scene->window->setColor(Qt::transparent);
    scene->window->setGeometry(0, 0, scene->width, scene->height);

    // initialize() is not called at all. It sets up RHI resources, returns
    // false under the software adaptation, and — worse — makes a GL context
    // current on this thread on its way to failing. The compositor's own
    // eglMakeCurrent then fails with BAD_ACCESS, because EGL refuses to hand a
    // thread to one client API while another holds it. The software scene graph
    // needs none of it.

    scene->engine = new QQmlEngine();
    scene->component =
        new QQmlComponent(scene->engine, QUrl::fromLocalFile(QString::fromUtf8(qml_path)));
    if (scene->component->isError()) {
        static QByteArray reason;
        reason = scene->component->errorString().toUtf8();
        solium_qml_scene_free(scene);
        return fail(reason.constData());
    }

    QObject *created = scene->component->create();
    scene->root = qobject_cast<QQuickItem *>(created);
    if (scene->root == nullptr) {
        delete created;
        solium_qml_scene_free(scene);
        return fail("the QML root is not an Item");
    }

    scene->root->setParentItem(scene->window->contentItem());
    scene->root->setWidth(scene->width);
    scene->root->setHeight(scene->height);

    // Qt tells us when the scene needs redrawing, so an idle bar costs one
    // comparison per frame instead of a rasterisation and an upload.
    // Lambdas rather than slots, so this file still needs no moc.
    QObject::connect(scene->control, &QQuickRenderControl::renderRequested,
                     scene->control, [scene]() { scene->dirty = true; });
    QObject::connect(scene->control, &QQuickRenderControl::sceneChanged,
                     scene->control, [scene]() { scene->dirty = true; });

    // Premultiplied because that is what the compositor blends with; asking it
    // to un-premultiply every frame would be work for nothing.
    scene->image = QImage(scene->width, scene->height, QImage::Format_ARGB32_Premultiplied);
    scene->image.fill(Qt::transparent);

    return scene;
}

extern "C" void solium_qml_scene_free(SoliumQmlScene *scene)
{
    if (scene == nullptr) {
        return;
    }
    delete scene->root;
    delete scene->component;
    delete scene->engine;
    delete scene->window;
    delete scene->control;
    delete scene;
}

extern "C" void solium_qml_scene_resize(SoliumQmlScene *scene, int width, int height)
{
    if (scene == nullptr || width <= 0 || height <= 0) {
        return;
    }
    if (scene->width == width && scene->height == height) {
        return;
    }
    scene->width = width;
    scene->height = height;
    scene->window->setGeometry(0, 0, width, height);
    if (scene->root != nullptr) {
        scene->root->setWidth(width);
        scene->root->setHeight(height);
    }
    scene->image = QImage(width, height, QImage::Format_ARGB32_Premultiplied);
    scene->image.fill(Qt::transparent);
    scene->dirty = true;
}

extern "C" void solium_qml_scene_advance(SoliumQmlScene *scene, long long elapsed_ms)
{
    (void)scene;
    if (g_driver != nullptr) {
        g_driver->advanceTo(static_cast<qint64>(elapsed_ms));
    }
    // Nothing runs Qt's event loop, so queued work — component completion,
    // property change notifications, deleteLater — is serviced here or never.
    if (g_app != nullptr) {
        QCoreApplication::processEvents();
    }
}

extern "C" int solium_qml_scene_render(SoliumQmlScene *scene)
{
    if (scene == nullptr || scene->control == nullptr || scene->image.isNull()) {
        return 0;
    }
    if (!scene->dirty) {
        return SOLIUM_QML_UNCHANGED;
    }

    // The image is deliberately *not* cleared here. Qt's software renderer
    // repaints only the regions it considers dirty, so clearing every frame
    // erases everything that has not changed and leaves just the parts that
    // animate. That looked exactly like a broken upload path: a bar with one
    // moving dot on it and nothing else. The image is cleared once, when it is
    // created or resized, and Qt owns it after that.
    scene->window->setRenderTarget(QQuickRenderTarget::fromPaintDevice(&scene->image));

    // polish, sync, render — and *not* beginFrame/endFrame. Those bracket a
    // frame on the RHI, which the software adaptation does not have; calling
    // them logs "QQuickRenderControl: No QRhi in beginFrame()" twice per frame
    // per scene, which with a bar and a frame per window fills the journal with
    // warnings that look like errors and are not.
    scene->control->polishItems();
    scene->control->sync();
    scene->control->render();
    scene->dirty = false;
    return 1;
}

extern "C" const unsigned char *solium_qml_scene_pixels(const SoliumQmlScene *scene, int *stride)
{
    if (scene == nullptr || scene->image.isNull()) {
        return nullptr;
    }
    if (stride != nullptr) {
        *stride = static_cast<int>(scene->image.bytesPerLine());
    }
    return scene->image.constBits();
}

extern "C" const char *solium_qml_scene_take_string(SoliumQmlScene *scene, const char *name)
{
    if (scene == nullptr || scene->root == nullptr) {
        return nullptr;
    }
    const QString value = scene->root->property(name).toString();
    if (value.isEmpty()) {
        return nullptr;
    }
    scene->root->setProperty(name, QVariant(QString()));
    scene->taken = value.toUtf8();
    return scene->taken.constData();
}

extern "C" void solium_qml_scene_set_string(SoliumQmlScene *scene, const char *name,
                                            const char *value)
{
    if (scene == nullptr || scene->root == nullptr) {
        return;
    }
    scene->root->setProperty(name, QVariant(QString::fromUtf8(value)));
}

extern "C" void solium_qml_scene_set_bool(SoliumQmlScene *scene, const char *name, int value)
{
    if (scene == nullptr || scene->root == nullptr) {
        return;
    }
    scene->root->setProperty(name, QVariant(value != 0));
}

extern "C" int solium_qml_scene_get_bool(const SoliumQmlScene *scene, const char *name)
{
    if (scene == nullptr || scene->root == nullptr) {
        return 0;
    }
    return scene->root->property(name).toBool() ? 1 : 0;
}

extern "C" void solium_qml_scene_set_real(SoliumQmlScene *scene, const char *name, double value)
{
    if (scene == nullptr || scene->root == nullptr) {
        return;
    }
    scene->root->setProperty(name, QVariant(value));
}

extern "C" void solium_qml_scene_pointer(SoliumQmlScene *scene, double x, double y, int pressed)
{
    if (scene == nullptr || scene->window == nullptr) {
        return;
    }

    const QPointF at(x, y);
    QEvent::Type type = QEvent::MouseMove;
    Qt::MouseButton button = Qt::NoButton;
    Qt::MouseButtons buttons = Qt::NoButton;

    if (pressed == 1) {
        type = QEvent::MouseButtonPress;
        button = Qt::LeftButton;
        buttons = Qt::LeftButton;
    } else if (pressed == 0) {
        type = QEvent::MouseButtonRelease;
        button = Qt::LeftButton;
    }

    QMouseEvent event(type, at, at, button, buttons, Qt::NoModifier);
    QCoreApplication::sendEvent(scene->window, &event);
}
