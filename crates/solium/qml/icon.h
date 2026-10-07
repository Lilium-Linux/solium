/*
 * `image://solium/icon/<name>?size=48&scale=2`: one theme icon, synchronous
 * (03 §3.2.14's own async provider is cut from this version -- see
 * `icon.cpp`'s file comment). The lookup itself -- the theme chain, the
 * directory matching, the fallback -- is Rust's (`crates/solium/src/icon.rs`),
 * reached the one way a C++ file in this compositor calls back into Rust:
 * the same shape as `solium_qml_log_from_qt` in `host.h`.
 */
#ifndef SOLIUM_QML_ICON_H
#define SOLIUM_QML_ICON_H

#include <QtQuick/QQuickImageProvider>

/* Rust's `icon::resolve`, across the C ABI. `name` must be a NUL-terminated
 * UTF-8 string valid for the call. Returns a heap `char*` path (UTF-8,
 * NUL-terminated) the caller must free with `solium_icon_lookup_free`, or
 * null when nothing was found. */
extern "C" char *solium_icon_lookup(const char *name, int size, int scale);
extern "C" void solium_icon_lookup_free(char *path);

class SoliumIconProvider : public QQuickImageProvider
{
public:
    SoliumIconProvider();
    QImage requestImage(const QString &id, QSize *size, const QSize &requestedSize) override;
};

#endif /* SOLIUM_QML_ICON_H */
