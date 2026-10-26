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

/* Who a call on this endpoint names: the default, for a facade, so a call
 * made as the default changes lands on the new one; else itself, by id. */
QJsonValue target_of(const SoliumAudioEndpoint &endpoint)
{
    if (!endpoint.facade.isEmpty()) {
        return endpoint.facade;
    }
    return endpoint.id();
}

} // namespace

void SoliumAudioEndpoint::setVolume(double volume)
{
    solium_service_call(QStringLiteral("audio.volume"),
                        QJsonObject{{QStringLiteral("target"), target_of(*this)},
                                    {QStringLiteral("value"), volume}});
}

void SoliumAudioEndpoint::adjust(double by)
{
    solium_service_call(QStringLiteral("audio.volume"),
                        QJsonObject{{QStringLiteral("target"), target_of(*this)},
                                    {QStringLiteral("by"), by}});
}

void SoliumAudioEndpoint::setMuted(bool muted)
{
    solium_service_call(QStringLiteral("audio.mute"),
                        QJsonObject{{QStringLiteral("target"), target_of(*this)},
                                    {QStringLiteral("value"), muted}});
}

void SoliumAudioEndpoint::toggleMuted()
{
    solium_service_call(QStringLiteral("audio.mute"),
                        QJsonObject{{QStringLiteral("target"), target_of(*this)},
                                    {QStringLiteral("value"), QStringLiteral("toggle")}});
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

SoliumAudio::SoliumAudio(QObject *parent) : QObject(parent)
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
    for (Facade &facade : facades) {
        if (facade.changed) {
            facade.endpoint->announce();
        }
    }
    if (own) {
        emit stateChanged();
    }
}

SoliumAudio *solium_audio()
{
    static SoliumAudio *audio = nullptr;
    if (QCoreApplication::instance() == nullptr) {
        return nullptr;
    }
    if (audio == nullptr) {
        audio = new SoliumAudio();
        QQmlEngine::setObjectOwnership(audio, QQmlEngine::CppOwnership);
    }
    return audio;
}

extern "C" int solium_qml_set_audio_state(const char *json)
{
    SoliumAudio *audio = solium_audio();
    if (audio == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    audio->setState(document.object().toVariantMap());
    return 1;
}
