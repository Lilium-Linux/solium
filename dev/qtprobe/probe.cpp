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

// Imports the dmabuf described by fd/stride/modifier as an EGLImage on
// `display` and binds it to a texture, printing exactly what happened.
// `display` must already have a context current on this thread — the
// caller's job, because the two callers below make current on two very
// different displays.
//
// `label` tags every line because this is called twice, against two
// different displays, and "eglCreateImageKHR failed" on its own would not
// say which attempt it was.
//
// Checks both EGL_EXT_image_dma_buf_import (the base import) and
// EGL_EXT_image_dma_buf_import_modifiers (required separately for the
// EGL_DMA_BUF_PLANE0_MODIFIER_*_EXT attribs below) before attempting the
// import, and prints both either way — "the extension is absent" and "the
// import was refused" are different answers, and only printing on failure
// would leave them looking the same.
static bool try_import(const char *label, EGLDisplay display, int fd, int stride, uint64_t modifier,
                        PFNEGLCREATEIMAGEKHRPROC create_image,
                        PFNGLEGLIMAGETARGETTEXTURE2DOESPROC target_texture)
{
    const char *extensions = eglQueryString(display, EGL_EXTENSIONS);
    const bool has_import = extensions != nullptr &&
        strstr(extensions, "EGL_EXT_image_dma_buf_import") != nullptr;
    const bool has_modifiers = extensions != nullptr &&
        strstr(extensions, "EGL_EXT_image_dma_buf_import_modifiers") != nullptr;
    printf("dmabuf[%s]: EGL_EXT_image_dma_buf_import: %s, _modifiers: %s\n",
           label, has_import ? "present" : "absent", has_modifiers ? "present" : "absent");

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
    EGLImageKHR image = create_image(display, EGL_NO_CONTEXT,
                                      EGL_LINUX_DMA_BUF_EXT, nullptr, attribs);
    if (image == EGL_NO_IMAGE_KHR) {
        printf("dmabuf[%s]: eglCreateImageKHR failed 0x%x  (format=ARGB8888 stride=%d modifier=0x%llx)\n",
               label, eglGetError(), stride, static_cast<unsigned long long>(modifier));
        return false;
    }

    GLuint texture = 0;
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    target_texture(GL_TEXTURE_2D, image);
    const GLenum error = glGetError();
    if (error != GL_NO_ERROR) {
        printf("dmabuf[%s]: glEGLImageTargetTexture2DOES failed 0x%x\n", label, error);
        return false;
    }

    printf("dmabuf[%s]: ok  texture=%u stride=%d modifier=0x%llx\n",
           label, texture, stride, static_cast<unsigned long long>(modifier));
    return true;
}

// Solium's own EGLDisplay is not eglGetDisplay(EGL_DEFAULT_DISPLAY). See
// tty.rs's open_gpu: `EGLDisplay::new(gbm.clone())` builds a GBM-platform
// display on the exact GBM device object the render buffers come from. That
// pairing — buffer and display on the same device — is what the plan
// depends on, so this reproduces it: a platform display obtained from
// `gbm`, the same device the buffer above was allocated on, and the import
// attempted against that display instead of the default one.
static bool try_gbm_paired_import(gbm_device *gbm, int fd, int stride, uint64_t modifier,
                                   PFNEGLCREATEIMAGEKHRPROC create_image,
                                   PFNGLEGLIMAGETARGETTEXTURE2DOESPROC target_texture)
{
    auto get_platform_display = reinterpret_cast<PFNEGLGETPLATFORMDISPLAYEXTPROC>(
        eglGetProcAddress("eglGetPlatformDisplayEXT"));
    if (!get_platform_display) {
        printf("dmabuf[gbm-paired display]: no eglGetPlatformDisplayEXT\n");
        return false;
    }
    EGLDisplay gbm_display = get_platform_display(EGL_PLATFORM_GBM_KHR, gbm, nullptr);
    if (gbm_display == EGL_NO_DISPLAY) {
        printf("dmabuf[gbm-paired display]: eglGetPlatformDisplayEXT failed 0x%x\n", eglGetError());
        return false;
    }
    EGLint major = 0, minor = 0;
    if (!eglInitialize(gbm_display, &major, &minor)) {
        printf("dmabuf[gbm-paired display]: eglInitialize failed 0x%x\n", eglGetError());
        return false;
    }
    printf("dmabuf[gbm-paired display]: EGL %d.%d\n", major, minor);

    // Same recipe as main()'s default-display setup below, on this display
    // instead: a pbuffer-capable ES2 config, a context, a pbuffer surface to
    // make it current on, because there is no window here either.
    eglBindAPI(EGL_OPENGL_ES_API);
    const EGLint config_attrs[] = {
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_NONE
    };
    EGLConfig config{};
    EGLint found = 0;
    if (!eglChooseConfig(gbm_display, config_attrs, &config, 1, &found) || found == 0) {
        printf("dmabuf[gbm-paired display]: no EGL config\n");
        return false;
    }
    const EGLint ctx_attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext context = eglCreateContext(gbm_display, config, EGL_NO_CONTEXT, ctx_attrs);
    if (context == EGL_NO_CONTEXT) {
        printf("dmabuf[gbm-paired display]: eglCreateContext failed 0x%x\n", eglGetError());
        return false;
    }
    const EGLint pbuffer_attrs[] = { EGL_WIDTH, 256, EGL_HEIGHT, 256, EGL_NONE };
    EGLSurface pbuffer = eglCreatePbufferSurface(gbm_display, config, pbuffer_attrs);
    if (pbuffer == EGL_NO_SURFACE) {
        printf("dmabuf[gbm-paired display]: eglCreatePbufferSurface failed 0x%x\n", eglGetError());
        return false;
    }
    if (!eglMakeCurrent(gbm_display, pbuffer, pbuffer, context)) {
        printf("dmabuf[gbm-paired display]: eglMakeCurrent failed 0x%x\n", eglGetError());
        return false;
    }

    return try_import("gbm-paired display", gbm_display, fd, stride, modifier,
                       create_image, target_texture);
}

static bool probe_dmabuf(EGLDisplay display)
{
    // eglCreateImageKHR and glEGLImageTargetTexture2DOES are extension entry
    // points, not core EGL/GLES2 symbols. This system's libEGL.so exports
    // only eglCreateImage (the core EGL 1.5 function, no KHR suffix) and
    // libGLESv2.so exports nothing for the OES import at all — confirmed
    // with `nm -D` — so linking the names directly fails at link time.
    // eglGetProcAddress is the only portable way to reach an extension
    // entry point; direct linkage to an EXT/KHR/OES name is never
    // guaranteed, on any vendor's drivers. Resolved once, used against
    // both displays below — the entry points do not depend on which
    // display they end up called with.
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

    // Attempt 1: whatever device eglGetDisplay(EGL_DEFAULT_DISPLAY) happened
    // to hand back. This is what the first version of this probe measured —
    // kept so the contrast is visible, not because it answers the plan's
    // question by itself. `display` already has a context current on it,
    // made current by main() before calling this function.
    const bool default_ok = try_import("default display", display, fd, stride, modifier,
                                        eglCreateImageKHR_, glEGLImageTargetTexture2DOES_);

    // Attempt 2: the pairing the plan actually depends on — a display built
    // from the same gbm device the buffer was allocated on, matching
    // tty.rs's open_gpu.
    const bool gbm_ok = try_gbm_paired_import(gbm, fd, stride, modifier,
                                               eglCreateImageKHR_, glEGLImageTargetTexture2DOES_);

    printf("dmabuf: summary  default-display=%s  gbm-paired-display=%s\n",
           default_ok ? "ok" : "failed", gbm_ok ? "ok" : "failed");

    // The GBM-paired result is the one the plan depends on; the default-
    // display result is context, not the answer, so it does not factor into
    // what this function reports as success.
    return gbm_ok;
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
