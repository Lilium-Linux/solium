/*
 * The `image://solium/icon/...` provider.
 *
 * **Synchronous, on purpose for this version.** 03 §3.2.14 asks for
 * `QQuickAsyncImageProvider` so a cold miss never blocks a frame; this is a
 * plain `QQuickImageProvider` instead, which means a lookup that has to walk
 * a theme's directories (no cache yet either -- see `icon.rs`'s module doc)
 * runs on whatever thread QML's image loading calls from. For a dock's few
 * dozen icons, scanned at most once each until the scene reloads, that is a
 * handful of `stat` calls -- not nothing, but not a frame stall either. The
 * natural P1 is both at once: an LRU in `icon.rs` and this provider moved to
 * `QQuickAsyncImageProvider`.
 *
 * SVG rasterises here through Qt's own `imageformats/libqsvg` plugin, loaded
 * at runtime if the Qt installation has it (Fedora's `qt6-qtsvg` ships it);
 * no `Qt6Svg` link, because `QImageReader` resolves image plugins by format,
 * not by what this binary was linked against. `setScaledSize` asks the SVG
 * plugin to rasterise at the requested size directly, rather than decoding
 * once and scaling after, so an icon stays crisp at whatever size the dock
 * asks for.
 */
#include "icon.h"

#include <QtCore/QUrlQuery>
#include <QtGui/QImageReader>

SoliumIconProvider::SoliumIconProvider() : QQuickImageProvider(QQuickImageProvider::Image) { }

QImage SoliumIconProvider::requestImage(const QString &id, QSize *size,
                                        const QSize &requestedSize)
{
    // `id` is everything after `image://solium/`: `icon/<name>?size=...`.
    // Only the `icon/` route exists in this version (tray, notification,
    // album art and avatar images are P1/P2, 03 §3.2.14's own table).
    const QString prefix = QStringLiteral("icon/");
    if (!id.startsWith(prefix)) {
        return {};
    }
    const QString rest = id.mid(prefix.size());
    const int query_at = rest.indexOf(QLatin1Char('?'));
    const QString name = query_at >= 0 ? rest.left(query_at) : rest;
    const QUrlQuery query(query_at >= 0 ? rest.mid(query_at + 1) : QString());

    const int requested = requestedSize.width() > 0 ? requestedSize.width() : 48;
    bool ok = false;
    int wanted_size = query.queryItemValue(QStringLiteral("size")).toInt(&ok);
    if (!ok || wanted_size <= 0) {
        wanted_size = requested;
    }
    int wanted_scale = query.queryItemValue(QStringLiteral("scale")).toInt(&ok);
    if (!ok || wanted_scale <= 0) {
        wanted_scale = 1;
    }
    const int device_size = wanted_size * wanted_scale;

    const QByteArray utf8_name = name.toUtf8();
    char *path = solium_icon_lookup(utf8_name.constData(), device_size, wanted_scale);
    if (path == nullptr) {
        return {};
    }
    const QString file_path = QString::fromUtf8(path);
    solium_icon_lookup_free(path);

    QImageReader reader(file_path);
    // Asks a vector format (SVG) to rasterise at this size directly, rather
    // than decode-then-scale, which is what keeps a symbolic icon crisp at an
    // odd device size (fractional scale, a size the theme itself does not
    // ship). A raster format (PNG) that is already smaller than this is
    // upscaled, the same trade a window's own icon makes.
    reader.setScaledSize(QSize(device_size, device_size));
    QImage image = reader.read();
    if (image.isNull()) {
        return {};
    }
    if (size != nullptr) {
        *size = image.size();
    }
    return image;
}
