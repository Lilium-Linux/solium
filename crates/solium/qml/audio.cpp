/*
 * `Audio` (#347): see audio.h.
 */
#include "audio.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtQml/QQmlEngine>

#include "host.h"
#include "notifications.h"

namespace {

SoliumRow *make_endpoint(QObject *parent)
{
    return new SoliumAudioEndpoint(parent);
}

SoliumRow *make_stream(QObject *parent)
{
    return new SoliumAudioStream(parent);
}

/* A call on this endpoint, with whom it names: a window's streams, all of
 * them, for `forWindow`'s facade (#416); the default, for a facade of the
 * output or the input, so a call made as the default changes lands on the
 * new one; else itself, by id. */
QJsonObject call_on(const SoliumAudioEndpoint &endpoint, const char *key, const QJsonValue &value)
{
    QJsonObject call{{QString::fromLatin1(key), value}};
    if (endpoint.window != 0) {
        call.insert(QStringLiteral("window"), static_cast<double>(endpoint.window));
    } else if (!endpoint.facade.isEmpty()) {
        call.insert(QStringLiteral("target"), endpoint.facade);
    } else {
        call.insert(QStringLiteral("target"), endpoint.id());
    }
    return call;
}

} // namespace

void SoliumAudioEndpoint::setVolume(double volume)
{
    solium_service_call(QStringLiteral("audio.volume"), call_on(*this, "value", volume));
}

void SoliumAudioEndpoint::adjust(double by)
{
    solium_service_call(QStringLiteral("audio.volume"), call_on(*this, "by", by));
}

void SoliumAudioEndpoint::setMuted(bool muted)
{
    solium_service_call(QStringLiteral("audio.mute"), call_on(*this, "value", muted));
}

void SoliumAudioEndpoint::toggleMuted()
{
    solium_service_call(QStringLiteral("audio.mute"),
                        call_on(*this, "value", QStringLiteral("toggle")));
}

void SoliumAudioEndpoint::makeDefault()
{
    solium_service_call(QStringLiteral("audio.default"),
                        QJsonObject{{QStringLiteral("endpoint"), id()}});
}

/* `Retire::Never`: a headset unplugged and plugged in again is the same row,
 * by the id Solium keeps for its `node.name`. */
SoliumAudioRows::SoliumAudioRows()
    : SoliumRows(&SoliumAudioEndpoint::staticMetaObject, make_endpoint, Retire::Never, "id")
{
}

/* A stream's id is its node's, which never comes back, so a stream gone is
 * a row gone. */
SoliumAudioStreamRows::SoliumAudioStreamRows()
    : SoliumRows(&SoliumAudioStream::staticMetaObject, make_stream, Retire::Never, "id")
{
}

SoliumAudio::SoliumAudio(bool redacted, QObject *parent) : QObject(parent), m_redacted(redacted)
{
    m_output.facade = QStringLiteral("output");
    m_input.facade = QStringLiteral("input");
    QQmlEngine::setObjectOwnership(&m_output, QQmlEngine::CppOwnership);
    QQmlEngine::setObjectOwnership(&m_input, QQmlEngine::CppOwnership);
}

QObject *SoliumAudio::outputs() const
{
    return solium_rows(SOLIUM_QML_ROWS_AUDIO_OUTPUTS);
}

QObject *SoliumAudio::inputs() const
{
    return solium_rows(SOLIUM_QML_ROWS_AUDIO_INPUTS);
}

QObject *SoliumAudio::streams() const
{
    return m_redacted ? solium_rows_redacted(SOLIUM_QML_ROWS_AUDIO_STREAMS)
                      : solium_rows(SOLIUM_QML_ROWS_AUDIO_STREAMS);
}

QObject *SoliumAudio::forWindow(int id)
{
    const quint64 key = id > 0 ? static_cast<quint64>(id) : 0;
    QPointer<SoliumAudioStream> &held = m_windows[key];
    if (held == nullptr) {
        /* The scene's to keep, and so to let go of: a window asked about
         * once is not kept here for the rest of the session. */
        held = new SoliumAudioStream();
        held->window = key;
        QQmlEngine::setObjectOwnership(held, QQmlEngine::JavaScriptOwnership);
        assign(held);
    }
    return held;
}

/* A window facade given its window's row, or absent when the window has no
 * stream, or this is the lock's `Audio`. */
void SoliumAudio::assign(SoliumAudioStream *facade) const
{
    const QVariant row = m_redacted || facade->window == 0
                             ? QVariant()
                             : m_window_rows.value(QString::number(facade->window));
    const bool present = row.isValid() && !row.isNull();
    bool changed = present && !facade->assign(row.toMap()).isEmpty();
    if (facade->present != present) {
        facade->present = present;
        changed = true;
    }
    if (changed) {
        facade->announce();
    }
}

void SoliumAudio::setState(const QVariantMap &state)
{
    const bool available = state.value(QStringLiteral("available")).toBool();
    const bool ready = state.value(QStringLiteral("ready")).toBool();
    // Every value written first, every change announced after, as a row
    // batch does: no binding sees a new volume with an old device.
    struct Facade {
        SoliumAudioEndpoint *endpoint;
        QVariant next;
        bool changed = false;
    };
    Facade facades[] = {{&m_output, state.value(QStringLiteral("output"))},
                        {&m_input, state.value(QStringLiteral("input"))}};
    for (Facade &facade : facades) {
        const bool present = facade.next.isValid() && !facade.next.isNull();
        if (present) {
            facade.changed = !facade.endpoint->assign(facade.next.toMap()).isEmpty();
        }
        if (facade.endpoint->present != present) {
            facade.endpoint->present = present;
            facade.changed = true;
        }
    }
    const bool own = available != m_available || ready != m_ready;
    m_available = available;
    m_ready = ready;
    m_window_rows = state.value(QStringLiteral("windows")).toMap();
    for (Facade &facade : facades) {
        if (facade.changed) {
            facade.endpoint->announce();
        }
    }
    // Every window facade a scene still holds, and the ones no scene holds
    // any more let go of.
    for (auto each = m_windows.begin(); each != m_windows.end();) {
        if (each.value() == nullptr) {
            each = m_windows.erase(each);
            continue;
        }
        assign(each.value());
        ++each;
    }
    if (own) {
        emit stateChanged();
    }
}

SoliumAudio *solium_audio(bool redacted)
{
    static SoliumAudio *audio = nullptr;
    static SoliumAudio *locked = nullptr;
    if (QCoreApplication::instance() == nullptr) {
        return nullptr;
    }
    SoliumAudio *&which = redacted ? locked : audio;
    if (which == nullptr) {
        which = new SoliumAudio(redacted);
        QQmlEngine::setObjectOwnership(which, QQmlEngine::CppOwnership);
    }
    return which;
}

extern "C" int solium_qml_set_audio_state(const char *json)
{
    SoliumAudio *audio = solium_audio(false);
    SoliumAudio *locked = solium_audio(true);
    if (audio == nullptr || locked == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    const QVariantMap state = document.object().toVariantMap();
    audio->setState(state);
    // The lock's reads the same volume, and no window's streams.
    locked->setState(state);
    return 1;
}
