/*
 * The `image://solium/icon/...` provider.
 *
 * **Synchronous, on purpose.** 03 §3.2.14 asks for
 * `QQuickAsyncImageProvider` so a cold miss never blocks a frame; this is a
 * plain `QQuickImageProvider` instead, because an asynchronous one lets icons
 * pop in after the first frame of a desktop, which must show whole. A request
 * runs on whatever thread QML's image loading calls from -- the GUI thread for
 * an ordinary `Image`.
 *
 * **Every answer is kept** (#306), by the whole request, a miss included, up
 * to `CACHE_KIB`: a shell shows the same icons again and again (a launcher
 * page, a header per window in an overview, a window's icon as it flies to its
 * launcher), and QML's own pixmap cache keeps an image nobody shows only
 * within a small budget. Without it each of those walked the theme and
 * rasterised the SVG again, about 5 ms for a 256 px icon, inside an animation
 * frame. Only the first request for an icon pays.
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

#include <QtCore/QCache>
#include <QtCore/QMutex>
#include <QtCore/QUrlQuery>
#include <QtGui/QImageReader>

namespace {

/* The answers kept, in KiB of pixels: room for a few hundred icons at the
 * sizes a shell asks for, a few at a window's size. */
constexpr qsizetype CACHE_KIB = 48 * 1024;

/* QML may load from its loader threads too (`asynchronous: true`). */
QMutex cache_lock;

QCache<QString, QImage> &kept()
{
    static QCache<QString, QImage> cache(CACHE_KIB);
    return cache;
}

QImage keep(const QString &key, const QImage &image)
{
    const qsizetype kib = image.sizeInBytes() / 1024;
    QMutexLocker locker(&cache_lock);
    kept().insert(key, new QImage(image), kib > 0 ? kib : 1);
    return image;
}

} // namespace

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

    const QString key = name + QLatin1Char('@') + QString::number(device_size) + QLatin1Char('x')
        + QString::number(wanted_scale);
    {
        QMutexLocker locker(&cache_lock);
        if (const QImage *image = kept().object(key)) {
            if (size != nullptr && !image->isNull()) {
                *size = image->size();
            }
            return *image;
        }
    }

    const QByteArray utf8_name = name.toUtf8();
    char *path = solium_icon_lookup(utf8_name.constData(), device_size, wanted_scale);
    if (path == nullptr) {
        return keep(key, {});
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
        return keep(key, {});
    }
    if (size != nullptr) {
        *size = image.size();
    }
    return keep(key, image);
}
