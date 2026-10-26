/*
 * `Notifications`: what Solium's notification server holds, for scenes
 * (03 §3.2.11, #346).
 *
 * Two keyed row models, `banners` and `history`, newest first, fed by Rust
 * like every other model (`crate::models`), and the service's own values
 * (`available`, `serving`, `dnd`, `owner`). The calls a scene makes on them
 * (`n.dismiss()`, `Notifications.setDnd(true)`) are queued for Rust, which
 * performs them on the service and tells Lua: the direct path of 03 §3.1.5.
 * Nothing here decides anything.
 */
#ifndef SOLIUM_QML_NOTIFICATIONS_H
#define SOLIUM_QML_NOTIFICATIONS_H

#include "rows.h"

#include <QtCore/QJsonObject>
#include <QtCore/QObject>
#include <QtCore/QVariant>
#include <QtGui/QImage>

/* One notification: a row of `banners` or `history`. A notification that
 * goes reads `present: false` for a short grace, so an exit animation bound
 * to it can finish. */
class SoliumNotification : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(int id READ id NOTIFY changed)
    Q_PROPERTY(int revision READ revision NOTIFY changed)
    Q_PROPERTY(QString appName READ appName NOTIFY changed)
    Q_PROPERTY(QString appIcon READ appIcon NOTIFY changed)
    Q_PROPERTY(QString desktopId READ desktopId NOTIFY changed)
    Q_PROPERTY(int window READ window NOTIFY changed)
    Q_PROPERTY(QString monitor READ monitor NOTIFY changed)
    Q_PROPERTY(QString summary READ summary NOTIFY changed)
    Q_PROPERTY(QString body READ body NOTIFY changed)
    Q_PROPERTY(QString bodyRich READ bodyRich NOTIFY changed)
    Q_PROPERTY(QString image READ image NOTIFY changed)
    Q_PROPERTY(QString icon READ icon NOTIFY changed)
    Q_PROPERTY(QString urgency READ urgency NOTIFY changed)
    Q_PROPERTY(QString category READ category NOTIFY changed)
    Q_PROPERTY(QVariantList actions READ actions NOTIFY changed)
    Q_PROPERTY(bool hasDefault READ hasDefault NOTIFY changed)
    Q_PROPERTY(bool resident READ resident NOTIFY changed)
    Q_PROPERTY(bool transient READ transient NOTIFY changed)
    Q_PROPERTY(double received READ received NOTIFY changed)
    Q_PROPERTY(bool held READ held NOTIFY changed)
    Q_PROPERTY(QString state READ state NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    int id() const { return value("id").toInt(); }
    int revision() const { return value("revision").toInt(); }
    QString appName() const { return value("appName").toString(); }
    QString appIcon() const { return value("appIcon").toString(); }
    QString desktopId() const { return value("desktopId").toString(); }
    int window() const { return value("window").toInt(); }
    QString monitor() const { return value("monitor").toString(); }
    QString summary() const { return value("summary").toString(); }
    QString body() const { return value("body").toString(); }
    QString bodyRich() const { return value("bodyRich").toString(); }
    QString image() const { return value("image").toString(); }
    QString icon() const { return value("icon").toString(); }
    QString urgency() const { return value("urgency").toString(); }
    QString category() const { return value("category").toString(); }
    QVariantList actions() const { return value("actions").toList(); }
    bool hasDefault() const { return value("hasDefault").toBool(); }
    bool resident() const { return value("resident").toBool(); }
    bool transient() const { return value("transient").toBool(); }
    double received() const { return value("received").toDouble(); }
    bool held() const { return value("held").toBool(); }
    QString state() const { return value("state").toString(); }
    void announce() override { emit changed(); }

    /* Gone, as dismissed by the user. */
    Q_INVOKABLE void dismiss();
    /* A pointer or a finger rests on its banner: its time stops. */
    Q_INVOKABLE void hold();
    Q_INVOKABLE void release();
    /* An action, by key; `default` is a click on the body. */
    Q_INVOKABLE void invoke(const QString &key = QStringLiteral("default"));
signals:
    void changed();
};

/* `banners` or `history`. */
class SoliumNotificationRows : public SoliumRows
{
    Q_OBJECT
public:
    SoliumNotificationRows();
};

/* `Notifications`, as a scene names it. */
class SoliumNotifications : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool available READ available NOTIFY stateChanged)
    Q_PROPERTY(bool serving READ serving NOTIFY stateChanged)
    Q_PROPERTY(bool dnd READ dnd WRITE setDnd NOTIFY stateChanged)
    Q_PROPERTY(QVariantMap owner READ owner NOTIFY stateChanged)
    Q_PROPERTY(QObject *banners READ banners CONSTANT)
    Q_PROPERTY(QObject *history READ history CONSTANT)
public:
    explicit SoliumNotifications(QObject *parent = nullptr);
    bool available() const { return m_state.value(QStringLiteral("available")).toBool(); }
    bool serving() const { return m_state.value(QStringLiteral("serving")).toBool(); }
    bool dnd() const { return m_state.value(QStringLiteral("dnd")).toBool(); }
    QVariantMap owner() const { return m_state.value(QStringLiteral("owner")).toMap(); }
    QObject *banners() const;
    QObject *history() const;
    void setState(const QVariantMap &state);

    /* The live notification with this id, banner or history, or an absent
     * row. */
    Q_INVOKABLE QObject *get(int id) const;
    Q_INVOKABLE void dismiss(int id);
    Q_INVOKABLE void dismissAll();
    Q_INVOKABLE void hold(int id);
    Q_INVOKABLE void release(int id);
    Q_INVOKABLE void invoke(int id, const QString &key = QStringLiteral("default"));
    Q_INVOKABLE void setDnd(bool on);
signals:
    void stateChanged();

private:
    QVariantMap m_state;
};

/* The one `Notifications` every scene names. */
SoliumNotifications *solium_notifications();

/* Queue a call a scene made, for Rust (`take_service_call`). */
void solium_service_call(const QString &action, const QJsonObject &data);

/* The image behind `image://solium/notification/<id>/<revision>`, or a null
 * image. Called from the image provider, on whatever thread it runs on. */
QImage solium_notification_image(quint32 id);

#endif
