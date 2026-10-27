/*
 * `Tray` (#363): see tray.h.
 */
#include "tray.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QHash>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QMutex>
#include <QtQml/QQmlEngine>

#include "host.h"
#include "notifications.h"

namespace {

SoliumRow *make_item(QObject *parent)
{
    return new SoliumTrayItem(parent);
}

/* The pictures Rust handed over, by `<id>/<kind>`. The provider may read
 * from QML's loader threads. */
QMutex images_lock;

QHash<QString, QImage> &images()
{
    static QHash<QString, QImage> held;
    return held;
}

/* The kinds `crate::services::tray::model::Picture` numbers. */
QString kind_name(quint32 kind)
{
    switch (kind) {
    case 0:
        return QStringLiteral("icon");
    case 1:
        return QStringLiteral("attention");
    case 2:
        return QStringLiteral("overlay");
    default:
        return {};
    }
}

QString image_key(quint32 id, const QString &kind)
{
    return QString::number(id) + QLatin1Char('/') + kind;
}

QJsonObject at(int id, int x, int y)
{
    return QJsonObject{{QStringLiteral("id"), id}, {QStringLiteral("x"), x}, {QStringLiteral("y"), y}};
}

} // namespace

void SoliumTrayItem::activate(int x, int y)
{
    solium_service_call(QStringLiteral("tray.activate"), at(id(), x, y));
}

void SoliumTrayItem::secondaryActivate(int x, int y)
{
    solium_service_call(QStringLiteral("tray.secondary_activate"), at(id(), x, y));
}

void SoliumTrayItem::scroll(int delta, const QString &orientation)
{
    solium_service_call(QStringLiteral("tray.scroll"),
                        QJsonObject{{QStringLiteral("id"), id()},
                                    {QStringLiteral("delta"), delta},
                                    {QStringLiteral("orientation"), orientation}});
}

void SoliumTrayItem::contextMenu(int x, int y)
{
    solium_service_call(QStringLiteral("tray.context_menu"), at(id(), x, y));
}

void SoliumTrayItem::openMenu(int x, int y)
{
    solium_service_call(QStringLiteral("tray.open_menu"), at(id(), x, y));
}

void SoliumTrayItem::closeMenu()
{
    solium_service_call(QStringLiteral("tray.close_menu"), QJsonObject{{QStringLiteral("id"), id()}});
}

void SoliumTrayItem::clickMenu(int entry)
{
    solium_service_call(QStringLiteral("tray.menu_click"),
                        QJsonObject{{QStringLiteral("id"), id()}, {QStringLiteral("entry"), entry}});
}

void SoliumTrayItem::openSubmenu(int entry)
{
    solium_service_call(QStringLiteral("tray.submenu"),
                        QJsonObject{{QStringLiteral("id"), id()}, {QStringLiteral("entry"), entry}});
}

/* `Retire::AfterGrace`, like `Notifications`: an item that leaves is gone,
 * and an id is never given out twice in a run. */
SoliumTrayRows::SoliumTrayRows()
    : SoliumRows(&SoliumTrayItem::staticMetaObject, make_item, Retire::AfterGrace, "id")
{
}

SoliumTray::SoliumTray(bool redacted, QObject *parent) : QObject(parent), m_redacted(redacted) { }

QObject *SoliumTray::items() const
{
    return m_redacted ? solium_rows_redacted(SOLIUM_QML_ROWS_TRAY)
                      : solium_rows(SOLIUM_QML_ROWS_TRAY);
}

void SoliumTray::setState(const QVariantMap &state)
{
    if (state != m_state) {
        m_state = state;
        emit stateChanged();
    }
}

QObject *SoliumTray::get(int id) const
{
    auto *rows = qobject_cast<SoliumRows *>(items());
    return rows != nullptr ? rows->get(QString::number(id)) : nullptr;
}

void SoliumTray::closeMenus()
{
    if (!m_redacted) {
        solium_service_call(QStringLiteral("tray.close_menu"), QJsonObject{});
    }
}

SoliumTray *solium_tray(bool redacted)
{
    static SoliumTray *tray = nullptr;
    static SoliumTray *locked = nullptr;
    if (QCoreApplication::instance() == nullptr) {
        return nullptr;
    }
    SoliumTray *&which = redacted ? locked : tray;
    if (which == nullptr) {
        which = new SoliumTray(redacted);
        QQmlEngine::setObjectOwnership(which, QQmlEngine::CppOwnership);
    }
    return which;
}

QImage solium_tray_image(quint32 id, const QString &kind)
{
    QMutexLocker locker(&images_lock);
    return images().value(image_key(id, kind));
}

extern "C" int solium_qml_set_tray_state(const char *json)
{
    SoliumTray *tray = solium_tray(false);
    if (tray == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    tray->setState(document.object().toVariantMap());
    return 1;
}

extern "C" int solium_qml_set_tray_image(quint32 id, quint32 kind, int width, int height,
                                         const unsigned char *rgba, size_t len)
{
    const QString name = kind_name(kind);
    if (QCoreApplication::instance() == nullptr || name.isEmpty() || rgba == nullptr
        || width <= 0 || height <= 0
        || len != static_cast<size_t>(width) * static_cast<size_t>(height) * 4) {
        return 0;
    }
    // Straight RGBA as Rust made it, copied: the bytes are Rust's.
    QImage image(rgba, width, height, width * 4, QImage::Format_RGBA8888);
    QMutexLocker locker(&images_lock);
    images().insert(image_key(id, name), image.copy());
    return 1;
}

extern "C" void solium_qml_drop_tray_image(quint32 id, quint32 kind)
{
    const QString name = kind_name(kind);
    if (name.isEmpty()) {
        return;
    }
    QMutexLocker locker(&images_lock);
    images().remove(image_key(id, name));
}
