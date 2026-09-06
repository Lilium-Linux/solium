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
 * Start Qt. Must be called once, before any scene, and from the thread that
 * will render. Returns 0 on failure.
 *
 * `import_path` is added to the QML import path, so every scene can reach the
 * shared design system with `import Solium`. One engine serves all scenes, so
 * that theme is a single object rather than a copy per surface.
 */
int solium_qml_start(const char *import_path);

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

void solium_qml_scene_free(SoliumQmlScene *scene);

void solium_qml_scene_resize(SoliumQmlScene *scene, int width, int height);

/*
 * Advance QML animations to `elapsed_ms`.
 *
 * Driven by the compositor's clock rather than Qt's own timer: there is one
 * clock in this compositor, and a QML animation running off a second one would
 * drift against every transform around it.
 */
void solium_qml_scene_advance(SoliumQmlScene *scene, long long elapsed_ms);

/* Returned by a render that was skipped because nothing had changed. */
#define SOLIUM_QML_UNCHANGED 2

/*
 * Render the scene if it has changed.
 *
 * Returns 1 when it rendered, SOLIUM_QML_UNCHANGED when the scene was already
 * up to date (the previous pixels are still valid), 0 on failure.
 */
int solium_qml_scene_render(SoliumQmlScene *scene);

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

/* Read a bool property. Non-clearing: this is state QML owns and we observe. */
int solium_qml_scene_get_bool(const SoliumQmlScene *scene, const char *name);

/* Pointer input, in scene coordinates. `pressed`: 1 down, 0 up, -1 motion. */
void solium_qml_scene_pointer(SoliumQmlScene *scene, double x, double y, int pressed);

#ifdef __cplusplus
}
#endif

#endif /* SOLIUM_QML_HOST_H */
