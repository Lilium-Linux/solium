// The real host.cpp, compiled verbatim. Nothing added.
// WIRECHECK_HOST_CPP is defined by build.rs, relative to this crate, so what
// is compiled here is always the host.cpp of the checkout this file is in.
// Overridable only so a deliberately broken copy can be built as a negative
// control -- see the README.
#ifndef WIRECHECK_HOST_CPP
#error "WIRECHECK_HOST_CPP must be defined by build.rs"
#endif
#include WIRECHECK_HOST_CPP

// Whether Qt's thread-local still names a context.
//
// The C-1 case below only tests anything when this is true: a *stale* belief is
// what makes QRhiGles2::ensureContext() skip its makeCurrent. A null belief is
// safe with or without the fix, so a run that reaches the free with Qt
// believing nothing proves nothing -- which is exactly how the first version of
// that case came to pass both ways. The harness asserts on this rather than
// hoping for it.
extern "C" int wirecheck_qt_believes_it_has_the_thread(void)
{
    return QOpenGLContext::currentContext() != nullptr ? 1 : 0;
}
