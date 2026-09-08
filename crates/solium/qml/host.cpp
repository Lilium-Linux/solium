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
 * The second of those two ways is now also in this file, as a parallel set of
 * `_gpu` entry points: the compositor allocates a buffer through GBM, we import
 * its dmabuf into Qt's own context as a texture, and Qt renders into that. The
 * contexts still share nothing — the *buffer* is what crosses, which is what
 * two processes would have had to do anyway. Everything about the software path
 * below is unchanged and stays the fallback; a machine where the import fails
 * must still run a desktop. The two are chosen between once per process, in
 * solium_qml_start / solium_qml_start_gpu, because the scene graph backend is a
 * process-wide decision in Qt.
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

#include "compat.h"

#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>

#include <QtCore/QAbstractAnimation>
#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QSize>
#include <QtCore/QUrl>
#include <QtCore/QVariant>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtCore/QString>
#include <QtGui/QMouseEvent>
#include <QtGui/QOpenGLContext>
#include <QtGui/QOpenGLFunctions>
#include <QtQml/QQmlComponent>
#include <QtQml/QQmlEngine>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickRenderControl>
#include <QtQuick/QQuickRenderTarget>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGRendererInterface>

/*
 * EGL last, and deliberately so.
 *
 * <EGL/egl.h> reaches <EGL/eglplatform.h>, which on a good many Linux setups
 * still pulls in Xlib for EGLNativeDisplayType — and Xlib defines `None`,
 * `Status` and `Bool` as bare macros. Parsed before Qt's headers those break
 * the build in a way that reads like a Qt problem. dev/qtprobe/probe.cpp has
 * the same ordering for the same reason.
 *
 * The GLES2 headers are *not* included, even though the dmabuf import is a
 * GLES extension: gl2ext.h wants gl2.h's core typedefs, which fight with
 * whichever GL header Qt's own qopengl.h has already chosen. The one entry
 * point we need from it is declared by hand below instead, and the handful of
 * plain GL calls go through QOpenGLFunctions — see import_dmabuf_texture.
 */
#include <EGL/egl.h>
#include <EGL/eglext.h>

#include <cstring>

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

/*
 * One engine for every scene.
 *
 * This is the difference between "the decorations happen to be QML" and "the
 * desktop is one design system". Sharing an engine means every scene sees the
 * same singletons, so a theme is a single object rather than a copy per
 * surface — and it is the prerequisite for an item ever moving from the dock
 * into a titlebar, which cannot happen across two scene graphs, let alone two
 * processes.
 */
QQmlEngine *g_engine = nullptr;
int g_argc = 1;
char g_arg0[] = "solium";
char *g_argv[] = { g_arg0, nullptr };

/*
 * Which scene graph this process came up on.
 *
 * There is no separate "started" flag: `g_app != nullptr` has always been it,
 * and a second one would be a second authority over the same fact. This says
 * *which* of the two starters won the race to be first, so the other one can
 * refuse rather than hand back a host on a backend the caller did not ask for.
 */
bool g_gpu_mode = false;

/*
 * glEGLImageTargetTexture2DOES, declared rather than included.
 *
 * It lives in <GLES2/gl2ext.h>, which cannot be included beside Qt's own GL
 * headers without the two fighting over the core typedefs. The signature is one
 * line; GLeglImageOES is a void pointer by definition, and on every ABI this
 * compositor runs on APIENTRY is empty, so a plain function pointer is the same
 * thing. It is never linked directly either way — see import_dmabuf_texture.
 */
using ImageTargetTexture2D = void (*)(GLenum target, void *image);

/* DRM_FORMAT_MOD_INVALID, without dragging in <drm_fourcc.h> for one constant.
 * A buffer whose modifier is this has no *known* layout, and the import must
 * then leave the modifier attributes off entirely rather than pass the sentinel
 * through as if it were a real tiling. */
constexpr unsigned long long kModifierInvalid = (1ULL << 56) - 1;

} // namespace

struct SoliumQmlScene
{
    QQuickRenderControl *control = nullptr;
    QQuickWindow *window = nullptr;
    QQmlComponent *component = nullptr;
    QQuickItem *root = nullptr;
    QImage image;
    /* Device pixels: the size of the image the compositor uploads. */
    int width = 0;
    int height = 0;
    /* Device pixels per logical one. See solium_qml_scene_resize. */
    double scale = 1.0;
    /* Whether the scene has changed since it was last rendered. */
    bool dirty = true;
    /* Backing store for the last value handed out by take_string. */
    QByteArray taken;

    /* GPU scenes only. `image` is null on those and `texture` is zero on
     * software ones, so either could stand in for this flag — but "which path
     * is this scene on" is the question the code keeps asking, and answering it
     * by inspecting a side effect is how the two paths get tangled. */
    bool gpu = false;
    /* The compositor's dmabuf, imported into Qt's context. The EGLImage is kept
     * alongside the texture only so it can be destroyed with it: the texture
     * would outlive it perfectly well under EGL_KHR_image_base, but every
     * compositor that does this keeps the pair together, and a driver that
     * disagrees would disagree intermittently. */
    GLuint texture = 0;
    EGLImageKHR egl_image = EGL_NO_IMAGE_KHR;
};

/*
 * Everything the two starters have in common.
 *
 * Which scene graph to use has to be decided *before* this runs — Qt reads that
 * decision while the application object is being built — so backend selection
 * stays in the callers and only what comes after it lives here.
 */
static bool start_common(const char *import_path)
{
    g_app = new QGuiApplication(g_argc, g_argv);
    if (g_app == nullptr) {
        return false;
    }

    g_driver = new CompositorAnimationDriver();
    g_driver->install();

    // Shell types the compositor provides, registered before any scene can
    // ask for them.
    solium_qml_register_compat();

    g_engine = new QQmlEngine();
    solium_qml_install_icons(g_engine);
    if (import_path != nullptr) {
        // Colon-separated, like a PATH. One entry is the compositor's own
        // module, so a scene can `import Solium` and reach the theme; the rest
        // are for shell code being brought in from elsewhere, which needs its
        // own modules and a compatibility layer on the search path beside them.
        const auto paths = QString::fromUtf8(import_path).split(QLatin1Char(':'),
                                                                Qt::SkipEmptyParts);
        for (const auto &path : paths) {
            g_engine->addImportPath(path);
        }
    }
    return true;
}

extern "C" int solium_qml_start(const char *import_path)
{
    if (g_app != nullptr) {
        // Already up. If it came up on the GPU this is a caller asking for the
        // other backend, and Qt cannot give it one: say no rather than hand
        // back a host that will not do what the caller is about to assume.
        return g_gpu_mode ? 0 : 1;
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

    return start_common(import_path) ? 1 : 0;
}

extern "C" int solium_qml_start_gpu(const char *import_path)
{
    if (g_app != nullptr) {
        return g_gpu_mode ? 1 : 0;
    }

    // eglfs, not offscreen — and this is measured, not preferred.
    //
    // Qt Quick picks its scene graph *adaptation* from the platform plugin's
    // capabilities, and the offscreen plugin does not report OpenGL, so Qt
    // silently selects the software adaptation no matter what setGraphicsApi
    // below asks for. QQuickRenderControl::initialize() then refuses with
    // "QRhi is only compatible with default adaptation", which names neither
    // the platform nor the adaptation and reads like an RHI bug. Measured on
    // this machine, Qt 6.11, NVIDIA 610.57.04: offscreen never gets an RHI;
    // eglfs does, and the whole import/render/fence round trip works on it.
    //
    // eglfs loads its eglfs_kms integration, which opens /dev/dri/card1 and
    // builds a GBM device of its own. That is a thing to watch when this runs
    // inside the compositor rather than in a test harness, because the
    // compositor is already DRM master on that node — QT_QPA_EGLFS_INTEGRATION
    // is the knob if it turns out to matter. It did not need master for any of
    // what QQuickRenderControl does, which is all this path uses.
    //
    // Left alone if the environment already names a platform, so that
    // possibility stays testable from outside without a rebuild.
    if (qEnvironmentVariableIsEmpty("QT_QPA_PLATFORM")) {
        qputenv("QT_QPA_PLATFORM", "eglfs");
    }

    // The RHI path is selected by *not* naming the software backend, and by
    // asking for OpenGL explicitly. Both matter: the default backend is chosen
    // from the platform and is not OpenGL everywhere. Note the asymmetry with
    // solium_qml_start above — QT_QUICK_BACKEND is an adaptation, setGraphicsApi
    // is an RHI backend, and they are not two ways of saying the same thing.
    //
    // An externally set QT_QUICK_BACKEND=software still wins over this, which is
    // deliberate: it is a documented Qt escape hatch. It surfaces immediately as
    // QQuickRenderControl::initialize() failing, which is the loud failure.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);

    if (!start_common(import_path)) {
        return 0;
    }
    g_gpu_mode = true;
    return 1;
}

extern "C" void solium_qml_clear_cache()
{
    // QQmlEngine caches compiled QML by URL, so building a scene from a file
    // that has just been edited hands back the *old* compilation. Reloading
    // without this looks exactly like reloading working — the scene is rebuilt,
    // nothing throws, and the screen does not change.
    if (g_engine != nullptr) {
        g_engine->clearComponentCache();
    }
}

extern "C" SoliumQmlScene *solium_qml_scene_new(const char *qml_path, int width, int height,
                                                const char **error)
{
    return solium_qml_scene_new_with(qml_path, width, height, nullptr, error);
}

/*
 * Build the QML and hang it off the scene's window.
 *
 * Everything from the component to the dirty signals is identical whether the
 * pixels end up in a QImage or in a texture, so both constructors call this and
 * only the render target differs between them.
 *
 * On failure `*error` is pointed at a description and the scene is left intact
 * for the caller to free — the caller allocated it and knows what else it has
 * attached by now, which is not a decision to make from in here.
 */
static bool load_component(SoliumQmlScene *scene, const char *qml_path,
                           const char *initial_json, const char **error)
{
    const auto fail = [error](const char *message) {
        if (error != nullptr) {
            *error = message;
        }
        return false;
    };

    scene->component =
        new QQmlComponent(g_engine, QUrl::fromLocalFile(QString::fromUtf8(qml_path)));
    if (scene->component->isError()) {
        // Static, because the pointer outlives this frame: the caller frees the
        // scene — and with it the component the string came from — before it
        // ever looks at the message.
        static QByteArray reason;
        reason = scene->component->errorString().toUtf8();
        return fail(reason.constData());
    }

    // Required properties have to be supplied *at creation*: setting them
    // afterwards is too late, and the component simply fails to build. The
    // shell's dock declares `required property var screenInfo`, which is what
    // made it resolve and still refuse to exist.
    QVariantMap initial;
    if (initial_json != nullptr) {
        QJsonParseError parsed{};
        const auto document = QJsonDocument::fromJson(QByteArray(initial_json), &parsed);
        if (parsed.error == QJsonParseError::NoError && document.isObject()) {
            initial = document.object().toVariantMap();
        } else {
            qWarning("initial properties were not an object: %s",
                     qPrintable(parsed.errorString()));
        }
    }

    QObject *created = initial.isEmpty() ? scene->component->create()
                                         : scene->component->createWithInitialProperties(initial);
    scene->root = qobject_cast<QQuickItem *>(created);
    if (scene->root == nullptr) {
        delete created;
        return fail("the QML root is not an Item, or the component could not be created — a required property left unset will do this");
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

    return true;
}

extern "C" SoliumQmlScene *solium_qml_scene_new_with(const char *qml_path, int width, int height,
                                                     const char *initial_json, const char **error)
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
    // A scene is built at 1x and rescaled by `solium_qml_scene_resize` the
    // first time it is drawn, which every scene is.
    scene->scale = 1.0;

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
    // needs none of it. (The GPU constructor below does call it, and must.)

    const char *reason = nullptr;
    if (!load_component(scene, qml_path, initial_json, &reason)) {
        solium_qml_scene_free(scene);
        return fail(reason);
    }

    // Premultiplied because that is what the compositor blends with; asking it
    // to un-premultiply every frame would be work for nothing.
    scene->image = QImage(scene->width, scene->height, QImage::Format_ARGB32_Premultiplied);
    scene->image.fill(Qt::transparent);

    // And pointed at, here as well as in resize. A scene that is never resized
    // otherwise never gets a render target at all: Qt draws into nothing and
    // the image stays exactly as transparent as it was filled. Every scene the
    // compositor hosts is resized to its area each frame -- except the cursor,
    // which is a fixed 24x24 and so was invisible for its whole life. An
    // invisible pointer is not a cosmetic failure; it is indistinguishable from
    // input being dead.
    QQuickRenderTarget target = QQuickRenderTarget::fromPaintDevice(&scene->image);
    target.setDevicePixelRatio(scene->scale);
    scene->window->setRenderTarget(target);

    return scene;
}

/*
 * Import the compositor's dmabuf as a texture in whichever context is current.
 *
 * This is dev/qtprobe/probe.cpp's try_import with the allocation taken out: the
 * buffer is the compositor's now, and only its description crosses the FFI.
 * Everything the probe learned the hard way is preserved —
 *
 *   * The entry points are resolved through eglGetProcAddress, never linked.
 *     This system's libEGL.so exports only the core EGL 1.5 `eglCreateImage`
 *     and libGLESv2.so exports nothing at all for the OES import (checked with
 *     `nm -D`), so naming them directly fails at link time. That is not a local
 *     quirk either: direct linkage to an EXT/KHR/OES name is never guaranteed
 *     on any vendor's driver.
 *
 *   * The import only succeeds on an EGLDisplay paired with the same device the
 *     buffer was allocated on. The probe measured *both*: the default display
 *     refused the identical fd with EGL_BAD_MATCH (0x3009) while a
 *     EGL_PLATFORM_GBM_KHR display on the buffer's own gbm_device took it.
 *
 * The second point is the one to keep in mind here, because the display used is
 * whichever one Qt made current and nothing lets us choose it — handing Qt our
 * own context is not available (QEGLContext::fromNative returns null on every
 * plugin this Qt has, which is the whole reason for the shared-buffer design).
 * Measured: under eglfs, Qt's display is a GBM-platform display on
 * /dev/dri/card1, and it accepts a buffer allocated on /dev/dri/renderD128.
 * So the pairing the probe found is per *GPU*, not per gbm_device object or per
 * DRM node — which is what makes this design possible at all. It does mean a
 * second GPU would break it, silently and only on that machine, so the failure
 * below carries the EGL error code and the display pointer: 0x3009 there is the
 * signature of exactly that.
 */
static GLuint import_dmabuf_texture(int dmabuf_fd, int width, int height, int stride,
                                    unsigned long long modifier, unsigned int fourcc,
                                    EGLImageKHR *out_image)
{
    *out_image = EGL_NO_IMAGE_KHR;

    // Resolved once per process; the entry points do not depend on the display
    // they are later called with.
    static PFNEGLCREATEIMAGEKHRPROC create_image =
        reinterpret_cast<PFNEGLCREATEIMAGEKHRPROC>(eglGetProcAddress("eglCreateImageKHR"));
    static ImageTargetTexture2D target_texture =
        reinterpret_cast<ImageTargetTexture2D>(eglGetProcAddress("glEGLImageTargetTexture2DOES"));
    if (create_image == nullptr || target_texture == nullptr) {
        qWarning("dmabuf import: entry points missing (eglCreateImageKHR: %s, "
                 "glEGLImageTargetTexture2DOES: %s)",
                 create_image != nullptr ? "found" : "missing",
                 target_texture != nullptr ? "found" : "missing");
        return 0;
    }

    // Qt's display and Qt's context, because the texture has to exist in the
    // context Qt renders with — a texture on any other context is a name Qt
    // would happily bind and quietly draw nothing into.
    EGLDisplay display = eglGetCurrentDisplay();
    QOpenGLContext *context = QOpenGLContext::currentContext();
    if (display == EGL_NO_DISPLAY || context == nullptr) {
        qWarning("dmabuf import: Qt left no %s current — the RHI is not on EGL, "
                 "so there is no context to import into",
                 display == EGL_NO_DISPLAY ? "EGL display" : "QOpenGLContext");
        return 0;
    }

    const char *extensions = eglQueryString(display, EGL_EXTENSIONS);
    const bool has_import = extensions != nullptr &&
        strstr(extensions, "EGL_EXT_image_dma_buf_import") != nullptr;
    const bool has_modifiers = extensions != nullptr &&
        strstr(extensions, "EGL_EXT_image_dma_buf_import_modifiers") != nullptr;
    if (!has_import) {
        qWarning("dmabuf import: EGL_EXT_image_dma_buf_import absent on Qt's display");
        return 0;
    }

    // The modifier attributes need their own extension, and a buffer with no
    // known layout must not carry the INVALID sentinel through as if it were a
    // real tiling — either way the import goes without them and lets the driver
    // assume linear. Built by hand rather than as a fixed array because those
    // two cases differ only in whether four entries are there at all.
    EGLint attribs[15];
    int n = 0;
    attribs[n++] = EGL_WIDTH;                     attribs[n++] = width;
    attribs[n++] = EGL_HEIGHT;                    attribs[n++] = height;
    attribs[n++] = EGL_LINUX_DRM_FOURCC_EXT;      attribs[n++] = static_cast<EGLint>(fourcc);
    attribs[n++] = EGL_DMA_BUF_PLANE0_FD_EXT;     attribs[n++] = dmabuf_fd;
    attribs[n++] = EGL_DMA_BUF_PLANE0_OFFSET_EXT; attribs[n++] = 0;
    attribs[n++] = EGL_DMA_BUF_PLANE0_PITCH_EXT;  attribs[n++] = stride;
    if (has_modifiers && modifier != kModifierInvalid) {
        attribs[n++] = EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT;
        attribs[n++] = static_cast<EGLint>(modifier & 0xffffffffULL);
        attribs[n++] = EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT;
        attribs[n++] = static_cast<EGLint>(modifier >> 32);
    }
    attribs[n++] = EGL_NONE;

    EGLImageKHR image =
        create_image(display, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, nullptr, attribs);
    if (image == EGL_NO_IMAGE_KHR) {
        qWarning("dmabuf import: eglCreateImageKHR failed 0x%x  "
                 "(%dx%d fourcc=0x%x stride=%d modifier=0x%llx display=%p)  "
                 "— 0x3009 is EGL_BAD_MATCH, which here means Qt's display is not "
                 "paired with the device the buffer came from",
                 eglGetError(), width, height, fourcc, stride, modifier,
                 static_cast<void *>(display));
        return 0;
    }

    // Through Qt's own function table rather than libGLESv2. Qt may have built
    // this context as desktop GL, and calling into a second GL dispatch library
    // against it is the kind of thing that works everywhere until it does not.
    // QOpenGLFunctions is by definition the GL that Qt's RHI is driving.
    QOpenGLFunctions *gl = context->functions();
    GLuint texture = 0;
    gl->glGenTextures(1, &texture);
    gl->glBindTexture(GL_TEXTURE_2D, texture);
    target_texture(GL_TEXTURE_2D, image);
    const GLenum error = gl->glGetError();
    if (error != GL_NO_ERROR) {
        qWarning("dmabuf import: glEGLImageTargetTexture2DOES failed 0x%x", error);
        gl->glDeleteTextures(1, &texture);
        // The image exists even though nothing was bound to it, and *out_image
        // is still EGL_NO_IMAGE_KHR — so this is the only place it can be
        // released. The caller has no handle on it to free.
        static PFNEGLDESTROYIMAGEKHRPROC destroy_image =
            reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(eglGetProcAddress("eglDestroyImageKHR"));
        if (destroy_image != nullptr) {
            destroy_image(display, image);
        }
        return 0;
    }

    // An imported image has no mipmaps and cannot be wrapped, so the defaults
    // (mipmapped minification, repeat) leave the texture incomplete. It is only
    // ever an FBO attachment here, where completeness is not checked — but the
    // compositor samples the same buffer on the other side, and a texture that
    // is fine as a target and wrong as a source is a bug that only appears once
    // the picture is otherwise working.
    gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    gl->glBindTexture(GL_TEXTURE_2D, 0);

    *out_image = image;
    return texture;
}

extern "C" SoliumQmlScene *solium_qml_scene_new_gpu(const char *qml_path, int width, int height,
                                                    int dmabuf_fd, int stride,
                                                    unsigned long long modifier,
                                                    unsigned int fourcc,
                                                    const char *initial_json)
{
    if (g_app == nullptr || !g_gpu_mode) {
        qWarning("solium_qml_scene_new_gpu called without a GPU host — "
                 "solium_qml_start_gpu has to have succeeded first");
        return nullptr;
    }
    if (width <= 0 || height <= 0 || stride <= 0 || dmabuf_fd < 0) {
        qWarning("solium_qml_scene_new_gpu: %dx%d stride=%d fd=%d is not a buffer",
                 width, height, stride, dmabuf_fd);
        return nullptr;
    }

    auto *scene = new SoliumQmlScene();
    scene->gpu = true;
    scene->width = width;
    scene->height = height;
    // Built at 1x like a software scene; resize supplies the real ratio. The
    // buffer's pixel size is fixed, so for a GPU scene only the *scale* can
    // change afterwards — see solium_qml_scene_resize.
    scene->scale = 1.0;

    scene->control = new QQuickRenderControl();
    scene->window = new QQuickWindow(scene->control);
    scene->window->setColor(Qt::transparent);
    scene->window->setGeometry(0, 0, scene->width, scene->height);

    // The RHI path *requires* initialize(); the software path forbids it. This
    // is the one line where the two genuinely diverge, and it also has the side
    // effect the whole GPU design hangs off: it makes Qt's own context current
    // on this thread, which is what the import below needs and what the
    // compositor has to undo afterwards before its next eglMakeCurrent.
    if (!scene->control->initialize()) {
        qWarning("QQuickRenderControl::initialize failed — Qt could not bring up "
                 "an RHI on this platform plugin, so the GPU path is not available");
        solium_qml_scene_free(scene);
        return nullptr;
    }

    scene->texture = import_dmabuf_texture(dmabuf_fd, width, height, stride, modifier, fourcc,
                                           &scene->egl_image);
    if (scene->texture == 0) {
        solium_qml_scene_free(scene);
        return nullptr;
    }

    // The fd is not kept and not dup'd: EGL takes its own reference on the
    // buffer during eglCreateImageKHR, so the caller's fd is only borrowed for
    // the length of this call. Holding it here would be a second owner of a
    // lifetime the compositor already tracks through its Dmabuf.
    QQuickRenderTarget target =
        QQuickRenderTarget::fromOpenGLTexture(scene->texture, QSize(width, height));
    target.setDevicePixelRatio(scene->scale);
    scene->window->setRenderTarget(target);

    const char *reason = nullptr;
    if (!load_component(scene, qml_path, initial_json, &reason)) {
        qWarning("solium_qml_scene_new_gpu: %s", reason != nullptr ? reason : "the QML did not load");
        solium_qml_scene_free(scene);
        return nullptr;
    }
    return scene;
}

extern "C" void solium_qml_scene_free(SoliumQmlScene *scene)
{
    if (scene == nullptr) {
        return;
    }

    // GL first, while Qt's context still exists: deleting the control tears
    // down the RHI and the context with it, and a texture name outliving its
    // context is not something that can be freed afterwards.
    //
    // Only when Qt's context is actually current, though. By the time the
    // compositor drops a scene it has normally restored its own context, and
    // glDeleteTextures against *that* would delete whatever object happens to
    // share the name — silent corruption of an unrelated surface, which is far
    // worse than the leak. So the leak is taken, and named.
    if (scene->texture != 0 || scene->egl_image != EGL_NO_IMAGE_KHR) {
        QOpenGLContext *context = QOpenGLContext::currentContext();
        EGLDisplay display = eglGetCurrentDisplay();
        if (context != nullptr && display != EGL_NO_DISPLAY) {
            if (scene->texture != 0) {
                context->functions()->glDeleteTextures(1, &scene->texture);
            }
            if (scene->egl_image != EGL_NO_IMAGE_KHR) {
                static PFNEGLDESTROYIMAGEKHRPROC destroy_image =
                    reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(
                        eglGetProcAddress("eglDestroyImageKHR"));
                if (destroy_image != nullptr) {
                    destroy_image(display, scene->egl_image);
                }
            }
        } else {
            qWarning("a GPU scene was freed with no GL context current: texture %u "
                     "and its EGLImage are leaked until the process exits",
                     scene->texture);
        }
    }

    delete scene->root;
    delete scene->component;
    delete scene->window;
    delete scene->control;
    delete scene;
}

extern "C" void solium_qml_scene_resize(SoliumQmlScene *scene, int width, int height, double scale)
{
    if (scene == nullptr || width <= 0 || height <= 0) {
        return;
    }
    if (scale <= 0.0) {
        scale = 1.0;
    }
    if (scene->width == width && scene->height == height && scene->scale == scale) {
        return;
    }

    // A GPU scene draws into a buffer the compositor allocated, so its pixel
    // size is not this function's to change: the texture is exactly as big as
    // the dmabuf. Falling through would replace the texture target with a paint
    // device and move the scene silently back onto the CPU, still rendering,
    // still looking fine — into an image nobody uploads, while the compositor
    // keeps sampling a texture frozen on its last frame.
    //
    // A scale-only change is fine and does happen: the same buffer, laid out
    // for a different monitor ratio.
    if (scene->gpu) {
        if (width != scene->width || height != scene->height) {
            qWarning("a GPU scene cannot be resized in place (%dx%d to %dx%d): "
                     "rebuild it on a buffer of the new size",
                     scene->width, scene->height, width, height);
            return;
        }
        scene->scale = scale;
        const int logical_width = qMax(1, qRound(width / scale));
        const int logical_height = qMax(1, qRound(height / scale));
        scene->window->setGeometry(0, 0, logical_width, logical_height);
        if (scene->root != nullptr) {
            scene->root->setWidth(logical_width);
            scene->root->setHeight(logical_height);
        }
        QQuickRenderTarget target =
            QQuickRenderTarget::fromOpenGLTexture(scene->texture, QSize(width, height));
        target.setDevicePixelRatio(scale);
        scene->window->setRenderTarget(target);
        scene->dirty = true;
        return;
    }

    scene->width = width;
    scene->height = height;
    scene->scale = scale;

    // `width` and `height` are *device* pixels — the image the compositor will
    // upload. The scene itself is laid out in logical ones, and the ratio
    // between them is the device pixel ratio.
    //
    // This distinction is the whole of HiDPI here, and getting it wrong is not
    // subtle in either direction. Give QML the device size and it lays out in
    // it: `font.pixelSize: 14` is fourteen device pixels, so on a 2x monitor
    // the text comes out half the size it should be in a canvas twice as
    // large. Give it the logical size with no ratio and Qt rasterises at
    // logical resolution and the image is stretched — blurry chrome, which is
    // exactly what a badly scaled desktop looks like.
    //
    // Logical geometry, device-sized target, ratio between them. Then a
    // titlebar is 32 logical pixels on every monitor and is drawn with as many
    // real pixels as that monitor has.
    const int logical_width = qMax(1, qRound(width / scale));
    const int logical_height = qMax(1, qRound(height / scale));
    scene->window->setGeometry(0, 0, logical_width, logical_height);
    if (scene->root != nullptr) {
        scene->root->setWidth(logical_width);
        scene->root->setHeight(logical_height);
    }

    scene->image = QImage(width, height, QImage::Format_ARGB32_Premultiplied);
    scene->image.setDevicePixelRatio(scale);
    scene->image.fill(Qt::transparent);
    // Set with the image rather than before every render: it is a property of
    // where the scene draws, and that only changes when the image does.
    QQuickRenderTarget target = QQuickRenderTarget::fromPaintDevice(&scene->image);
    target.setDevicePixelRatio(scale);
    scene->window->setRenderTarget(target);
    scene->dirty = true;
}

/* Advance every animation in the process, once for the whole frame.
 *
 * The driver and the event loop are one per process, not one per scene, so
 * doing this per scene did the same global work once per decorated window.
 * More importantly it used to be skipped when a scene looked settled -- and an
 * animation that is not advanced never changes, never marks itself dirty, and
 * never gets advanced again. Ticking is unconditional and cheap; *rendering*
 * is what waits to be asked. */
extern "C" void solium_qml_tick(long long elapsed_ms)
{
    /* Two different jobs, at two different rates.
     *
     * Advancing the animations has to happen every frame, or an animation
     * that is not ticked never changes, never asks to be drawn, and never
     * gets ticked again -- which is how a loop with a pause in it dies at its
     * first pause. With nothing registered it costs almost nothing, so it is
     * unconditional.
     *
     * Draining Qt's event queue is the expensive half, and none of what is in
     * there is frame-critical: component completion, deleteLater, queued
     * notifications. Property changes are delivered synchronously and do not
     * wait for this. So it runs at 60Hz however fast the screen is, which on
     * a 260Hz monitor is a quarter of the work for the same behaviour. */
    if (g_driver != nullptr) {
        g_driver->advanceTo(static_cast<qint64>(elapsed_ms));
    }
    if (g_app != nullptr) {
        static long long drained_at = 0;
        if (elapsed_ms - drained_at >= 16 || elapsed_ms < drained_at) {
            drained_at = elapsed_ms;
            QCoreApplication::processEvents();
        }
    }
}

/* Whether Qt has asked for this scene to be drawn again. */
extern "C" int solium_qml_scene_dirty(const SoliumQmlScene *scene)
{
    return (scene != nullptr && scene->dirty) ? 1 : 0;
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

/*
 * A fence for the frame Qt has just submitted, or a CPU wait instead.
 *
 * The compositor samples this buffer from a different context on a different
 * device queue. Without something ordering the two it reads whatever has landed
 * so far, which shows up as intermittent tearing and half-drawn chrome — the
 * worst kind of bug to find out about, because it is invisible until it is not
 * and never reproduces on demand.
 *
 * Returns the fence fd for the caller to own and close, or -1 having already
 * waited on the CPU. -1 is the correct answer and not an error: it just costs a
 * stall instead of a hand-off.
 */
static int fence_after_render()
{
    EGLDisplay display = eglGetCurrentDisplay();
    QOpenGLContext *context = QOpenGLContext::currentContext();
    if (display == EGL_NO_DISPLAY || context == nullptr) {
        // Nothing to fence *with* and nothing to flush *through*. The scene did
        // render, so this is not a render failure — but the caller has to be
        // told there is no ordering, and -1 is exactly that statement.
        qWarning("no EGL display or GL context current after a GPU render: "
                 "the frame is unfenced and unflushed");
        return -1;
    }

    // Extension entry points, resolved not linked — the same rule as the import.
    static PFNEGLCREATESYNCKHRPROC create_sync =
        reinterpret_cast<PFNEGLCREATESYNCKHRPROC>(eglGetProcAddress("eglCreateSyncKHR"));
    static PFNEGLDESTROYSYNCKHRPROC destroy_sync =
        reinterpret_cast<PFNEGLDESTROYSYNCKHRPROC>(eglGetProcAddress("eglDestroySyncKHR"));
    static PFNEGLDUPNATIVEFENCEFDANDROIDPROC dup_fence =
        reinterpret_cast<PFNEGLDUPNATIVEFENCEFDANDROIDPROC>(
            eglGetProcAddress("eglDupNativeFenceFDANDROID"));

    QOpenGLFunctions *gl = context->functions();
    if (create_sync == nullptr || destroy_sync == nullptr || dup_fence == nullptr) {
        // EGL_ANDROID_native_fence_sync is present on this machine (the probe
        // checked), so this branch is for the machines where it is not. A
        // glFinish is correct, just expensive: it blocks until the GPU is idle,
        // which is a superset of "this frame has landed".
        gl->glFinish();
        return -1;
    }

    EGLSyncKHR sync = create_sync(display, EGL_SYNC_NATIVE_FENCE_ANDROID, nullptr);
    if (sync == EGL_NO_SYNC_KHR) {
        gl->glFinish();
        return -1;
    }

    // Flush *between* creating the sync and dup'ing it, which is the order the
    // extension requires and not an arbitrary one: the fence is inserted into
    // the command stream by eglCreateSyncKHR, and eglDupNativeFenceFDANDROID is
    // only defined once that command has actually been submitted. Dup first and
    // the driver has a fence it has not been asked to schedule.
    gl->glFlush();
    const int fence_fd = dup_fence(display, sync);
    destroy_sync(display, sync);

    if (fence_fd == EGL_NO_NATIVE_FENCE_FD_ANDROID) {
        // The driver made a sync object and then declined to export it. Same
        // fallback: wait here, and say so with -1.
        gl->glFinish();
        return -1;
    }
    return fence_fd;
}

extern "C" int solium_qml_scene_render_gpu(SoliumQmlScene *scene, int *fence_fd)
{
    if (scene == nullptr || fence_fd == nullptr) {
        return 0;
    }
    // Set before any early return, so a caller that ignores the return value
    // still never closes an uninitialised fd.
    *fence_fd = -1;

    if (scene->control == nullptr || !scene->gpu || scene->texture == 0) {
        qWarning("solium_qml_scene_render_gpu on a scene that is not a GPU scene");
        return 0;
    }
    if (!scene->dirty) {
        return SOLIUM_QML_UNCHANGED;
    }

    // polish, begin, sync, render, end — and here beginFrame/endFrame *are*
    // required, which is the exact inverse of the software path above. They
    // bracket a frame on the RHI; the software adaptation has no RHI and logs
    // about it, the RHI path has one and needs it told when a frame starts and
    // stops. Same five calls, one pair present in one path and absent in the
    // other, and neither is a copy of the other with a line missing.
    scene->control->polishItems();
    scene->control->beginFrame();
    scene->control->sync();
    scene->control->render();
    scene->control->endFrame();
    scene->dirty = false;

    *fence_fd = fence_after_render();
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

extern "C" void solium_qml_scene_set_int(SoliumQmlScene *scene, const char *name, int value)
{
    if (scene == nullptr || scene->root == nullptr) {
        return;
    }
    scene->root->setProperty(name, QVariant(value));
}

/* How much of the window a decoration reserves is the decoration's decision,
 * so it is read back from QML rather than configured beside it. */
extern "C" int solium_qml_scene_get_int(const SoliumQmlScene *scene, const char *name)
{
    if (scene == nullptr || scene->root == nullptr) {
        return 0;
    }
    return scene->root->property(name).toInt();
}

extern "C" int solium_qml_scene_get_bool(const SoliumQmlScene *scene, const char *name)
{
    if (scene == nullptr || scene->root == nullptr) {
        return 0;
    }
    return scene->root->property(name).toBool() ? 1 : 0;
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
