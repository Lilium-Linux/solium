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
// qInstallMessageHandler, QMessageLogContext and QtMsgType. Reached through
// QtGlobal by everything else in here that calls qWarning; named explicitly
// because this file now *installs* the handler rather than only feeding it.
#include <QtCore/QtMessageHandler>
#include <QtCore/QSize>
#include <QtCore/QUrl>
#include <QtCore/QVariant>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtCore/QString>
#include <QtGui/QMouseEvent>
#include <QtGui/QOpenGLContext>
#include <QtGui/QOpenGLFunctions>
#include <QtGui/QSurface>
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

#include <algorithm>
#include <cstring>
#include <vector>

namespace {

/*
 * An animation driver the compositor advances by hand.
 *
 * No Q_OBJECT: nothing here needs signals, slots or properties, so the build
 * needs no moc step.
 *
 * What it reports is *not* the compositor's clock, and the difference is the
 * whole of this class. Qt's contract for `elapsed()` is "the number of
 * milliseconds since the animations was started" -- QAnimationDriver's own
 * implementation is `d->running ? d->timer.elapsed() : 0`, with the timer
 * restarted inside `start()` (qtbase v6.11.2,
 * src/corelib/animation/qabstractanimation.cpp:826-857). It is time *since
 * animating began*, not a process clock, and Qt relies on that:
 *
 *   When the process goes from having no animations at all to having one,
 *   `QUnifiedTimer::startTimers` finds its reference time invalid and resets
 *   the lot -- `lastTick = 0; time.start(); temporalDrift = 0;
 *   driverStartTime = 0` (ibid. :378-389). The next tick computes
 *   `delta = elapsed() - lastTick`, so whatever `elapsed()` returns at that
 *   moment is handed to the newly started animation as its first step.
 *
 * Reporting the compositor's uptime there hands a brand new animation the
 * entire uptime in one step. Measured on this Qt, offscreen, software
 * adaptation, with the compositor's own loop: reveal.qml's 260ms appear
 * animation went from y=-34 to y=0 in a single tick, at every uptime from
 * 640ms to an hour, with or without an idle gap before it. It never played.
 *
 * Qt cannot correct for it either, because the correction it has --
 * `startAnimationDriver` setting `driverStartTime = elapsed()` -- only runs
 * when the driver is stopped, and this driver is not: `QUnifiedTimer::restart`
 * -> `localRestart` starts it again whenever no animation is registered, so it
 * is started once at the first tick of the process and runs for ever after.
 * Measured here: one start at 16ms, one stop for a single tick when an
 * animation finished, and a restart immediately after with nothing animating.
 *
 * So the origin is kept here instead, and dragged along behind the clock for as
 * long as the process has nothing animating. `elapsed()` reads 0 across a still
 * desktop and starts counting from the animation that breaks it, which is what
 * Qt's own driver would have reported and what its bookkeeping assumes.
 *
 * The caller answers "is anything animating"; see `solium_qml_tick`.
 *
 * Note what this deliberately does *not* do: it does not clamp the step. While
 * anything is animating the origin is frozen, so this clock advances exactly
 * with `Clock::now()` and a frame that arrives late advances every animation by
 * however long it was late -- a 300ms stall moves a 260ms animation straight to
 * its end.
 *
 * That is correct, and clamping it would be a second and worse defect. The
 * compositor's own transforms -- `present.rs`, the pane rectangles a decoration
 * is *drawn on* -- are computed from `now` directly, with no clamp and no
 * catching up to do. A QML clock that refused to skip would fall behind them on
 * every hitch and stay behind, which is a titlebar easing at a different rate
 * from the window it is attached to. Keeping the two in step is the whole
 * reason this driver is hand-fed rather than left on Qt's own timer; see the
 * top of this file. An animation starved of frames and then jumping to where
 * the clock says it should be is the same answer every other animated thing on
 * the screen gives.
 *
 * The origin only ever moves across stretches in which no QML animation exists,
 * so it cannot introduce that drift either: there is nothing to be in step with
 * while it slides.
 */
class CompositorAnimationDriver : public QAnimationDriver
{
public:
    qint64 elapsed() const override { return m_elapsed - m_origin; }

    void advanceTo(qint64 elapsed, bool anything_animating)
    {
        m_elapsed = elapsed;
        /* Nothing is animating, so no animation can be measuring from here:
         * move the origin up and report zero. The moment one starts, the origin
         * is already where it should be -- the frame it started on. */
        if (!anything_animating) {
            m_origin = elapsed;
        }
        advanceAnimation();
    }

private:
    qint64 m_elapsed = 0;
    qint64 m_origin = 0;
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
    /* Which EGL display and context the two above belong to, captured at import
     * from eglGetCurrentDisplay/eglGetCurrentContext.
     *
     * Recorded rather than re-derived because at teardown there is no other way
     * to ask the question that matters. Qt's QOpenGLContext::currentContext() is
     * a thread-local Qt sets in its own makeCurrent; the compositor takes the
     * thread back with a raw eglMakeCurrent, which Qt never sees, so that
     * thread-local stays pointing at Qt's context — stale, not null. Comparing
     * these two against eglGetCurrentContext/eglGetCurrentDisplay is an
     * EGL-level question and gets an EGL-level answer. */
    EGLDisplay egl_display = EGL_NO_DISPLAY;
    EGLContext egl_context = EGL_NO_CONTEXT;
    /* The same two facts at Qt's level, captured at the same moment: the
     * QOpenGLContext Qt renders with and the surface it was made current
     * against.
     *
     * Kept because taking the thread *back* for this scene needs both, and
     * neither can be re-derived later. QOpenGLContext::currentContext() is the
     * thread-local this whole file distrusts, and QOpenGLContext::surface() is
     * cleared by doneCurrent() — which is exactly the state a rebind is reached
     * in. See take_the_thread. Both belong to the render control and live as
     * long as it does, which is longer than any scene of ours. */
    QOpenGLContext *qt_context = nullptr;
    QSurface *qt_surface = nullptr;
};

namespace {

/*
 * Every scene in the process.
 *
 * The driver's clock is one per process and has to be told whether *anything*
 * is animating, not whether one scene is — an animation in the dock is as good
 * a reason to keep the clock running as one in a titlebar, and a clock rebased
 * while the dock is mid-sweep would jump it. No scene can answer that; only the
 * set of them can, and this file is the only place the set exists. See
 * `CompositorAnimationDriver`.
 *
 * A raw vector rather than anything cleverer: scenes are created and freed by
 * hand through the two `_new` entry points and `solium_qml_scene_free`, there
 * are as many of them as there are decorated windows, and the only operations
 * are append, erase-one and walk.
 */
std::vector<SoliumQmlScene *> g_scenes;

} // namespace

/*
 * Every Qt message, on its way to the compositor's log.
 *
 * Qt's own levels onto ours. Nothing else is decided here: the Rust side does
 * the formatting and calls one `tracing` macro, which is the whole of what a
 * message handler is allowed to do — see the re-entrancy note below.
 */
static void forward_qt_message(QtMsgType type, const QMessageLogContext &context,
                               const QString &message)
{
    /* One message at a time, per thread.
     *
     * A handler that logs through anything that can itself log is a warning
     * that warns about itself, and the recursion is unbounded: it takes the
     * session rather than producing a line. Nothing on the Rust side can reach
     * qWarning today — it formats and calls a macro — but "today" is the part
     * that stops being true, and dropping the inner message is the only exit
     * that does not need the outer one to finish first.
     *
     * Per thread rather than global because qInstallMessageHandler's contract
     * says the handler may be called from any thread, and a global flag would
     * silently swallow a second thread's messages rather than a loop. */
    static thread_local bool forwarding = false;
    if (forwarding) {
        return;
    }
    forwarding = true;

    int level = SOLIUM_QML_LOG_WARN;
    switch (type) {
    case QtDebugMsg:
        level = SOLIUM_QML_LOG_DEBUG;
        break;
    case QtInfoMsg:
        level = SOLIUM_QML_LOG_INFO;
        break;
    case QtWarningMsg:
        level = SOLIUM_QML_LOG_WARN;
        break;
    case QtCriticalMsg:
    /* Qt aborts as soon as this returns and there is nothing here that could
     * stop it, nor anything that should try: a qFatal is Qt saying it cannot
     * continue. Saying it at `error` first is the whole of what is available,
     * and is the difference between a log that ends mid-sentence and one that
     * ends with the reason. */
    case QtFatalMsg:
        level = SOLIUM_QML_LOG_ERROR;
        break;
    }

    /* The QByteArray owns these bytes for exactly as long as the call the Rust
     * side may read them in. QMessageLogContext's own strings have the same
     * lifetime and go straight through; `category` is "default" when Qt has no
     * better answer, and the null check is for a caller that built a context by
     * hand. */
    const QByteArray text = message.toUtf8();
    solium_qml_log_from_qt(level,
                           context.category != nullptr ? context.category : "default",
                           text.constData(), context.file, context.line, context.function);

    forwarding = false;
}

/*
 * Point Qt's diagnostic channel at the compositor's log, once.
 *
 * Everything about this is about being *early*. QGuiApplication's constructor
 * warns — about platform plugins, about missing fonts, about a display it
 * cannot open — and a handler installed after it has already lost the messages
 * that say why the process is about to behave oddly. So both starters open
 * with this, before their own qputenv and backend calls, and it is idempotent
 * so that being called from two places is not a thing to reason about.
 *
 * The previous handler is dropped rather than kept and chained. Qt's default
 * one is what this replaces; chaining would print every message twice wherever
 * that handler prints at all, and where it prints is the problem this fixes.
 */
static void route_qt_diagnostics()
{
    static bool installed = false;
    if (installed) {
        return;
    }
    installed = true;
    qInstallMessageHandler(forward_qt_message);
}

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
    // First, and before the qputenv pair below: those are the last lines in
    // this process that can run before Qt is capable of saying anything.
    route_qt_diagnostics();

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
    // First here too, and `QQuickWindow::setGraphicsApi` below is the reason it
    // is not simply done once inside start_common: that call is Qt API, it
    // warns when it is made too late, and start_common runs after it.
    route_qt_diagnostics();

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
    // The mirror of the check solium_qml_scene_new_gpu opens with, and not
    // symmetry for its own sake: a *software* scene on a GPU host is a
    // segfault, not a bad-looking frame.
    //
    // Everything below succeeds under g_gpu_mode — the window is built, the QML
    // loads, the QImage is attached — and the crash arrives a frame later,
    // inside solium_qml_scene_render's control->sync(). With the RHI adaptation
    // selected Qt builds a QSGBatchRenderer, which asks the render context for
    // its QRhi; this scene never called initialize(), so there is none, and Qt
    // dereferences it regardless. QRhi::ubufAlignment() on a null this, SIGSEGV,
    // the session gone. Measured: the compositor came up, drew its first cursor
    // frame and died in that call.
    //
    // Refusing at construction turns that into a scene that will not load,
    // which every caller in the compositor already handles by drawing nothing
    // and saying so — a bare window, an invisible pointer, a gap in the
    // picture. All recoverable, none of them fatal.
    if (g_gpu_mode) {
        return fail("this process came up on the GPU scene graph and a software scene cannot "
                    "render on it; build it with solium_qml_scene_new_gpu instead");
    }

    auto *scene = new SoliumQmlScene();
    // Registered before anything can fail, so that the `solium_qml_scene_free`
    // on every error path below is also what takes it back out again.
    g_scenes.push_back(scene);
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
static bool import_dmabuf_texture(SoliumQmlScene *scene, int dmabuf_fd, int stride,
                                  unsigned long long modifier, unsigned int fourcc)
{
    const int width = scene->width;
    const int height = scene->height;

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
        return false;
    }

    // Qt's display and Qt's context, because the texture has to exist in the
    // context Qt renders with — a texture on any other context is a name Qt
    // would happily bind and quietly draw nothing into.
    //
    // Both are read at the EGL level and *recorded on the scene*, which is what
    // makes the texture safe to delete later. See solium_qml_scene_free: at
    // teardown the only honest question is "is the context this texture belongs
    // to the one current right now", and that can only be answered by comparing
    // against the values captured here. QOpenGLContext::currentContext() cannot
    // answer it — it is Qt's own thread-local, set by QOpenGLContext::makeCurrent
    // and untouched by the raw eglMakeCurrent the compositor uses to take the
    // thread back, so it goes stale rather than null.
    EGLDisplay display = eglGetCurrentDisplay();
    EGLContext egl_context = eglGetCurrentContext();
    QOpenGLContext *context = QOpenGLContext::currentContext();
    if (display == EGL_NO_DISPLAY || egl_context == EGL_NO_CONTEXT || context == nullptr) {
        qWarning("dmabuf import: Qt left no %s current — the RHI is not on EGL, "
                 "so there is no context to import into",
                 display == EGL_NO_DISPLAY      ? "EGL display"
                 : egl_context == EGL_NO_CONTEXT ? "EGL context"
                                                 : "QOpenGLContext");
        return false;
    }

    const char *extensions = eglQueryString(display, EGL_EXTENSIONS);
    const bool has_import = extensions != nullptr &&
        strstr(extensions, "EGL_EXT_image_dma_buf_import") != nullptr;
    const bool has_modifiers = extensions != nullptr &&
        strstr(extensions, "EGL_EXT_image_dma_buf_import_modifiers") != nullptr;
    if (!has_import) {
        qWarning("dmabuf import: EGL_EXT_image_dma_buf_import absent on Qt's display");
        return false;
    }

    // Sized by its own initialiser, never counted by hand.
    //
    // The first version of this counted the entries itself and declared
    // `EGLint attribs[15]` for a list of seventeen, writing eight bytes past
    // the end — on the path that *works*, modifier present, which is why it
    // looked fine. Nothing diagnoses that: the index is a runtime variable, so
    // -Wall -Wextra at -O2 says nothing and the corruption is whatever happened
    // to be in those eight bytes of frame. The array now takes its length from
    // the list itself, which is the form that cannot be miscounted.
    //
    // Both cases live in one list because an EGL attribute list ends at its
    // first EGL_NONE: dropping the modifier pairs is a matter of moving the
    // terminator up over them rather than building a second list. The modifier
    // attributes need their own extension, and a buffer with no known layout
    // must not carry the INVALID sentinel through as if it were a real tiling —
    // either way the import goes without them and lets the driver assume linear.
    EGLint attribs[] = {
        EGL_WIDTH, width,
        EGL_HEIGHT, height,
        EGL_LINUX_DRM_FOURCC_EXT, static_cast<EGLint>(fourcc),
        EGL_DMA_BUF_PLANE0_FD_EXT, dmabuf_fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, stride,
        EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, static_cast<EGLint>(modifier & 0xffffffffULL),
        EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, static_cast<EGLint>(modifier >> 32),
        EGL_NONE,
    };
    // Where EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT sits above. The assertion is the
    // point: any edit to the list changes its length and trips it, so the index
    // cannot quietly drift away from the entry it names.
    constexpr size_t modifier_lo_index = 12;
    static_assert(sizeof(attribs) / sizeof(attribs[0]) == modifier_lo_index + 5,
                  "the two modifier pairs must be the last four entries before EGL_NONE");
    if (!has_modifiers || modifier == kModifierInvalid) {
        attribs[modifier_lo_index] = EGL_NONE;
    }

    EGLImageKHR image =
        create_image(display, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, nullptr, attribs);
    if (image == EGL_NO_IMAGE_KHR) {
        qWarning("dmabuf import: eglCreateImageKHR failed 0x%x  "
                 "(%dx%d fourcc=0x%x stride=%d modifier=0x%llx display=%p)  "
                 "— 0x3009 is EGL_BAD_MATCH, which here means Qt's display is not "
                 "paired with the device the buffer came from",
                 eglGetError(), width, height, fourcc, stride, modifier,
                 static_cast<void *>(display));
        return false;
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
        // The image exists even though nothing was bound to it, and nothing has
        // been recorded on the scene — so this is the only place it can be
        // released. The caller has no handle on it to free.
        static PFNEGLDESTROYIMAGEKHRPROC destroy_image =
            reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(eglGetProcAddress("eglDestroyImageKHR"));
        if (destroy_image != nullptr) {
            destroy_image(display, image);
        }
        return false;
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

    scene->texture = texture;
    scene->egl_image = image;
    scene->egl_display = display;
    scene->egl_context = egl_context;
    // Qt's own handles on the same thing, recorded here because here is the one
    // moment they can be trusted: Qt genuinely holds the thread. `surface()` in
    // particular is not available later — doneCurrent() clears it. See
    // take_the_thread.
    scene->qt_context = context;
    scene->qt_surface = context->surface();
    return true;
}

/*
 * Store the frame the way the rest of the world stores an image: row 0 on top.
 *
 * Qt Quick renders through QRhi, and on OpenGL QRhi leaves a texture render
 * target in the framebuffer's own orientation — origin bottom-left, so the
 * scene's *top* row lands in the buffer's *last* row. That is correct and
 * conventional for a texture Qt is going to sample itself. It is wrong for this
 * one, which is a dmabuf the compositor imports: a dmabuf is top-down unless it
 * carries DRM_FORMAT_MOD/Y_INVERT saying otherwise, ours does not, and every
 * consumer of it — smithay's importer, a screencopy client, a later KMS plane —
 * reads row 0 as the top. Without this the shell is drawn upside down.
 *
 * Measured, not assumed: with this off, a four-quadrant probe scene read back
 * from an independent EGL context has its QML top half in the buffer's bottom
 * half, exactly and only mirrored. See dev/README.md.
 *
 * Done here rather than in the compositor because both of the Rust-side levers
 * are worse. Smithay's `y_inverted` texture flag negates the texture matrix's
 * y row without the matching translation, which samples outside the texture;
 * and a `Transform::Flipped180` on the render element mirrors within the
 * element's *logical* size while the source rectangle is in device pixels, so
 * it is right at scale 1 and wrong on every scaled monitor. Orientation belongs
 * to whoever fills the buffer.
 */
static void mirror_for_the_compositor(QQuickRenderTarget *target)
{
    target->setMirrorVertically(true);
}

/*
 * Is this scene's own GL context the one current on this thread right now?
 *
 * The single owner of that question. It was asked in two places with two
 * different answers — the texture delete compared display *and* context, the
 * render path compared only the context — and in a third, the teardown reached
 * through `delete scene->control`, it was not asked at all. One rule, one
 * place, so a fourth site inherits it rather than re-deriving it.
 *
 * Asked at the EGL level, deliberately. QOpenGLContext::currentContext() cannot
 * answer it: it is a thread-local Qt sets inside its own makeCurrent, and the
 * compositor takes the thread back with a raw eglMakeCurrent that Qt never
 * sees, so that thread-local goes *stale rather than null*. The values compared
 * here were captured from eglGetCurrentDisplay/eglGetCurrentContext at import,
 * where Qt genuinely did have the thread.
 *
 * Both halves and not just the context: a context handle is only meaningful
 * against the display that issued it, and the stricter of the two old tests is
 * the one the texture delete needs. Nothing that was safe under the looser test
 * is unsafe under this one.
 *
 * Split in two so that the rule has one owner even when the pair being asked
 * about is not the one currently recorded on the scene. solium_qml_scene_rebind
 * disposes of the *previous* texture after the import has already overwritten
 * the recording with the new one, and asking the scene there would compare the
 * old name against the new record — which is true by construction and answers
 * nothing. It asks about the pair it saved instead, through the same rule.
 *
 * Context first and display second, so that the call reads in the order the
 * name does. EGLDisplay and EGLContext are both `void *`, so a transposed call
 * is not a type error — it would compile, always return false, and silently
 * skip the texture delete. That is a leak rather than corruption, which makes
 * it the one outcome of the three here that nothing would ever report.
 */
static bool context_is_current(EGLContext context, EGLDisplay display)
{
    return context != EGL_NO_CONTEXT && display != EGL_NO_DISPLAY &&
        eglGetCurrentContext() == context && eglGetCurrentDisplay() == display;
}

static bool scene_context_is_current(const SoliumQmlScene *scene)
{
    return context_is_current(scene->egl_context, scene->egl_display);
}

/*
 * Tell Qt the truth about whose context is current, when it has it wrong.
 *
 * This is the other half of the fact solium_qml_scene_free already documents,
 * and it decides whether the second frame draws at all — and, on the teardown
 * path, whether Qt deletes its own GL objects or the compositor's.
 *
 * QOpenGLContext::currentContext() is a thread-local Qt sets in its own
 * makeCurrent. The compositor takes the thread back with a raw eglMakeCurrent —
 * it has to; it is not a Qt program — and Qt never sees that, so the
 * thread-local goes stale rather than null. QRhiGles2::ensureContext() then
 * asks exactly that question, believes its context is already current, skips
 * the makeCurrent it needs, and issues the whole frame against whatever context
 * really is current: the compositor's. Nothing fails. beginFrame, sync, render
 * and endFrame all return, the fence is real and signals, and the dmabuf stays
 * empty, because the FBO and texture names Qt drew through mean something else
 * — or nothing — in the compositor's context.
 *
 * Measured on this machine, and it is not subtle once you know where to look:
 * with the compositor's context current across a render the buffer reads back
 * as 16384 zero bytes and with Qt's it reads back as the frame, byte for byte
 * identical to the software path. The first frame after a scene is built works
 * either way, because initialize() left Qt's context current and nothing has
 * taken it yet, which is exactly why a one-frame probe cannot see this.
 *
 * On a *teardown* it is worse, because teardown deletes. QRhiGles2::destroy()
 * and the scenegraph invalidate reached through `delete scene->window` and
 * `delete scene->control` both go through the same ensureContext(), and then
 * executeDeferredReleases() issues glDeleteTextures, glDeleteBuffers,
 * glDeleteFramebuffers and glDeleteProgram for Qt's own names. Against the
 * compositor's context those integers name the compositor's objects — a client
 * surface, the texture program, a vertex buffer — and they are deleted.
 *
 * doneCurrent() is the supported way to say it: it releases the context and
 * clears the thread-local, so ensureContext() below finds no current context
 * and makes its own current properly. It also releases the *compositor's*
 * context from this thread, which is fine and expected — the compositor
 * restores its own context after every call in here, because Qt's teardown
 * leaves none current anyway.
 */
static void clear_stale_current_context(const SoliumQmlScene *scene)
{
    QOpenGLContext *believed = QOpenGLContext::currentContext();
    if (believed == nullptr || scene_context_is_current(scene)) {
        return;
    }
    believed->doneCurrent();
}

/*
 * Give the thread back with *no* GL context current on it.
 *
 * QQuickRenderControl::initialize() makes Qt's context current and never puts
 * anything back, so building a GPU scene left Qt holding a thread the
 * compositor also uses. Undoing that was the caller's job — and a caller with
 * no renderer to hand could not do it at all. ShellSurface::new is exactly
 * that: it is reached from a client attaching, with no frame in sight and
 * nothing to restore *to*. An obligation only some callers can discharge is not
 * an obligation, it is a bug waiting for the third call site.
 *
 * So it is discharged here, and the postcondition of building a GPU scene is
 * the same as the postcondition of freeing one: nothing is current.
 *
 * "Nothing current" and not "the compositor's context current", because this
 * file has no way to name the compositor's context and does not need one. The
 * danger was never an empty thread; it is somebody *else's* context on it.
 * Every entry point on GlesRenderer itself re-binds its own context before it
 * touches GL — import_dmabuf, bind, render, wait, copy_framebuffer,
 * GlesRenderer::with_context — so an empty thread costs one eglMakeCurrent and
 * nothing else.
 *
 * That is true of the *renderer* and not of a live *frame*, which is the one
 * thing in this design that carries a current context across calls.
 * GlesFrame::with_context is `Ok(func(&self.renderer.gl))` with no make_current
 * at all, and so are finish_internal and the frame's own destructor. So the
 * invariant is narrower than "an empty thread is harmless": no GPU-scene entry
 * point may run while a GlesFrame is alive. It is stated and enforced on the
 * Rust side, in qml::no_frame_in_flight, because that is where the frames are.
 *
 * The one compositor-side call that an empty thread is *not* enough for is our
 * own EGLFence::import, and that sits after an explicit restore in surface.rs,
 * where it is visible.
 */
static void release_the_thread(const SoliumQmlScene *scene)
{
    if (!scene_context_is_current(scene)) {
        return;
    }
    if (QOpenGLContext *context = QOpenGLContext::currentContext()) {
        context->doneCurrent();
    }
}

/*
 * Take the thread for this scene's own context, and leave Qt believing it.
 *
 * The inverse of release_the_thread, for the one entry point that has to issue
 * GL outside a frame: solium_qml_scene_rebind. Everywhere else the thread is
 * taken by Qt itself — initialize() on the way in, QRhiGles2::ensureContext()
 * inside beginFrame — and the only job left to this file was to give it back.
 *
 * Not a raw eglMakeCurrent, which is what "make this context current" would
 * otherwise mean here. That is precisely the move that creates the stale belief
 * clear_stale_current_context exists to undo: EGL would say Qt's context and
 * Qt's thread-local would say nothing, and import_dmabuf_texture needs a live
 * QOpenGLContext to reach Qt's own function table through. QOpenGLContext::
 * makeCurrent sets both halves, so afterwards the belief is true and
 * ensureContext() is right to trust it.
 *
 * Not QQuickRenderControl::initialize() either, which is how the *build* path
 * arrives here with a current context. It would be reusing a constructor for
 * its side effect: beyond bringing up an RHI that already exists, initialize()
 * re-runs the scene graph render context's own initialize() and re-emits
 * sceneGraphInitialized — the scene graph being stood up a second time under a
 * live item tree. This function exists so that a resize keeps everything above
 * the buffer, and re-initialising the scene graph is not keeping it. Whether a
 * second initialize() would even make anything current is Qt's business and
 * not a thing to depend on.
 *
 * Returns false when Qt never held the thread for this scene at all — a
 * software scene, or a GPU scene whose import failed — which is a caller error
 * rather than a state to recover from.
 */
static bool take_the_thread(const SoliumQmlScene *scene)
{
    if (scene->qt_context == nullptr || scene->qt_surface == nullptr) {
        return false;
    }
    return scene->qt_context->makeCurrent(scene->qt_surface);
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
    // As in the software constructor: registered first, so the error paths'
    // `solium_qml_scene_free` is the only place it has to be taken out.
    g_scenes.push_back(scene);
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
    // on this thread, which is what the import below needs. It is given back at
    // the bottom of this function — see release_the_thread — so a caller with no
    // renderer to restore is not left holding a thread Qt has taken.
    if (!scene->control->initialize()) {
        qWarning("QQuickRenderControl::initialize failed — Qt could not bring up "
                 "an RHI on this platform plugin, so the GPU path is not available");
        solium_qml_scene_free(scene);
        return nullptr;
    }

    if (!import_dmabuf_texture(scene, dmabuf_fd, stride, modifier, fourcc)) {
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
    mirror_for_the_compositor(&target);
    scene->window->setRenderTarget(target);

    const char *reason = nullptr;
    if (!load_component(scene, qml_path, initial_json, &reason)) {
        qWarning("solium_qml_scene_new_gpu: %s", reason != nullptr ? reason : "the QML did not load");
        // Not released first: the free path takes the thread back itself, and
        // finding Qt's own context current is the *good* case for it — the one
        // ordering in which the texture can be deleted explicitly.
        solium_qml_scene_free(scene);
        return nullptr;
    }
    // Last, after everything that needed Qt's context has had it.
    release_the_thread(scene);
    return scene;
}

extern "C" void solium_qml_scene_free(SoliumQmlScene *scene)
{
    if (scene == nullptr) {
        return;
    }

    // Out of the registry first, before anything below can leave a half-torn
    // scene in it: the next `solium_qml_tick` walks this list, and a scene whose
    // root has been deleted would be walked through a dangling pointer. Both
    // `_new` entry points register on the line after `new`, so a scene is in
    // here for exactly as long as it exists.
    g_scenes.erase(std::remove(g_scenes.begin(), g_scenes.end(), scene), g_scenes.end());

    // Before any Qt teardown runs, and for the same reason the render path does
    // it — but the stakes here are higher, because teardown *deletes*.
    //
    // `delete scene->window` and `delete scene->control` below reach
    // QRhiGles2::destroy() and the scenegraph invalidate, both of which go
    // through the same QRhiGles2::ensureContext(). It skips its makeCurrent when
    // Qt's stale thread-local says Qt's context is already current, which is
    // exactly what it says after the compositor has taken the thread back. Then
    // executeDeferredReleases() calls glDeleteTextures, glDeleteBuffers,
    // glDeleteFramebuffers and glDeleteProgram on Qt's own names — against the
    // compositor's context, where those integers name the compositor's objects.
    //
    // Which is the corruption the comment on the texture delete below describes,
    // arriving through a door that comment was not watching: it guards the one
    // delete this file makes by hand, and Qt makes a dozen more on its way out.
    //
    // Before this became the ordinary ordering the compositor's context was
    // never current across a GPU scene's lifetime, so it could not happen. It is
    // reachable on plain lifecycle now: a pane closing, a loading scene replaced
    // when its client attaches, a scripted instance dropped on reload.
    //
    // Measured: with this line removed and nothing else changed, freeing one
    // scene destroys the compositor's GL buffers 1 and 2 — smithay's vertex
    // buffers, which existed before Qt was started at all.
    clear_stale_current_context(scene);

    // The two GPU resources have very different requirements, and treating them
    // as one thing is what made the first version of this wrong.
    //
    // The EGLImage belongs to a *display*, not to a context: eglDestroyImageKHR
    // needs the right EGLDisplay and no current context at all. We recorded that
    // display at import, so this is always safe and always runs. It also has to
    // run: an EGLImage is not reclaimed when a context dies, so skipping it
    // leaks for the life of the process.
    if (scene->egl_image != EGL_NO_IMAGE_KHR && scene->egl_display != EGL_NO_DISPLAY) {
        static PFNEGLDESTROYIMAGEKHRPROC destroy_image =
            reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(
                eglGetProcAddress("eglDestroyImageKHR"));
        if (destroy_image != nullptr) {
            destroy_image(scene->egl_display, scene->egl_image);
        }
        scene->egl_image = EGL_NO_IMAGE_KHR;
    }

    // The texture belongs to a *context*, and a GL name is only meaningful
    // inside the one that issued it. Both Qt's context and the compositor's
    // number their textures from 1, so deleting name N against the wrong context
    // destroys whatever that context happens to have called N — a live client
    // surface, most likely. That is unrecoverable and silent, and it is strictly
    // worse than not deleting at all.
    //
    // So the test is scene_context_is_current: an EGL-level identity check
    // against what was recorded at import.
    // The earlier version asked QOpenGLContext::currentContext() != nullptr,
    // which cannot answer that — Qt's thread-local still points at Qt's context
    // after the compositor takes the thread back with a raw eglMakeCurrent, so
    // the guard passed and the delete ran against the compositor's context,
    // causing the exact corruption this comment describes.
    //
    // Note this is asked *after* clear_stale_current_context above, which is
    // what makes these the only three states, and which one you land in is a
    // property of the ordering rather than of anything going wrong:
    //
    //   * Qt's own context current. The free right after initialize(), or one
    //     that follows a render with nothing in between. Delete it here, which
    //     is the only case where an explicit delete is possible or needed.
    //
    //   * Nothing current. What most frees look like: after a render,
    //     clear_stale_current_context released the thread; after a build or a
    //     rebind, release_the_thread did, that being their postcondition.
    //
    //   * A third context current — in practice the compositor's — with Qt
    //     believing nothing is. Reached whenever some *other* scene's teardown
    //     cleared Qt's thread-local before this scene was freed, which two
    //     shell surfaces are enough to produce.
    //
    // None of the three is a failure and none of them says anything, because
    // the outcome is the same in all three: the name belongs to a context that
    // `delete scene->control` destroys a few lines below, and destroying a
    // context reclaims its objects. Nothing is leaked for the life of the
    // process in any of them.
    //
    // Both of the last two were warned about at some point, and both warnings
    // were wrong. The first asserted "another context on the thread" when the
    // thread was empty and fired on every free a session makes. The second was
    // written for the third state on the theory that it was a caller error, and
    // dev/wirecheck then produced it in a run where nothing was wrong: the
    // teardown is safe there precisely *because* Qt's thread-local is null, so
    // QRhiGles2::ensureContext() finds no current context and makes its own
    // properly. A null belief is the one thing this whole path can trust.
    if (scene->texture != 0) {
        if (scene_context_is_current(scene)) {
            QOpenGLContext *context = QOpenGLContext::currentContext();
            if (context != nullptr) {
                context->functions()->glDeleteTextures(1, &scene->texture);
                scene->texture = 0;
            }
        }
    }

    // Cached before the deletes, because the postcondition below has to be
    // established after `scene` is gone and cannot read it then.
    const bool was_gpu = scene->gpu;
    const EGLDisplay display = scene->egl_display;

    delete scene->root;
    delete scene->component;
    delete scene->window;
    delete scene->control;
    delete scene;

    // "A GPU scene freed leaves no GL context current" is stated as fact in
    // qml.rs, on `Scene::gpu`'s postcondition and in `Drop for Scene`, and the
    // whole shape of release_the_thread rests on it: building a scene and
    // freeing one are supposed to end the same way, so that a caller with no
    // renderer never has to care which happened.
    //
    // Up to here it was inherited rather than established. Qt's
    // ~QOpenGLContext does call doneCurrent when its context is the current
    // one, which is why it has held so far — but that is Qt's business, it is
    // conditional on a thread-local this file has spent three commits learning
    // not to trust, and a documented postcondition that is only true by
    // someone else's accident is not one.
    //
    // GPU scenes only. A software scene never had a context and the compositor
    // may legitimately have its own current across the free; releasing it there
    // would be this file reaching into a path it has no business in.
    if (was_gpu && eglGetCurrentContext() != EGL_NO_CONTEXT) {
        const EGLDisplay current = eglGetCurrentDisplay();
        eglMakeCurrent(current != EGL_NO_DISPLAY ? current : display, EGL_NO_SURFACE,
                       EGL_NO_SURFACE, EGL_NO_CONTEXT);
    }
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
            qWarning("solium_qml_scene_resize cannot change a GPU scene's pixel "
                     "size (%dx%d to %dx%d): it has no buffer to change it to. "
                     "Use solium_qml_scene_rebind with one.",
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
        mirror_for_the_compositor(&target);
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

/*
 * Point a GPU scene at a different buffer, and change nothing else.
 *
 * The scale-only branch of solium_qml_scene_resize above already does
 * everything a resize needs except swapping the texture underneath it. This is
 * that branch with a new buffer put under it, and it is a separate entry point
 * rather than a fall-through of resize because it takes a buffer and resize
 * cannot: the compositor allocates, and there is nothing in a
 * (width, height, scale) call for this function to allocate from.
 *
 * Why it is worth the entry point at all is in host.h: the alternative is
 * rebuilding the scene, and a rebuilt scene is a new object tree whose
 * animations all restart. A pane is sized from an animating rectangle, so that
 * happens once per frame for the length of every window animation, and an
 * animation restarted every frame never advances. This is a correctness fix
 * that happens to also be much cheaper.
 */
extern "C" bool solium_qml_scene_rebind(SoliumQmlScene *scene, int dmabuf_fd, int stride,
                                        unsigned long long modifier, unsigned int fourcc,
                                        int width, int height, double scale)
{
    if (scene == nullptr || !scene->gpu || scene->control == nullptr || width <= 0 ||
        height <= 0 || stride <= 0 || dmabuf_fd < 0) {
        qWarning("solium_qml_scene_rebind: not a GPU scene, or %dx%d stride=%d fd=%d is not a "
                 "buffer",
                 width, height, stride, dmabuf_fd);
        return false;
    }
    if (scale <= 0.0) {
        scale = 1.0;
    }

    // Everything below issues GL, so the thread has to be *honestly* Qt's
    // first. Nothing here can assume it already is: the compositor took it back
    // with a raw eglMakeCurrent after the last frame, and Qt's thread-local
    // still says otherwise — see clear_stale_current_context. Taking it
    // through QOpenGLContext rather than clearing the stale belief and hoping
    // something else takes it, because the import needs a live QOpenGLContext
    // and not merely a current EGL context.
    if (!take_the_thread(scene)) {
        qWarning("solium_qml_scene_rebind: could not make the scene's own GL context current");
        return false;
    }

    // Import first, release second. An import that fails leaves the scene whole
    // and still drawing, which is the difference between a dropped frame and a
    // black window.
    //
    // The size goes on the scene *before* the import and comes back off if it
    // fails, because import_dmabuf_texture sizes the EGLImage from
    // scene->width and scene->height. Setting them afterwards would import the
    // new buffer at the *old* size, and nothing is obliged to notice: an fd
    // carries no dimensions, so EGL_WIDTH and EGL_HEIGHT are the only thing
    // that says how many rows the image has.
    //
    // The display and context are saved for the same reason and it is not
    // symmetry: a successful import overwrites scene->egl_display and
    // scene->egl_context with the recording it just took, and both releases
    // below run after that. Asking the *scene* which context its texture is in
    // would then be comparing the old name against the new record — true by
    // construction, which is the shape of answer this file has been wrong with
    // three times. The old pair is what the old name belongs to.
    const EGLImageKHR previous_image = scene->egl_image;
    const GLuint previous_texture = scene->texture;
    const EGLDisplay previous_display = scene->egl_display;
    const EGLContext previous_context = scene->egl_context;
    const int previous_width = scene->width;
    const int previous_height = scene->height;
    scene->egl_image = EGL_NO_IMAGE_KHR;
    scene->texture = 0;
    scene->width = width;
    scene->height = height;

    if (!import_dmabuf_texture(scene, dmabuf_fd, stride, modifier, fourcc)) {
        scene->egl_image = previous_image;
        scene->texture = previous_texture;
        scene->width = previous_width;
        scene->height = previous_height;
        qWarning("solium_qml_scene_rebind: the new buffer would not import, "
                 "staying on the old one");
        release_the_thread(scene);
        return false;
    }

    // The old pair, now that there is a new one, and released against the
    // display and context *they* belong to rather than the ones just recorded.
    // The EGLImage belongs to a display and the texture to a context — the same
    // split solium_qml_scene_free explains at length, and for the same reason:
    // a GL name deleted against the wrong context destroys whatever that
    // context calls N.
    //
    // The answer is yes today whichever pair is asked about, because a scene's
    // context can only change when QQuickRenderControl is destroyed and that
    // takes the scene with it. Asked about the saved pair anyway: "which
    // context is this name in" is a question this file has one owner for, and
    // a call site that gets the right answer from the wrong record is exactly
    // how the third one went wrong.
    if (previous_image != EGL_NO_IMAGE_KHR && previous_display != EGL_NO_DISPLAY) {
        static PFNEGLDESTROYIMAGEKHRPROC destroy_image =
            reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(
                eglGetProcAddress("eglDestroyImageKHR"));
        if (destroy_image != nullptr) {
            destroy_image(previous_display, previous_image);
        }
    }
    if (previous_texture != 0 && context_is_current(previous_context, previous_display)) {
        QOpenGLContext *context = QOpenGLContext::currentContext();
        if (context != nullptr) {
            GLuint doomed = previous_texture;
            context->functions()->glDeleteTextures(1, &doomed);
        }
    }

    scene->scale = scale;

    // Logical geometry, device-sized target, ratio between them — identical to
    // the scale-only branch of solium_qml_scene_resize, and see the long
    // comment there for why it is that way round.
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
    mirror_for_the_compositor(&target);
    scene->window->setRenderTarget(target);
    scene->dirty = true;

    // Nothing current on the way out, which is what building a GPU scene and
    // freeing one both promise. The three entry points a caller with no
    // renderer can reach now end the same way, so ShellSurface never has to
    // know which of them it just called — only render_gpu leaves Qt's context
    // behind, and it says so.
    release_the_thread(scene);
    return true;
}

static bool animation_running(const QObject *item);

/* Is anything at all in this process animating?
 *
 * The per-scene question `solium_qml_scene_animating` answers, asked of every
 * scene, because the animation clock is per process. Short-circuits on the
 * first scene that says yes, which is the case that matters: when something is
 * animating this stops at the first window, and when nothing is every scene is
 * settled and each walk is the cheap one -- a settled reactive.qml is 21
 * QObjects and 0.6us.
 *
 * It is the same walk, and therefore the same trust, as the one the render loop
 * already gates on. If it were ever wrong the compositor would already have
 * stopped drawing that scene, so nothing here can be stranded that was not
 * stranded already.
 *
 * A `Timer` is not counted, for the same reason it is not counted below, and
 * the answer does not change what one does: a Timer is not driven by the
 * animation driver at all -- it fires out of `processEvents`, off Qt's own
 * event loop -- so pinning this clock cannot slow one down. Measured: a 100ms
 * repeating Timer fired 9 times over 960ms of ticked frames, identically with
 * the clock pinned and unpinned. The gap a running Timer *does* have is
 * unchanged and is described on `solium_qml_scene_animating`. */
static bool anything_animating()
{
    return std::any_of(g_scenes.begin(), g_scenes.end(), [](const SoliumQmlScene *scene) {
        return scene != nullptr && scene->root != nullptr && animation_running(scene->root);
    });
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
     * a 260Hz monitor is a quarter of the work for the same behaviour.
     *
     * Drained *before* the advance, and that order is measured rather than
     * incidental. Starting a QML animation does not register it on the spot:
     * `QAnimationTimer::registerAnimation` queues `startAnimations` through the
     * event loop (qtbase v6.11.2, qabstractanimation.cpp:659-663), so an
     * animation a property write started last frame joins the running set only
     * when this queue is next drained. Drained after the advance, the advance
     * that follows the write skips it and the first step it takes covers three
     * frames at once; drained before, it takes the next step with everything
     * else. Measured on reveal.qml's 260ms appear animation, from a settled
     * scene: first visible step 33ms in rather than 49ms in, one frame earlier,
     * and one more frame of the animation actually drawn.
     *
     * The 16ms throttle stays. Dropping it -- draining every frame -- was
     * measured against the same animation and changed nothing at 60Hz: the
     * queue is drained once per frame either way there, and the throttle only
     * ever bites on a screen faster than 60Hz, where it is the whole point. */
    if (g_app != nullptr) {
        static long long drained_at = 0;
        if (elapsed_ms - drained_at >= 16 || elapsed_ms < drained_at) {
            drained_at = elapsed_ms;
            QCoreApplication::processEvents();
        }
    }
    if (g_driver != nullptr) {
        /* Asked once for the whole process, and asked *here* rather than
         * inside the driver, because the driver has no way to reach the
         * scenes. See `CompositorAnimationDriver` for what the answer is for. */
        g_driver->advanceTo(static_cast<qint64>(elapsed_ms), anything_animating());
    }
}

/* Whether Qt has asked for this scene to be drawn again. */
extern "C" int solium_qml_scene_dirty(const SoliumQmlScene *scene)
{
    return (scene != nullptr && scene->dirty) ? 1 : 0;
}

/*
 * Is any animation inside this scene still running?
 *
 * A different question from `dirty`, and the compositor needs both. `dirty`
 * means "Qt has something new to draw", which is raised only when a property
 * that is actually rendered changes value. A running animation does not change
 * one every tick: the tick that *starts* a `Behavior` has not moved the
 * property yet, and an interpolation between two nearby values spends several
 * ticks rounding to the number it already had. On those ticks the scene is
 * clean while the animation is very much alive, and a compositor that reads
 * `dirty` as "still animating" stops drawing and never advances it again.
 *
 * Measured against this Qt (6.11.2, software adaptation, the same driver and
 * the same renderRequested/sceneChanged wiring as below), from a settled scene,
 * on the frame the compositor writes the property that triggers the animation
 * and the three ticks after it -- dirty, and whether an animation is running:
 *
 *   reveal.qml     pointerInside  false/yes  false/yes  true/yes  true/yes
 *   reactive.qml   pointerInside  false/yes  true/yes   true/yes  true/yes
 *   top.qml        focused        true/yes   false/yes  true/yes  true/yes
 *   border.qml     focused        true/yes   false/yes  true/yes  true/yes
 *   proximity.qml  pointerInside  true/yes   true/yes   true/yes  true/yes
 *
 * Every one of them has at least one clean tick before it has finished, and
 * `reveal` and `reactive` are clean on the very frame that starts them, so on
 * the dirty flag alone their animation never took a single step unless
 * something unrelated damaged the screen.
 *
 * That table used to end `true/no` on every row, and the `no` was the whole of
 * the third defect in this area rather than a healthy animation ending. None of
 * these animations is shorter than 100ms and `reveal`'s is 260ms; not one of
 * them can be over three ticks -- 48ms -- after it started. They read `no`
 * because they had already been advanced past their own end in a single step,
 * by an animation clock that handed a newly registered animation the
 * compositor's entire uptime. See `CompositorAnimationDriver`. The rows above
 * are the same five decorations re-measured with that fixed.
 *
 * Which is also the warning: this function and `dirty` together cannot tell an
 * animation that is *playing* from one that finished in one tick. Both say
 * exactly what they should in both cases. Whether an animation is advancing at
 * the right rate is not a question either of them is asked -- dev/wirecheck's
 * appear case is what asks it.
 *
 * Asked of the scene and not of the process. `QAnimationDriver::isRunning()`
 * looks like the direct answer and is not one: `QAnimationDriver::advanceAnimation`
 * ends in `QUnifiedTimer::restart` -> `localRestart`, which starts the driver
 * again whenever it is not running, *including when no animation is registered*
 * (qtbase v6.11.2, src/corelib/animation/qabstractanimation.cpp:333-349). Qt's
 * own stop is queued, so the driver reads stopped for exactly one tick and
 * running for every tick after it, for ever. Measured here: with the screen
 * being damaged for eight frames after the animation starts -- a pointer still
 * moving, which is usually *why* it started -- a compositor gated on
 * `isRunning()` drew 400 frames of 400 and never went idle again.
 *
 * `running` on a QQuickAbstractAnimation is exact instead, and per scene.
 * Reached through QObject::inherits so this needs no Qt private headers; the
 * walk short-circuits, and the compositor only asks when the scene is clean,
 * which is the only frame the answer can change anything. A settled
 * reactive.qml is 21 QObjects and 0.6 us.
 *
 * `anything_animating` asks the same question of every scene at once, for the
 * animation *clock* rather than for the frame -- see `solium_qml_tick`. Same
 * walk, same trust: a scene this is wrong about has already stopped being
 * drawn.
 *
 * What it does not cover: a `Timer`. A scene whose next change is a timer
 * firing -- Quickshell.SystemClock is the one in the tree -- is not animating
 * by this answer and will not be given the frame its timer needs to fire on,
 * because `solium_qml_tick` is what drains the event queue and only runs on a
 * frame that is drawn. Counting running Timers here would fix that and would
 * also pin the compositor at full rate for as long as any clock exists, which
 * is the wrong trade; what that wants is a wake-up deadline, not a busy loop.
 */
static bool animation_running(const QObject *item)
{
    if (item->inherits("QQuickAbstractAnimation") && item->property("running").toBool()) {
        return true;
    }
    for (const QObject *child : item->children()) {
        if (animation_running(child)) {
            return true;
        }
    }
    return false;
}

extern "C" int solium_qml_scene_animating(const SoliumQmlScene *scene)
{
    if (scene == nullptr || scene->root == nullptr) {
        return 0;
    }
    return animation_running(scene->root) ? 1 : 0;
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

    // Before anything that touches the RHI, and above all before beginFrame,
    // which is where Qt decides whether it needs its context back.
    clear_stale_current_context(scene);

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
