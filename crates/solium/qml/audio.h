/*
 * `Audio`: the default output and input, every sink and source, and every
 * application's playback stream, for scenes (03 §3.2.5, #347, #416).
 *
 * `outputs`, `inputs` and `streams` are keyed row models fed by Rust like
 * every other model. `output` and `input` are facades that are never null:
 * they follow the effective default, so a binding to `Audio.output.volume`
 * survives a switch to headphones. `forWindow(id)` is a facade of the same
 * kind for one window's streams, set together. The calls a scene makes on
 * an endpoint go to Rust, which writes them to PipeWire, coalesced, and
 * tells Lua who asked.
 */
#ifndef SOLIUM_QML_AUDIO_H
#define SOLIUM_QML_AUDIO_H

#include "rows.h"

#include <QtCore/QHash>
#include <QtCore/QObject>
#include <QtCore/QPointer>
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
    /* For `Audio.forWindow`'s facade, the window whose streams it acts on,
     * all of them; 0 otherwise. */
    quint64 window = 0;
signals:
    void changed();
};

/* One application's playback stream: a row of `streams`, or the facade
 * `forWindow` hands out for one window's streams as one (#416). */
class SoliumAudioStream : public SoliumAudioEndpoint
{
    Q_OBJECT
    Q_PROPERTY(QString app READ app NOTIFY changed)
    Q_PROPERTY(QString appId READ appId NOTIFY changed)
    Q_PROPERTY(QString binary READ binary NOTIFY changed)
    Q_PROPERTY(int pid READ pid NOTIFY changed)
    Q_PROPERTY(QString mediaName READ mediaName NOTIFY changed)
    Q_PROPERTY(QVariantList windows READ windows NOTIFY changed)
    Q_PROPERTY(QVariantList streams READ streams NOTIFY changed)
    Q_PROPERTY(int count READ count NOTIFY changed)
public:
    using SoliumAudioEndpoint::SoliumAudioEndpoint;
    QString app() const { return value("app").toString(); }
    QString appId() const { return value("appId").toString(); }
    QString binary() const { return value("binary").toString(); }
    int pid() const { return value("pid").toInt(); }
    QString mediaName() const { return value("mediaName").toString(); }
    QVariantList windows() const { return value("windows").toList(); }
    /* The facade's streams, by id; a row is its own one. */
    QVariantList streams() const
    {
        return window != 0 ? value("streams").toList() : QVariantList{id()};
    }
    int count() const { return window != 0 ? value("count").toInt() : 1; }
};

class SoliumAudioRows : public SoliumRows
{
    Q_OBJECT
public:
    SoliumAudioRows();
};

class SoliumAudioStreamRows : public SoliumRows
{
    Q_OBJECT
public:
    SoliumAudioStreamRows();
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
    Q_PROPERTY(QObject *streams READ streams CONSTANT)
public:
    explicit SoliumAudio(bool redacted, QObject *parent = nullptr);
    bool available() const { return m_available; }
    bool ready() const { return m_ready; }
    QObject *output() { return &m_output; }
    QObject *input() { return &m_input; }
    QObject *outputs() const;
    QObject *inputs() const;
    QObject *streams() const;
    /* One window's playback streams as one stream, never null: `present`
     * while the window has any, its volume the loudest of theirs, `muted`
     * while every one is, and every call acting on all of them. By the
     * window's id, as `Windows` and a frame's `windowId` give it. The same
     * object for the same window while a scene holds it. */
    Q_INVOKABLE QObject *forWindow(int id);
    void setState(const QVariantMap &state);
signals:
    void stateChanged();

private:
    bool m_redacted;
    bool m_available = false;
    bool m_ready = false;
    SoliumAudioEndpoint m_output;
    SoliumAudioEndpoint m_input;
    /* Every window facade a scene still holds, by window, and the windows'
     * rows they were last given. */
    QHash<quint64, QPointer<SoliumAudioStream>> m_windows;
    QVariantMap m_window_rows;
    void assign(SoliumAudioStream *facade) const;
};

/* The `Audio` a scene names: the session's, or, `redacted`, the lock's,
 * which has the volume and no application's stream (#416). */
SoliumAudio *solium_audio(bool redacted = false);

#endif
