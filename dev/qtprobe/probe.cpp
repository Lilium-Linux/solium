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
#include <cstring>

// Second question, since the first is no (below, and in the spike this probe
// backs): can Qt at least render into a dmabuf we allocate and hand back, so
// the two contexts share a buffer instead of a context? This allocates a GBM
// buffer, wraps it as an EGLImage, and binds it to a texture the way a
// dmabuf-backed render target would. If this fails, the GPU path is closed
// on this machine, full stop — the fallback is the software scene graph the
// spike already documents, not a workaround here.
#include <gbm.h>
#include <fcntl.h>
#include <unistd.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>

static bool probe_dmabuf(EGLDisplay display)
{
    // eglCreateImageKHR and glEGLImageTargetTexture2DOES are extension entry
    // points, not core EGL/GLES2 symbols. This system's libEGL.so exports
    // only eglCreateImage (the core EGL 1.5 function, no KHR suffix) and
    // libGLESv2.so exports nothing for the OES import at all — confirmed
    // with `nm -D` — so linking the names directly fails at link time.
    // eglGetProcAddress is the only portable way to reach an extension
    // entry point; direct linkage to an EXT/KHR/OES name is never
    // guaranteed, on any vendor's drivers.
    auto eglCreateImageKHR_ = reinterpret_cast<PFNEGLCREATEIMAGEKHRPROC>(
        eglGetProcAddress("eglCreateImageKHR"));
    auto glEGLImageTargetTexture2DOES_ = reinterpret_cast<PFNGLEGLIMAGETARGETTEXTURE2DOESPROC>(
        eglGetProcAddress("glEGLImageTargetTexture2DOES"));
    if (!eglCreateImageKHR_ || !glEGLImageTargetTexture2DOES_) {
        printf("dmabuf: extension entry points missing (eglCreateImageKHR: %s, "
               "glEGLImageTargetTexture2DOES: %s)\n",
               eglCreateImageKHR_ ? "found" : "missing",
               glEGLImageTargetTexture2DOES_ ? "found" : "missing");
        return false;
    }

    int drm = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
    if (drm < 0) { printf("dmabuf: no render node\n"); return false; }

    gbm_device *gbm = gbm_create_device(drm);
    if (!gbm) { printf("dmabuf: no gbm device\n"); close(drm); return false; }

    gbm_bo *bo = gbm_bo_create(gbm, 256, 256, GBM_FORMAT_ARGB8888,
                               GBM_BO_USE_RENDERING);
    if (!bo) { printf("dmabuf: gbm_bo_create failed\n"); return false; }

    int fd = gbm_bo_get_fd(bo);
    const int stride = static_cast<int>(gbm_bo_get_stride(bo));
    const uint64_t modifier = gbm_bo_get_modifier(bo);

    EGLint attribs[] = {
        EGL_WIDTH, 256, EGL_HEIGHT, 256,
        EGL_LINUX_DRM_FOURCC_EXT, GBM_FORMAT_ARGB8888,
        EGL_DMA_BUF_PLANE0_FD_EXT, fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, stride,
        EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, static_cast<EGLint>(modifier & 0xffffffff),
        EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, static_cast<EGLint>(modifier >> 32),
        EGL_NONE,
    };
    EGLImageKHR image = eglCreateImageKHR_(display, EGL_NO_CONTEXT,
                                          EGL_LINUX_DMA_BUF_EXT, nullptr, attribs);
    if (image == EGL_NO_IMAGE_KHR) {
        printf("dmabuf: eglCreateImageKHR failed 0x%x  (format=ARGB8888 stride=%d modifier=0x%llx)\n",
               eglGetError(), stride, static_cast<unsigned long long>(modifier));
        return false;
    }

    GLuint texture = 0;
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    glEGLImageTargetTexture2DOES_(GL_TEXTURE_2D, image);
    const GLenum error = glGetError();
    if (error != GL_NO_ERROR) {
        printf("dmabuf: glEGLImageTargetTexture2DOES failed 0x%x\n", error);
        return false;
    }

    printf("dmabuf: ok  texture=%u stride=%d modifier=0x%llx\n",
           texture, stride, static_cast<unsigned long long>(modifier));
    return true;
}

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

    const char *extensions = eglQueryString(display, EGL_EXTENSIONS);
    const bool has_fence_sync = extensions != nullptr &&
        strstr(extensions, "EGL_ANDROID_native_fence_sync") != nullptr;
    printf("EGL_ANDROID_native_fence_sync: %s\n",
           has_fence_sync ? "present" : "absent");

    // probe_dmabuf does real GL work (glGenTextures, glBindTexture,
    // glEGLImageTargetTexture2DOES), which needs a context current on this
    // thread. Nothing above makes "context" current — the adoption branch
    // above is the only thing that would have, and it fails by design on
    // this Qt (that is the whole point of this file). Make it current
    // ourselves on a pbuffer surface; "config" was already chosen with
    // EGL_SURFACE_TYPE, EGL_PBUFFER_BIT for exactly this reason.
    const EGLint pbuffer_attrs[] = { EGL_WIDTH, 256, EGL_HEIGHT, 256, EGL_NONE };
    EGLSurface pbuffer = eglCreatePbufferSurface(display, config, pbuffer_attrs);
    if (pbuffer == EGL_NO_SURFACE) {
        printf("dmabuf: eglCreatePbufferSurface failed 0x%x\n", eglGetError());
        return 3;
    }
    if (!eglMakeCurrent(display, pbuffer, pbuffer, context)) {
        printf("dmabuf: eglMakeCurrent failed 0x%x\n", eglGetError());
        return 3;
    }

    const bool dmabuf_ok = probe_dmabuf(display);
    return dmabuf_ok ? 0 : 4;
}
