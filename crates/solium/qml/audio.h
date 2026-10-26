/*
 * `Audio`: the default output and input, and every sink and source, for
 * scenes (03 §3.2.5, #347).
 *
 * `outputs` and `inputs` are keyed row models fed by Rust like every other
 * model. `output` and `input` are facades that are never null: they follow
 * the effective default, so a binding to `Audio.output.volume` survives a
 * switch to headphones. The calls a scene makes on an endpoint go to Rust,
 * which writes them to PipeWire, coalesced, and tells Lua who asked.
 */
#ifndef SOLIUM_QML_AUDIO_H
#define SOLIUM_QML_AUDIO_H

#include "rows.h"

#include <QtCore/QObject>
#include <QtCore/QVariant>

/* One sink or source: a row of `outputs` or `inputs`, or the facade behind
 * `output` and `input`. */
class SoliumAudioEndpoint : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(int id READ id NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(QString label READ label NOTIFY changed)
    Q_PROPERTY(QString kind READ kind NOTIFY changed)
    Q_PROPERTY(QString icon READ icon NOTIFY changed)
    Q_PROPERTY(double volume READ volume NOTIFY changed)
    Q_PROPERTY(bool muted READ muted NOTIFY changed)
    Q_PROPERTY(bool pending READ pending NOTIFY changed)
    Q_PROPERTY(double step READ step NOTIFY changed)
    Q_PROPERTY(double max READ max NOTIFY changed)
    Q_PROPERTY(bool isDefault READ isDefault NOTIFY changed)
    Q_PROPERTY(QString portLabel READ portLabel NOTIFY changed)
    Q_PROPERTY(QString portKind READ portKind NOTIFY changed)
    Q_PROPERTY(QString state READ state NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    int id() const { return value("id").toInt(); }
    QString name() const { return value("name").toString(); }
    QString label() const { return value("label").toString(); }
    QString kind() const { return value("kind").toString(); }
    QString icon() const { return value("icon").toString(); }
    double volume() const { return value("volume").toDouble(); }
    bool muted() const { return value("muted").toBool(); }
    bool pending() const { return value("pending").toBool(); }
    double step() const { return value("step").toDouble(); }
    double max() const { return value("max").toDouble(); }
    bool isDefault() const { return value("isDefault").toBool(); }
    QString portLabel() const { return value("portLabel").toString(); }
    QString portKind() const { return value("portKind").toString(); }
    QString state() const { return value("state").toString(); }
    void announce() override { emit changed(); }

    /* Perceptual, as `wpctl` shows it, clamped to `max`. A slider may call
     * this on every move: Solium sends one write at a time, the newest. */
    Q_INVOKABLE void setVolume(double volume);
    /* Up or down by `by`, rounded to `step`. */
    Q_INVOKABLE void adjust(double by);
    Q_INVOKABLE void setMuted(bool muted);
    Q_INVOKABLE void toggleMuted();
    /* Make it the default. */
    Q_INVOKABLE void makeDefault();

    /* `"output"` or `"input"` for the facades, which act on whatever is the
     * default when called; empty for a row, which acts on itself. */
    QString facade;
signals:
    void changed();
};

class SoliumAudioRows : public SoliumRows
{
    Q_OBJECT
public:
    SoliumAudioRows();
};

/* `Audio`, as a scene names it. */
class SoliumAudio : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool available READ available NOTIFY stateChanged)
    Q_PROPERTY(bool ready READ ready NOTIFY stateChanged)
    Q_PROPERTY(QObject *output READ output CONSTANT)
    Q_PROPERTY(QObject *input READ input CONSTANT)
    Q_PROPERTY(QObject *outputs READ outputs CONSTANT)
    Q_PROPERTY(QObject *inputs READ inputs CONSTANT)
public:
    explicit SoliumAudio(QObject *parent = nullptr);
    bool available() const { return m_available; }
    bool ready() const { return m_ready; }
    QObject *output() { return &m_output; }
    QObject *input() { return &m_input; }
    QObject *outputs() const;
    QObject *inputs() const;
    void setState(const QVariantMap &state);
signals:
    void stateChanged();

private:
    bool m_available = false;
    bool m_ready = false;
    SoliumAudioEndpoint m_output;
    SoliumAudioEndpoint m_input;
};

SoliumAudio *solium_audio();

#endif
