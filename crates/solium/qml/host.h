/*
 * Hosting a Qt Quick scene inside the compositor.
 *
 * A C ABI on purpose: it is the whole surface Rust has to reason about, and it
 * is small enough to read in one sitting. Everything Qt is behind it.
 */

#ifndef SOLIUM_QML_HOST_H
#define SOLIUM_QML_HOST_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SoliumQmlScene SoliumQmlScene;

/*
 * Qt's diagnostics, on their way *out* of C++.
 *
 * The one declaration in this header that points the other way: the host calls
 * this, and whoever links the host defines it. `crates/solium/src/qml.rs` does,
 * with one `tracing` macro per level; `dev/wirecheck` has its own, because it
 * compiles host.cpp without the compositor crate behind it.
 *
 * It is installed as Qt's message handler before QGuiApplication exists, so QML
 * binding errors, `console.log`/`console.warn`, and Qt's own warnings all
 * arrive here rather than at Qt's default handler. That handler is not worth
 * chaining to, and not merely because everything would then be said twice:
 * measured on this Fedora Qt 6.11, it writes to stderr when stderr is a
 * console and to journald when it is not. On a TTY session stderr *is* a
 * console — the VT the compositor is about to take — so every one of these
 * messages was printed underneath the desktop and reached no log at all.
 *
 * The levels below are ours rather than Qt's `QtMsgType`, whose numbering is
 * Qt's to change and does not run in severity order.
 *
 * `category` is Qt's logging category ("default", "qml", "qt.qpa.…"). `file`
 * and `function` are null and `line` is 0 in a release Qt build, so all three
 * are optional. None of the pointers outlive the call — a QMessageLogContext's
 * strings are only valid for the duration of the handler — so an
 * implementation that keeps anything has to copy it.
 *
 * It may be called from any thread: that is `qInstallMessageHandler`'s
 * contract, whatever this compositor happens to do today.
 */
#define SOLIUM_QML_LOG_DEBUG 0
#define SOLIUM_QML_LOG_INFO 1
#define SOLIUM_QML_LOG_WARN 2
#define SOLIUM_QML_LOG_ERROR 3

void solium_qml_log_from_qt(int level, const char *category, const char *message,
                            const char *file, int line, const char *function);

/*
 * Start Qt. Must be called once, before any scene, and from the thread that
 * will render. Returns 0 on failure.
 *
 * `import_path` is added to the QML import path, so every scene can reach the
 * shared design system with `import Solium`. One engine serves all scenes, so
 * that theme is a single object rather than a copy per surface.
 */
int solium_qml_start(const char *import_path);

/* Start the host on the RHI (OpenGL) scene graph rather than the software one.
 *
 * Returns 1 when Qt came up on the GPU, 0 when it did not — in which case the
 * caller must fall back to solium_qml_start(). Only one of the two may be
 * called in a process: the scene graph backend is chosen once. Calling the
 * other one afterwards returns 0 rather than quietly handing back a host on
 * the wrong backend.
 *
 * Two limits on that contract, both of which the caller has to plan around
 * rather than discover:
 *
 * 1. It cannot always return. This selects the QPA platform plugin (eglfs), and
 *    the plugin is loaded inside QGuiApplication's constructor. When a named
 *    plugin cannot initialise — not built into this Qt, or eglfs unable to open
 *    a DRM device — Qt calls qFatal and the *process aborts*. Verified here:
 *    given a plugin name that does not exist, this call dies with SIGABRT
 *    rather than returning 0. So on a machine where the GPU path is unavailable
 *    in that particular way there is no fallback, there is a crash. A caller
 *    that needs the software path to stay reachable has to decide whether the
 *    GPU path is viable *before* calling this; the return value is too late.
 *
 * 2. Returning 1 does not mean the path works. It means Qt came up on an RHI
 *    backend. Everything the GPU path actually depends on — the render control
 *    initialising, the dmabuf importing — is per scene, and is reported by
 *    solium_qml_scene_new_gpu returning NULL. There is no way back at that
 *    point: Qt fixes its scene graph backend for the life of the process, so
 *    solium_qml_start() will then correctly refuse and the caller is left with
 *    a GPU host that cannot build GPU scenes. Treat NULL from the *first*
 *    scene_new_gpu as fatal to the GPU path, not as a per-scene error.
 */
int solium_qml_start_gpu(const char *import_path);

/* A scene that renders into a buffer we allocated.
 *
 * `dmabuf_fd` is borrowed for the call — EGL takes its own reference on the
 * buffer during the import, so the caller may close its fd as soon as this
 * returns. `modifier` is the DRM format modifier, `fourcc` the DRM fourcc.
 *
 * Returns NULL on failure, having written the reason to the warning log. There
 * is no `error` out-parameter, unlike the software constructor: every way this
 * can fail is a property of the driver or of Qt rather than of the QML, so the
 * useful detail is an EGL or GL error code and not a component error string.
 */
SoliumQmlScene *solium_qml_scene_new_gpu(const char *qml_path, int width, int height,
                                         int dmabuf_fd, int stride,
                                         unsigned long long modifier,
                                         unsigned int fourcc,
                                         const char *initial_json);

/* Render, and hand back a fence that signals when the GPU is done.
 *
 * `*fence_fd` is set to -1 when the driver gave no fence, which the caller must
 * treat as "finished" only after glFinish. Ownership passes to the caller.
 *
 * Returns 1 when it rendered, SOLIUM_QML_UNCHANGED when the scene was already
 * up to date, 0 on failure.
 */
int solium_qml_scene_render_gpu(SoliumQmlScene *scene, int *fence_fd);

/* Point an existing GPU scene at a different buffer.
 *
 * Everything above the buffer — the render control, the RHI, the QML object
 * tree and its animation state — is kept. Only the EGLImage and the texture
 * are replaced, which is the whole of what a dmabuf's fixed size forces.
 *
 * That is not a performance note. A rebuilt scene is a *new object tree*, so
 * every animation, transition and stored property in it restarts from zero —
 * and a pane's scene is sized from an animating rectangle, so it is resized on
 * every frame of every window animation. A scene rebuilt per resize does not
 * animate slowly; it never advances.
 *
 * `width` and `height` are device pixels and must match the new buffer;
 * `scale` is how many of those make a logical one, as in
 * solium_qml_scene_resize. `dmabuf_fd` is borrowed for the call, exactly as it
 * is by solium_qml_scene_new_gpu.
 *
 * Returns false and leaves the scene on its previous buffer on failure, so a
 * surface whose resize failed keeps drawing last frame's picture.
 *
 * Leaves *no* GL context current on this thread — the same postcondition as
 * solium_qml_scene_new_gpu and solium_qml_scene_free, so that a caller with no
 * renderer to restore never has to know which of the three it just called. */
bool solium_qml_scene_rebind(SoliumQmlScene *scene, int dmabuf_fd, int stride,
                             unsigned long long modifier, unsigned int fourcc,
                             int width, int height, double scale);

/*
 * Load `qml_path` into a scene rendering at `width` x `height`.
 *
 * Returns NULL on failure, with `error` set to a description.
 */
/* Like solium_qml_scene_new, but supplies properties the component requires
 * before it is built. `initial_json` is a JSON object, or null. */
/* Forget compiled QML, so the next scene is read from disk. */
void solium_qml_clear_cache(void);

SoliumQmlScene *solium_qml_scene_new_with(const char *qml_path, int width, int height,
                                          const char *initial_json, const char **error);

SoliumQmlScene *solium_qml_scene_new(const char *qml_path, int width, int height,
                                     const char **error);

/* Destroy a scene.
 *
 * For a scene from solium_qml_scene_new_gpu, call this with that scene's own GL
 * context current — the one Qt made current inside solium_qml_scene_new_gpu,
 * not the compositor's. The texture lives in Qt's context and a GL name only
 * means anything inside the context that issued it; both contexts number their
 * textures from 1, so deleting against the wrong one would destroy an unrelated
 * object. The host checks (at the EGL level, against what it recorded at
 * import) and skips the delete rather than risk that, so getting this wrong is
 * a warning and a slightly later reclaim, not corruption. The EGLImage is
 * released either way: it belongs to a display, not a context.
 *
 * Either way, this leaves *no* context current: Qt makes its own current to
 * tear down its RHI and then releases it, whatever was current on the way in.
 * Measured — eglGetCurrentContext() is NULL on return. So the compositor has to
 * make its own context current again after freeing a GPU scene, exactly as it
 * does after rendering one. */
void solium_qml_scene_free(SoliumQmlScene *scene);

/* Resize a scene. `width` and `height` are *device* pixels — the image the
 * compositor uploads — and `scale` is how many of those make a logical one, so
 * the scene is laid out in width/scale by height/scale and rasterised at the
 * full size. See the comment on the definition: getting this the wrong way
 * round gives either half-size text or blurry chrome.
 *
 * On a GPU scene only `scale` may change here: the pixel size is the buffer's,
 * and this function has no buffer to change it to. Pass one to
 * solium_qml_scene_rebind instead. */
void solium_qml_scene_resize(SoliumQmlScene *scene, int width, int height, double scale);

/*
 * Advance QML animations to `elapsed_ms`.
 *
 * Driven by the compositor's clock rather than Qt's own timer: there is one
 * clock in this compositor, and a QML animation running off a second one would
 * drift against every transform around it.
 */
void solium_qml_tick(long long elapsed_ms);

/* Returned by a render that was skipped because nothing had changed. */
#define SOLIUM_QML_UNCHANGED 2

/*
 * Render the scene if it has changed.
 *
 * Returns 1 when it rendered, SOLIUM_QML_UNCHANGED when the scene was already
 * up to date (the previous pixels are still valid), 0 on failure.
 */
int solium_qml_scene_render(SoliumQmlScene *scene);
int solium_qml_scene_dirty(const SoliumQmlScene *scene);

/*
 * The pixels of the last render: premultiplied ARGB32, `*stride` bytes per row.
 *
 * Owned by the scene and valid until the next render or resize.
 */
const unsigned char *solium_qml_scene_pixels(const SoliumQmlScene *scene, int *stride);

/*
 * Read a string property and clear it, returning NULL when it was empty.
 *
 * How QML reports back: a button sets `action`, the compositor takes it. One
 * direction, one owner — a property the compositor also wrote would be two
 * authorities over one piece of state.
 *
 * The returned pointer is owned by the scene and valid until the next call.
 */
const char *solium_qml_scene_take_string(SoliumQmlScene *scene, const char *name);

void solium_qml_scene_set_string(SoliumQmlScene *scene, const char *name, const char *value);
void solium_qml_scene_set_bool(SoliumQmlScene *scene, const char *name, int value);
void solium_qml_scene_set_int(SoliumQmlScene *scene, const char *name, int value);
int solium_qml_scene_get_int(const SoliumQmlScene *scene, const char *name);

/* Read a bool property. Non-clearing: this is state QML owns and we observe. */
int solium_qml_scene_get_bool(const SoliumQmlScene *scene, const char *name);

/* Pointer input, in scene coordinates. `pressed`: 1 down, 0 up, -1 motion. */
void solium_qml_scene_pointer(SoliumQmlScene *scene, double x, double y, int pressed);

#ifdef __cplusplus
}
#endif

#endif /* SOLIUM_QML_HOST_H */
