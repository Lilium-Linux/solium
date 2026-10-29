/*
 * `Media`: the MPRIS players on the session bus, for scenes (#379).
 *
 * `players` is a keyed row model fed by Rust like every other model
 * (`crate::models`), `active` a facade that follows the player Rust chose
 * (the one playing, else the last that played) and is never null, with the
 * service's own values (`available`). A player's `position` moves on by
 * itself while it plays: Rust sends where it was and when, on the
 * compositor's clock, and the row works out where it is now each time it is
 * read, telling its bindings to read again as the track reaches each
 * whole `positionInterval` milliseconds. The calls a scene makes
 * (`player.playPause()`, `player.seek(10)`) are queued for Rust, which makes
 * them on the bus: the direct path, as `Notifications`' and `Tray`'s are.
 * Nothing here decides anything.
 */
#ifndef SOLIUM_QML_MEDIA_H
#define SOLIUM_QML_MEDIA_H

#include "rows.h"

#include <QtCore/QJsonObject>
#include <QtCore/QObject>
#include <QtCore/QTimer>
#include <QtCore/QVariant>
#include <QtGui/QImage>

/* One player: a row of `players`, and `Media.active`. A player that leaves
 * reads `present: false` for a short grace, so an exit animation bound to it
 * can finish; `Media.active` with no player reads `present: false` too. The
 * player's own text (title, artist, album, identity) is plain text: show it
 * with `textFormat: Text.PlainText`. */
class SoliumMediaPlayer : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(int id READ id NOTIFY changed)
    Q_PROPERTY(QString service READ service NOTIFY changed)
    Q_PROPERTY(QString identity READ identity NOTIFY changed)
    Q_PROPERTY(QString desktopEntry READ desktopEntry NOTIFY changed)
    Q_PROPERTY(QString icon READ icon NOTIFY changed)
    Q_PROPERTY(QString status READ status NOTIFY changed)
    Q_PROPERTY(bool playing READ playing NOTIFY changed)
    Q_PROPERTY(bool active READ active NOTIFY changed)
    Q_PROPERTY(QString title READ title NOTIFY changed)
    Q_PROPERTY(QString artist READ artist NOTIFY changed)
    Q_PROPERTY(QString album READ album NOTIFY changed)
    Q_PROPERTY(QString art READ art NOTIFY changed)
    Q_PROPERTY(QString artUrl READ artUrl NOTIFY changed)
    Q_PROPERTY(QString trackId READ trackId NOTIFY changed)
    Q_PROPERTY(double length READ length NOTIFY changed)
    Q_PROPERTY(double position READ position NOTIFY positionChanged)
    Q_PROPERTY(double rate READ rate NOTIFY changed)
    Q_PROPERTY(QVariant volume READ volume NOTIFY changed)
    Q_PROPERTY(QVariant shuffle READ shuffle NOTIFY changed)
    Q_PROPERTY(QString loopStatus READ loopStatus NOTIFY changed)
    Q_PROPERTY(bool canPlay READ canPlay NOTIFY changed)
    Q_PROPERTY(bool canPause READ canPause NOTIFY changed)
    Q_PROPERTY(bool canGoNext READ canGoNext NOTIFY changed)
    Q_PROPERTY(bool canGoPrevious READ canGoPrevious NOTIFY changed)
    Q_PROPERTY(bool canSeek READ canSeek NOTIFY changed)
    Q_PROPERTY(bool canControl READ canControl NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    int id() const { return value("id").toInt(); }
    QString service() const { return value("service").toString(); }
    QString identity() const { return value("identity").toString(); }
    QString desktopEntry() const { return value("desktopEntry").toString(); }
    QString icon() const { return value("icon").toString(); }
    QString status() const { return value("status").toString(); }
    bool playing() const { return value("playing").toBool(); }
    bool active() const { return value("active").toBool(); }
    QString title() const { return value("title").toString(); }
    QString artist() const { return value("artist").toString(); }
    QString album() const { return value("album").toString(); }
    QString art() const { return value("art").toString(); }
    QString artUrl() const { return value("artUrl").toString(); }
    QString trackId() const { return value("trackId").toString(); }
    double length() const { return value("length").toDouble(); }
    /* Seconds into the track now: where it was at `positionAt` on the
     * compositor's clock, moved on at `rate` while it plays, and held within
     * the track. */
    double position() const;
    double rate() const { return value("rate").toDouble(); }
    QVariant volume() const { return value("volume"); }
    QVariant shuffle() const { return value("shuffle"); }
    QString loopStatus() const { return value("loopStatus").toString(); }
    bool canPlay() const { return value("canPlay").toBool(); }
    bool canPause() const { return value("canPause").toBool(); }
    bool canGoNext() const { return value("canGoNext").toBool(); }
    bool canGoPrevious() const { return value("canGoPrevious").toBool(); }
    bool canSeek() const { return value("canSeek").toBool(); }
    bool canControl() const { return value("canControl").toBool(); }
    void announce() override
    {
        emit changed();
        emit positionChanged();
    }
    /* Its position's bindings told to read again, while it plays. */
    void tick()
    {
        if (present && playing()) {
            emit positionChanged();
        }
    }

    Q_INVOKABLE void playPause();
    Q_INVOKABLE void play();
    Q_INVOKABLE void pause();
    Q_INVOKABLE void next();
    Q_INVOKABLE void previous();
    /* By `offset` seconds, forward or back. */
    Q_INVOKABLE void seek(double offset);
    /* To `position` seconds into the track. */
    Q_INVOKABLE void setPosition(double position);
    /* Its own volume, 0.0 to 1.0, for a player that offers one. */
    Q_INVOKABLE void setVolume(double volume);
signals:
    void changed();
    void positionChanged();

private:
    void call(const char *action, const QJsonObject &data = {}) const;
};

class SoliumMediaRows : public SoliumRows
{
    Q_OBJECT
public:
    SoliumMediaRows();
    SoliumMediaPlayer *active() { return &m_active; }
    /* Every playing player's position told to move on. */
    void tick();
    /* Milliseconds until the first playing player's position reaches its
     * next whole `interval`, or -1 with none playing. */
    int untilNext(int interval) const;

private:
    void follow();
    SoliumMediaPlayer m_active;
};

/* `Media`, as a scene names it. A lock's scenes read one with no players. */
class SoliumMedia : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool available READ available NOTIFY stateChanged)
    Q_PROPERTY(int positionInterval READ positionInterval NOTIFY stateChanged)
    Q_PROPERTY(QObject *players READ players CONSTANT)
    Q_PROPERTY(QObject *active READ active CONSTANT)
public:
    explicit SoliumMedia(bool redacted, QObject *parent = nullptr);
    bool available() const { return m_state.value(QStringLiteral("available")).toBool(); }
    int positionInterval() const { return m_state.value(QStringLiteral("positionInterval")).toInt(); }
    QObject *players() const;
    QObject *active() const;
    void setState(const QVariantMap &state);

    /* The player with this id, or an absent row. */
    Q_INVOKABLE QObject *get(int id) const;
signals:
    void stateChanged();

private:
    SoliumMediaRows *rows() const;
    /* The ticker runs while a player plays and the interval is not 0, set
     * for the next whole interval of a playing track. */
    void retime();
    bool m_redacted;
    QVariantMap m_state;
    QTimer m_ticker;
};

/* The `Media` a scene names: the session's, or, `redacted`, the lock's. */
SoliumMedia *solium_media(bool redacted);

/* The art behind `image://solium/media/<id>/<revision>`, or a null image.
 * Called from the image provider, on whatever thread it runs on. */
QImage solium_media_art(quint32 id);

#endif
