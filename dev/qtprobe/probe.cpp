// Does this Qt adopt a foreign EGL context?
//
// The whole GPU path for in-compositor QML turns on this one call. If it
// returns a context, Qt can render into a texture the compositor samples
// directly. If it returns null, Qt renders on a context of its own and the
// only way across is a shared buffer.
#include <QtGui/QGuiApplication>
#include <QtGui/QOpenGLContext>
#include <QtGui/QOffscreenSurface>
#include <EGL/egl.h>
#include <cstdio>

int main(int argc, char **argv)
{
    QGuiApplication app(argc, argv);
    printf("platform: %s\n", qPrintable(app.platformName()));

    EGLDisplay display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (display == EGL_NO_DISPLAY) { printf("no EGL display\n"); return 2; }
    EGLint major = 0, minor = 0;
    if (!eglInitialize(display, &major, &minor)) { printf("eglInitialize failed\n"); return 2; }
    printf("EGL %d.%d\n", major, minor);

    eglBindAPI(EGL_OPENGL_ES_API);
    // No window system here, so no window-capable config exists: pbuffer is
    // what a surfaceless context can be made against.
    const EGLint config_attrs[] = {
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_NONE
    };
    EGLConfig config{};
    EGLint found = 0;
    if (!eglChooseConfig(display, config_attrs, &config, 1, &found) || found == 0) {
        printf("no EGL config\n"); return 2;
    }
    const EGLint ctx_attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext context = eglCreateContext(display, config, EGL_NO_CONTEXT, ctx_attrs);
    if (context == EGL_NO_CONTEXT) { printf("eglCreateContext failed\n"); return 2; }
    printf("made an EGL context to offer Qt\n");

#if QT_CONFIG(egl)
    QOpenGLContext *adopted =
        QNativeInterface::QEGLContext::fromNative(context, display);
    printf("fromNative: %s\n", adopted == nullptr ? "NULL — cannot adopt" : "adopted");
    if (adopted != nullptr) {
        QOffscreenSurface surface;
        surface.create();
        printf("makeCurrent on the adopted context: %s\n",
               adopted->makeCurrent(&surface) ? "yes" : "no");
    }
#else
    printf("this Qt was built without EGL support\n");
#endif
    return 0;
}
