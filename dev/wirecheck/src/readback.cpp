// The compositor's side of the join: import the SAME dmabuf on a second,
// independent EGL display and read the pixels Qt left in it.
//
// This is our own code, not host.cpp's — it stands in for what the compositor
// does when it samples the shared buffer. The point is that it is a *different*
// context from Qt's, so it proves the pixels landed in the shared memory rather
// than merely in Qt's own texture object.
//
// The modifier rule below mirrors host.cpp's exactly: modifier attributes go on
// unless the extension is missing or the modifier is the INVALID sentinel.

#include <gbm.h>
#include <fcntl.h>
#include <unistd.h>
#include <cstdio>
#include <cstring>
#include <cstdint>
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>

static constexpr unsigned long long kModifierInvalid = (1ULL << 56) - 1;

extern "C" int join_readback(int dmabuf_fd, int w, int h, int stride,
                             unsigned long long modifier, unsigned int fourcc,
                             unsigned char *out)
{
    auto create_image =
        reinterpret_cast<PFNEGLCREATEIMAGEKHRPROC>(eglGetProcAddress("eglCreateImageKHR"));
    auto target_texture = reinterpret_cast<PFNGLEGLIMAGETARGETTEXTURE2DOESPROC>(
        eglGetProcAddress("glEGLImageTargetTexture2DOES"));
    auto get_platform_display = reinterpret_cast<PFNEGLGETPLATFORMDISPLAYEXTPROC>(
        eglGetProcAddress("eglGetPlatformDisplayEXT"));
    if (!create_image || !target_texture || !get_platform_display) {
        printf("  readback: entry points missing\n");
        return -1;
    }

    int drm = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
    if (drm < 0) { printf("  readback: no render node\n"); return -2; }
    gbm_device *gbm = gbm_create_device(drm);
    if (!gbm) { printf("  readback: no gbm device\n"); return -3; }

    EGLDisplay dpy = get_platform_display(EGL_PLATFORM_GBM_KHR, gbm, nullptr);
    EGLint major = 0, minor = 0;
    if (dpy == EGL_NO_DISPLAY || !eglInitialize(dpy, &major, &minor)) {
        printf("  readback: gbm-paired display failed 0x%x\n", eglGetError());
        return -4;
    }
    eglBindAPI(EGL_OPENGL_ES_API);
    const EGLint cfg_attrs[] = { EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
                                 EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_NONE };
    EGLConfig cfg{}; EGLint found = 0;
    eglChooseConfig(dpy, cfg_attrs, &cfg, 1, &found);
    if (!found) { printf("  readback: no config\n"); return -5; }
    const EGLint ctx_attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctx_attrs);
    const EGLint pb_attrs[] = { EGL_WIDTH, w, EGL_HEIGHT, h, EGL_NONE };
    EGLSurface pb = eglCreatePbufferSurface(dpy, cfg, pb_attrs);
    if (!eglMakeCurrent(dpy, pb, pb, ctx)) {
        printf("  readback: makeCurrent failed 0x%x\n", eglGetError());
        return -6;
    }
    printf("  readback: EGL %d.%d on an independent gbm-paired display %p\n",
           major, minor, static_cast<void *>(dpy));

    const char *ext = eglQueryString(dpy, EGL_EXTENSIONS);
    const bool has_modifiers = ext && strstr(ext, "EGL_EXT_image_dma_buf_import_modifiers");
    const bool with_modifier = has_modifiers && modifier != kModifierInvalid;

    EGLint attribs[] = {
        EGL_WIDTH, w,
        EGL_HEIGHT, h,
        EGL_LINUX_DRM_FOURCC_EXT, static_cast<EGLint>(fourcc),
        EGL_DMA_BUF_PLANE0_FD_EXT, dmabuf_fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, stride,
        EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, static_cast<EGLint>(modifier & 0xffffffffULL),
        EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, static_cast<EGLint>(modifier >> 32),
        EGL_NONE,
    };
    if (!with_modifier) {
        attribs[12] = EGL_NONE;
    }
    printf("  readback: importing %s modifier attributes\n", with_modifier ? "WITH" : "WITHOUT");

    EGLImageKHR img = create_image(dpy, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, nullptr, attribs);
    if (img == EGL_NO_IMAGE_KHR) {
        printf("  readback: eglCreateImageKHR FAILED 0x%x\n", eglGetError());
        return -7;
    }
    GLuint tex = 0;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_2D, tex);
    target_texture(GL_TEXTURE_2D, img);
    GLenum e = glGetError();
    if (e != GL_NO_ERROR) {
        printf("  readback: glEGLImageTargetTexture2DOES FAILED 0x%x\n", e);
        return -8;
    }
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    printf("  readback: imported the same buffer as texture %u\n", tex);

    GLuint fbo = 0;
    glGenFramebuffers(1, &fbo);
    glBindFramebuffer(GL_FRAMEBUFFER, fbo);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
    GLenum st = glCheckFramebufferStatus(GL_FRAMEBUFFER);
    if (st != GL_FRAMEBUFFER_COMPLETE) {
        printf("  readback: FBO incomplete 0x%x\n", st);
        return -9;
    }
    glReadPixels(0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, out);
    e = glGetError();
    if (e != GL_NO_ERROR) {
        printf("  readback: glReadPixels FAILED 0x%x\n", e);
        return -10;
    }
    printf("  readback: read %d x %d RGBA back out of the shared buffer\n", w, h);
    return 0;
}

/* Is EGL_ANDROID_native_fence_sync advertised here? Reported for context; the
 * real answer for the join is whether host.cpp handed back a fence fd. */
extern "C" int join_has_fence_ext(void)
{
    EGLDisplay dpy = eglGetCurrentDisplay();
    if (dpy == EGL_NO_DISPLAY) { return -1; }
    const char *ext = eglQueryString(dpy, EGL_EXTENSIONS);
    return (ext && strstr(ext, "EGL_ANDROID_native_fence_sync")) ? 1 : 0;
}
