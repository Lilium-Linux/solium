/*
 * `Notifications` (#346): see notifications.h.
 */
#include "notifications.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QHash>
#include <QtCore/QJsonDocument>
#include <QtCore/QMutex>
#include <QtCore/QVector>
#include <QtQml/QQmlEngine>

#include "host.h"

namespace {

SoliumRow *make_notification(QObject *parent)
{
    return new SoliumNotification(parent);
}

/* Calls scenes made, oldest first, until Rust takes them. */
QVector<QPair<QByteArray, QByteArray>> &calls()
{
    static QVector<QPair<QByteArray, QByteArray>> queued;
    return queued;
}

/* The images Rust handed over, by notification id. The provider may read
 * from QML's loader threads. */
QMutex images_lock;

QHash<quint32, QImage> &images()
{
    static QHash<quint32, QImage> held;
    return held;
}

QJsonObject with_id(int id)
{
    return QJsonObject{{QStringLiteral("id"), id}};
}

} // namespace

void solium_service_call(const QString &action, const QJsonObject &data)
{
    // A scene that floods the queue between two loops cannot grow it without
    // bound: past this, its calls are dropped.
    if (calls().size() >= 1024) {
        return;
    }
    calls().append({action.toUtf8(), QJsonDocument(data).toJson(QJsonDocument::Compact)});
}

void SoliumNotification::dismiss()
{
    solium_service_call(QStringLiteral("notifications.dismiss"), with_id(id()));
}

void SoliumNotification::hold()
{
    solium_service_call(QStringLiteral("notifications.hold"), with_id(id()));
}

void SoliumNotification::release()
{
    solium_service_call(QStringLiteral("notifications.release"), with_id(id()));
}

void SoliumNotification::invoke(const QString &key)
{
    QJsonObject data = with_id(id());
    data.insert(QStringLiteral("key"), key);
    solium_service_call(QStringLiteral("notifications.invoke"), data);
}

/* `Retire::AfterGrace`, like `Windows`: a closed notification is gone, and
 * an id is never given out twice in a run. */
SoliumNotificationRows::SoliumNotificationRows()
    : SoliumRows(&SoliumNotification::staticMetaObject, make_notification, Retire::AfterGrace,
                 "id")
{
}

SoliumNotifications::SoliumNotifications(QObject *parent) : QObject(parent) { }

QObject *SoliumNotifications::banners() const
{
    return solium_rows(SOLIUM_QML_ROWS_NOTIFICATION_BANNERS);
}

QObject *SoliumNotifications::history() const
{
    return solium_rows(SOLIUM_QML_ROWS_NOTIFICATION_HISTORY);
}

void SoliumNotifications::setState(const QVariantMap &state)
{
    if (state != m_state) {
        m_state = state;
        emit stateChanged();
    }
}

QObject *SoliumNotifications::get(int id) const
{
    const QString key = QString::number(id);
    for (int model :
         {SOLIUM_QML_ROWS_NOTIFICATION_BANNERS, SOLIUM_QML_ROWS_NOTIFICATION_HISTORY}) {
        SoliumRows *rows = solium_rows(model);
        if (rows == nullptr) {
            continue;
        }
        if (QObject *row = rows->get(key); row != nullptr) {
            if (auto *notification = qobject_cast<SoliumNotification *>(row);
                notification != nullptr && notification->present) {
                return notification;
            }
        }
    }
    SoliumRows *banners = solium_rows(SOLIUM_QML_ROWS_NOTIFICATION_BANNERS);
    return banners != nullptr ? banners->get(key) : nullptr;
}

void SoliumNotifications::dismiss(int id)
{
    solium_service_call(QStringLiteral("notifications.dismiss"), with_id(id));
}

void SoliumNotifications::dismissAll()
{
    solium_service_call(QStringLiteral("notifications.dismiss"),
                        QJsonObject{{QStringLiteral("all"), true}});
}

void SoliumNotifications::hold(int id)
{
    solium_service_call(QStringLiteral("notifications.hold"), with_id(id));
}

void SoliumNotifications::release(int id)
{
    solium_service_call(QStringLiteral("notifications.release"), with_id(id));
}

void SoliumNotifications::invoke(int id, const QString &key)
{
    QJsonObject data = with_id(id);
    data.insert(QStringLiteral("key"), key);
    solium_service_call(QStringLiteral("notifications.invoke"), data);
}

void SoliumNotifications::setDnd(bool on)
{
    solium_service_call(QStringLiteral("notifications.dnd"),
                        QJsonObject{{QStringLiteral("value"), on}});
}

SoliumNotifications *solium_notifications()
{
    static SoliumNotifications *notifications = nullptr;
    if (QCoreApplication::instance() == nullptr) {
        return nullptr;
    }
    if (notifications == nullptr) {
        notifications = new SoliumNotifications();
        QQmlEngine::setObjectOwnership(notifications, QQmlEngine::CppOwnership);
    }
    return notifications;
}

QImage solium_notification_image(quint32 id)
{
    QMutexLocker locker(&images_lock);
    return images().value(id);
}

extern "C" int solium_qml_set_notifications_state(const char *json)
{
    SoliumNotifications *notifications = solium_notifications();
    if (notifications == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    notifications->setState(document.object().toVariantMap());
    return 1;
}

extern "C" int solium_qml_set_notification_image(quint32 id, quint32 revision, int width,
                                                 int height, const unsigned char *rgba,
                                                 size_t len)
{
    Q_UNUSED(revision);
    if (QCoreApplication::instance() == nullptr || rgba == nullptr || width <= 0 || height <= 0
        || len != static_cast<size_t>(width) * static_cast<size_t>(height) * 4) {
        return 0;
    }
    // Straight RGBA as Rust decoded it, copied: the bytes are Rust's.
    QImage image(rgba, width, height, width * 4, QImage::Format_RGBA8888);
    QMutexLocker locker(&images_lock);
    images().insert(id, image.copy());
    return 1;
}

extern "C" void solium_qml_drop_notification_image(quint32 id)
{
    QMutexLocker locker(&images_lock);
    images().remove(id);
}

extern "C" int solium_qml_take_service_call(const char **action, const char **data_json)
{
    if (calls().isEmpty()) {
        return 0;
    }
    static QByteArray name;
    static QByteArray data;
    const auto taken = calls().takeFirst();
    name = taken.first;
    data = taken.second;
    if (action != nullptr) {
        *action = name.constData();
    }
    if (data_json != nullptr) {
        *data_json = data.constData();
    }
    return 1;
}
