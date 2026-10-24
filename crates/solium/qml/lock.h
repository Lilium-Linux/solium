/*
 * Solium's own lock, as its scenes read it (#281): the `Lock` singleton, and
 * the two native items a lock scene builds its prompt from, `SecretField`
 * (under the shipped `SecretInput`) and `SecretKeypad`.
 *
 * **None of them holds the password, or any part of it.** It is typed into a
 * zeroised buffer in Rust (`lock_auth.rs`), and Qt is told only how many
 * characters it has, whether they are selected, whether the field takes
 * typing now, and that a keypad key was pressed -- never which. A key typed at
 * the lock never reaches a scene as a Qt event, and a press or a finger on a
 * keypad never reaches its scene at all: Rust hit-tests the pad itself
 * (`solium_qml_scene_keypad_at`) and keeps the press.
 *
 * **Nothing here can unlock.** `Lock`'s properties are read-only; its two
 * methods ask Rust to check what was typed, or to clear it, and carry nothing.
 * Only Rust ends the lock, on `solium-auth`'s yes.
 * `lock_auth::tests::nothing_in_qml_can_unlock_or_read_the_secret`.
 */
#ifndef SOLIUM_QML_LOCK_H
#define SOLIUM_QML_LOCK_H

#include <QtCore/QList>
#include <QtCore/QObject>
#include <QtCore/QPointF>
#include <QtCore/QPointer>
#include <QtCore/QString>
#include <QtCore/QVariantMap>
#include <QtQml/QQmlComponent>
#include <QtQuick/QQuickItem>

class QJsonObject;

/* `Lock`, in `import Solium`: the native lock's state, its counts, its line
 * of text and whose session it is. Read-only; `submit()` and `clear()` are
 * requests Rust takes once a loop (`solium_qml_lock_take_requests`). */
class SoliumLock : public QObject
{
    Q_OBJECT
    /* "idle", "prompting", "checking", "failed", "lockedOut", "unavailable"
     * or "unlocking". */
    Q_PROPERTY(QString state READ state NOTIFY stateChanged)
    /* Wrong passwords since the lock was taken. */
    Q_PROPERTY(int attempts READ attempts NOTIFY attemptsChanged)
    /* Whole seconds until the field takes a password again, while locked
     * out, or until the next try, while unavailable; 0 otherwise. */
    Q_PROPERTY(int retryAfter READ retryAfter NOTIFY retryAfterChanged)
    /* The line that goes with the state: "Wrong password", or the way out
     * while the check is unavailable. Empty otherwise. */
    Q_PROPERTY(QString message READ message NOTIFY messageChanged)
    /* `{ name, realName, avatar }`: the login name, the name from the passwd
     * entry, and a `file://` URL of AccountsService's picture, or "". */
    Q_PROPERTY(QVariantMap user READ user NOTIFY userChanged)
    /* The monitor the prompt is on: `lock.prompt_on`'s. */
    Q_PROPERTY(QString promptOn READ promptOn NOTIFY promptOnChanged)
    /* Whether the session is locked by Solium's own lock. False from the
     * unlock on, while the scene plays its exit (`state` reads "unlocking"),
     * and while the scene is kept for the next lock: a scene runs its Timers
     * and loops only while this is true. */
    Q_PROPERTY(bool locked READ locked NOTIFY lockedChanged)
    /* How long the exit lasts, in milliseconds: `lock.exit_ms`. The scene is
     * drawn over the desktop for this long after the unlock, and then it is
     * gone, whatever it is doing. */
    Q_PROPERTY(int exitMs READ exitMs NOTIFY exitMsChanged)
public:
    using QObject::QObject;
    QString state() const { return m_state; }
    int attempts() const { return m_attempts; }
    int retryAfter() const { return m_retry_after; }
    QString message() const { return m_message; }
    QVariantMap user() const { return m_user; }
    QString promptOn() const { return m_prompt_on; }
    bool locked() const { return m_locked; }
    int exitMs() const { return m_exit_ms; }

    /* Ask Rust to check what was typed. Carries nothing: there is nothing in
     * Qt to carry. */
    Q_INVOKABLE void submit();
    /* Ask Rust to clear what was typed. */
    Q_INVOKABLE void clear();

    void publish(const QJsonObject &next);
    /* The requests made since the last call: 1 for submit, 2 for clear. */
    int takeRequests();

signals:
    void stateChanged();
    void attemptsChanged();
    void retryAfterChanged();
    void messageChanged();
    void userChanged();
    void promptOnChanged();
    void lockedChanged();
    void exitMsChanged();

private:
    QString m_state = QStringLiteral("idle");
    int m_attempts = 0;
    int m_retry_after = 0;
    QString m_message;
    QVariantMap m_user;
    QString m_prompt_on;
    bool m_locked = false;
    int m_exit_ms = 0;
    int m_requests = 0;
};

/* One key of the keypad, as Rust lays it out: what its face says, the
 * letters under it, and its kind ("key", "erase" or "submit"). */
struct SoliumPadKey
{
    QString label;
    QString letters;
    QString kind;
    bool operator==(const SoliumPadKey &other) const
    {
        return label == other.label && letters == other.letters && kind == other.kind;
    }
};

/* What the field and the keypad show, shared by every one of them in every
 * scene. Not given to QML: the items read it and say what they may. */
class SoliumSecretState : public QObject
{
    Q_OBJECT
public:
    using QObject::QObject;
    int length = 0;
    bool selected = false;
    bool accepting = false;
    bool pressed = false;
    /* How many keypad keys have been pressed, ever: each one more is one
     * `keyPressed()`. */
    unsigned long long presses = 0;
    QString layout;
    QList<QList<SoliumPadKey>> rows;

    void publish(const QJsonObject &field, const QJsonObject &keypad);

signals:
    void fieldChanged();
    void pressedChanged();
    void keyPressed();
    void keypadChanged();
};

/* `SecretField`: the native half of a password field. It draws nothing; the
 * shipped `SecretInput` draws its dots and caret from these, and a scene of
 * its own may do the same. */
class SoliumSecretField : public QQuickItem
{
    Q_OBJECT
    /* How many characters have been typed. */
    Q_PROPERTY(int length READ length NOTIFY lengthChanged)
    /* Whether they are all selected, so the next key replaces them. */
    Q_PROPERTY(bool selected READ selected NOTIFY selectedChanged)
    /* Whether typing goes into the field now: not while checking, locked
     * out or unavailable. */
    Q_PROPERTY(bool accepting READ accepting NOTIFY acceptingChanged)
public:
    explicit SoliumSecretField(QQuickItem *parent = nullptr);
    int length() const;
    bool selected() const;
    bool accepting() const;

signals:
    void lengthChanged();
    void selectedChanged();
    void acceptingChanged();

private:
    void follow();
    int m_length = 0;
    bool m_selected = false;
    bool m_accepting = false;
};

/* `SecretKeypad`: a passcode pad whose keys are the scene's `delegate`, laid
 * out by Solium in the rows `lock.keypad` names. Each delegate is given
 * `label`, `letters`, `kind`, `row` and `column` once, when it is made; while
 * a key is held the pad is `pressed`, and each key pressed is one
 * `keyPressed()`, never saying which. */
class SoliumSecretKeypad : public QQuickItem
{
    Q_OBJECT
    Q_PROPERTY(QQmlComponent *delegate READ delegate WRITE setDelegate NOTIFY delegateChanged)
    /* "digits", "letters" or "keyboard", as `lock.keypad` says. */
    Q_PROPERTY(QString layout READ layout NOTIFY layoutChanged)
    Q_PROPERTY(bool pressed READ pressed NOTIFY pressedChanged)
public:
    explicit SoliumSecretKeypad(QQuickItem *parent = nullptr);
    ~SoliumSecretKeypad() override;
    QQmlComponent *delegate() const { return m_delegate; }
    void setDelegate(QQmlComponent *delegate);
    QString layout() const;
    bool pressed() const;

    /* The key at `local`, in this item's coordinates, counted row by row
     * from 0, or -1 for none. Asked by Rust only. */
    int keyAt(QPointF local) const;

signals:
    void delegateChanged();
    void layoutChanged();
    void pressedChanged();
    void keyPressed();

protected:
    void componentComplete() override;
    void geometryChange(const QRectF &next, const QRectF &old) override;

private:
    void rebuild();
    void place();
    QRectF cell(int row, int column) const;
    QQmlComponent *m_delegate = nullptr;
    QList<QPointer<QQuickItem>> m_keys;
    /* The rows the delegates were made for: what `keyAt` counts by. */
    QList<int> m_lengths;
};

SoliumLock *solium_lock();
SoliumSecretState *solium_secret_state();

/* The keypad key at `at`, in `root`'s scene coordinates, of the first visible
 * and enabled keypad found under `root`, or -1. */
int solium_keypad_at(QQuickItem *root, QPointF at);

#endif /* SOLIUM_QML_LOCK_H */
