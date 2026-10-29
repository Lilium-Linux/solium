/*
 * `Media` (#379): see media.h.
 */
#include "media.h"

#include <algorithm>
#include <cmath>

#include <QtCore/QCoreApplication>
#include <QtCore/QHash>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QMutex>
#include <QtQml/QQmlEngine>

#include "host.h"
#include "notifications.h"

namespace {

SoliumRow *make_player(QObject *parent)
{
    return new SoliumMediaPlayer(parent);
}

/* The art Rust handed over, by player id. The provider may read from QML's
 * loader threads. */
QMutex art_lock;

QHash<quint32, QImage> &art()
{
    static QHash<quint32, QImage> held;
    return held;
}

} // namespace

double SoliumMediaPlayer::position() const
{
    const double at = value("position").toDouble();
    double now = at;
    const double rate = value("rate").toDouble();
    if (playing() && rate > 0) {
        const long long since = solium_qml_clock_ms() - value("positionAt").toLongLong();
        if (since > 0) {
            now += rate * static_cast<double>(since) / 1000.0;
        }
    }
    const double end = length();
    if (end > 0 && now > end) {
        now = end;
    }
    return now < 0 ? 0 : now;
}

void SoliumMediaPlayer::call(const char *action, const QJsonObject &data) const
{
    // An absent row (no player, or a lock's) is nobody to call.
    if (!present) {
        return;
    }
    QJsonObject sent = data;
    sent.insert(QStringLiteral("id"), id());
    solium_service_call(QString::fromLatin1(action), sent);
}

void SoliumMediaPlayer::playPause()
{
    call("media.play_pause");
}

void SoliumMediaPlayer::play()
{
    call("media.play");
}

void SoliumMediaPlayer::pause()
{
    call("media.pause");
}

void SoliumMediaPlayer::next()
{
    call("media.next");
}

void SoliumMediaPlayer::previous()
{
    call("media.previous");
}

void SoliumMediaPlayer::seek(double offset)
{
    call("media.seek", QJsonObject{{QStringLiteral("offset"), offset}});
}

void SoliumMediaPlayer::setPosition(double position)
{
    call("media.set_position", QJsonObject{{QStringLiteral("position"), position}});
}

void SoliumMediaPlayer::setVolume(double volume)
{
    call("media.volume", QJsonObject{{QStringLiteral("value"), volume}});
}

/* `Retire::AfterGrace`, like `Tray`: a player that leaves is gone, and an id
 * is never given out twice in a run. */
SoliumMediaRows::SoliumMediaRows()
    : SoliumRows(&SoliumMediaPlayer::staticMetaObject, make_player, Retire::AfterGrace, "id")
{
    QQmlEngine::setObjectOwnership(&m_active, QQmlEngine::CppOwnership);
    QObject::connect(this, &SoliumRows::applied, this, [this]() { follow(); });
}

/* `active` follows the row Rust marked, taken whole, so with no player it is
 * as empty as an absent row, not the player active last. */
void SoliumMediaRows::follow()
{
    const SoliumRow *now = nullptr;
    for (const SoliumRow *row : rows()) {
        if (row->present && row->value("active").toBool()) {
            now = row;
        }
    }
    const bool present = now != nullptr;
    const QVariantMap next = present ? now->values : QVariantMap();
    if (next != m_active.values || present != m_active.present) {
        m_active.values = next;
        m_active.present = present;
        m_active.announce();
    }
}

void SoliumMediaRows::tick()
{
    for (SoliumRow *row : rows()) {
        static_cast<SoliumMediaPlayer *>(row)->tick();
    }
    m_active.tick();
}

int SoliumMediaRows::untilNext(int interval) const
{
    double soonest = -1;
    for (const SoliumRow *row : rows()) {
        const auto *player = static_cast<const SoliumMediaPlayer *>(row);
        const double rate = player->rate();
        if (!player->present || !player->playing() || !(rate > 0)) {
            continue;
        }
        // From where it is to the next whole interval of the track, at its
        // rate, so a clock showing seconds turns as the track's second does.
        const double at = player->position() * 1000.0;
        const double next = (std::floor(at / interval) + 1) * interval;
        const double wait = (next - at) / rate;
        if (soonest < 0 || wait < soonest) {
            soonest = wait;
        }
    }
    if (soonest < 0) {
        return -1;
    }
    // A millisecond past the boundary, so the read lands on its far side.
    return static_cast<int>(std::ceil(std::min(soonest, static_cast<double>(interval)))) + 1;
}

/* The ticker is a QTimer: Qt serves it between frames through
 * `solium_qml_drain`, which brings the compositor's clock to now before it
 * does, so the position each tick reads is on that clock. It fires once at
 * a time, at the next whole `positionInterval` of a playing track, and is
 * set again from there. */
SoliumMedia::SoliumMedia(bool redacted, QObject *parent) : QObject(parent), m_redacted(redacted)
{
    m_ticker.setSingleShot(true);
    m_ticker.setTimerType(Qt::PreciseTimer);
    QObject::connect(&m_ticker, &QTimer::timeout, this, [this]() {
        if (SoliumMediaRows *held = rows()) {
            held->tick();
        }
        retime();
    });
    if (SoliumMediaRows *held = rows()) {
        QObject::connect(held, &SoliumRows::applied, this, [this]() { retime(); });
    }
}

SoliumMediaRows *SoliumMedia::rows() const
{
    return static_cast<SoliumMediaRows *>(players());
}

QObject *SoliumMedia::players() const
{
    return m_redacted ? solium_rows_redacted(SOLIUM_QML_ROWS_MEDIA)
                      : solium_rows(SOLIUM_QML_ROWS_MEDIA);
}

QObject *SoliumMedia::active() const
{
    SoliumMediaRows *held = rows();
    return held != nullptr ? held->active() : nullptr;
}

void SoliumMedia::setState(const QVariantMap &state)
{
    if (state != m_state) {
        m_state = state;
        emit stateChanged();
        retime();
    }
}

void SoliumMedia::retime()
{
    const int interval = positionInterval();
    SoliumMediaRows *held = rows();
    const int wait = interval > 0 && held != nullptr ? held->untilNext(interval) : -1;
    if (wait < 0) {
        m_ticker.stop();
        return;
    }
    m_ticker.start(wait);
}

QObject *SoliumMedia::get(int id) const
{
    SoliumMediaRows *held = rows();
    return held != nullptr ? held->get(QString::number(id)) : nullptr;
}

SoliumMedia *solium_media(bool redacted)
{
    static SoliumMedia *media = nullptr;
    static SoliumMedia *locked = nullptr;
    if (QCoreApplication::instance() == nullptr) {
        return nullptr;
    }
    SoliumMedia *&which = redacted ? locked : media;
    if (which == nullptr) {
        which = new SoliumMedia(redacted);
        QQmlEngine::setObjectOwnership(which, QQmlEngine::CppOwnership);
    }
    return which;
}

QImage solium_media_art(quint32 id)
{
    QMutexLocker locker(&art_lock);
    return art().value(id);
}

extern "C" int solium_qml_set_media_state(const char *json)
{
    SoliumMedia *media = solium_media(false);
    if (media == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    media->setState(document.object().toVariantMap());
    return 1;
}

extern "C" int solium_qml_set_media_art(quint32 id, int width, int height,
                                        const unsigned char *rgba, size_t len)
{
    if (QCoreApplication::instance() == nullptr || rgba == nullptr || width <= 0 || height <= 0
        || len != static_cast<size_t>(width) * static_cast<size_t>(height) * 4) {
        return 0;
    }
    // Straight RGBA as Rust made it, copied: the bytes are Rust's.
    QImage image(rgba, width, height, width * 4, QImage::Format_RGBA8888);
    QMutexLocker locker(&art_lock);
    art().insert(id, image.copy());
    return 1;
}

extern "C" void solium_qml_drop_media_art(quint32 id)
{
    QMutexLocker locker(&art_lock);
    art().remove(id);
}
