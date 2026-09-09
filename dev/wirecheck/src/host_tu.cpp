// The real host.cpp, compiled verbatim. Nothing added to it.
// WIRECHECK_HOST_CPP is defined by build.rs, relative to this crate, so what
// is compiled here is always the host.cpp of the checkout this file is in.
// Overridable only so a deliberately broken copy can be built as a negative
// control -- see the README.
#ifndef WIRECHECK_HOST_CPP
#error "WIRECHECK_HOST_CPP must be defined by build.rs"
#endif
#include WIRECHECK_HOST_CPP

#include <QtGui/qopenglcontext_platform.h>

// Is Qt holding a *stale* belief about *this* scene, right now?
//
// The precondition of the free-path case, and the only state in which that case
// tests anything. Two halves, and neither alone is worth having:
//
//   * Qt's thread-local names this scene's own context. A belief is what makes
//     QRhiGles2::ensureContext() skip the makeCurrent it needs. A null belief
//     is safe with or without the fix -- and so, once Task 7 puts several
//     scenes on this path, is a belief naming some *other* live scene, which
//     ensureContext() compares against its own ctx and corrects.
//
//   * EGL disagrees. If Qt's context really is current then the belief is true
//     rather than stale, and the teardown is fine either way.
//
// The first version of this asked `QOpenGLContext::currentContext() != nullptr`
// and nothing else. It took no scene, so it could not tell whose belief it was;
// it never consulted EGL, so it could not tell a stale belief from a correct
// one; and it passed with the fix reverted.
//
// The scene's own EGLContext was captured at import from eglGetCurrentContext,
// on the thread Qt genuinely held. Qt's side of the comparison comes through
// the QEGLContext native interface rather than from anything host.cpp recorded,
// so the two are derived independently and cannot agree by construction.
extern "C" int wirecheck_belief_names_scene(void *opaque)
{
    const auto *scene = static_cast<const SoliumQmlScene *>(opaque);
    QOpenGLContext *believed = QOpenGLContext::currentContext();
    if (scene == nullptr || believed == nullptr || scene->egl_context == EGL_NO_CONTEXT) {
        return 0;
    }
    auto *egl = believed->nativeInterface<QNativeInterface::QEGLContext>();
    return egl != nullptr && egl->nativeContext() == scene->egl_context ? 1 : 0;
}

// Reported separately from the above so a lost precondition says which half went
// missing rather than only that it is gone.
extern "C" int wirecheck_egl_agrees_with(void *opaque)
{
    const auto *scene = static_cast<const SoliumQmlScene *>(opaque);
    if (scene == nullptr) {
        return 0;
    }
    return eglGetCurrentContext() == scene->egl_context ? 1 : 0;
}
