/*
 * Quickshell's runtime types, for shell QML running inside the compositor.
 *
 * The Lilium shell is written against Quickshell, which hosts QML in its own
 * process. Here the compositor hosts it, so the types the shell instantiates
 * have to exist somewhere — and a `Process` cannot be faked in QML, because
 * QML has no way to start one.
 *
 * These are matched to what the shell actually uses, counted from its source
 * rather than from Quickshell's documentation: `Process` with `command`,
 * `stdout`, `stderr`, `onExited` and `write`; `StdioCollector` (35 uses) and
 * `SplitParser` (5) to read what comes back; `FileView` for the config files.
 *
 * Unlike host.cpp this needs moc: properties and signals are the entire point.
 */

#pragma once

#include <QtCore/QObject>
#include <QtCore/QProcess>
#include <QtCore/QStringList>
#include <QtCore/QTimer>
#include <QtCore/QVariantMap>
#include <QtNetwork/QLocalSocket>

/*
 * `stdout` and `stderr` are macros in glibc, and Quickshell names two of
 * Process's properties after them. Undefining them here is the price of
 * keeping the shell's own QML unedited, which is the whole point of a shim.
 */
#ifdef stdout
#undef stdout
#endif
#ifdef stderr
#undef stderr
#endif

/* Collects a stream in full and hands it over when it ends. */
class StdioCollector : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString text READ text NOTIFY textChanged)
    Q_PROPERTY(QString data READ text NOTIFY textChanged)

public:
    explicit StdioCollector(QObject *parent = nullptr) : QObject(parent) {}

    QString text() const { return m_text; }
    void append(const QString &chunk);
    void finish();

Q_SIGNALS:
    void textChanged();
    void streamFinished();

private:
    QString m_text;
};

/* Splits a stream on a marker and reports each piece as it arrives. */
class SplitParser : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString splitMarker READ splitMarker WRITE setSplitMarker NOTIFY splitMarkerChanged)

public:
    explicit SplitParser(QObject *parent = nullptr) : QObject(parent) {}

    QString splitMarker() const { return m_marker; }
    void setSplitMarker(const QString &marker);
    void append(const QString &chunk);
    void finish();

Q_SIGNALS:
    void splitMarkerChanged();
    void read(const QString &data);

private:
    QString m_marker = QStringLiteral("\n");
    QString m_buffer;
};

/* A child process. */
class Process : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QStringList command READ command WRITE setCommand NOTIFY commandChanged)
    Q_PROPERTY(bool running READ running WRITE setRunning NOTIFY runningChanged)
    Q_PROPERTY(QObject *stdout READ stdoutSink WRITE setStdoutSink NOTIFY stdoutChanged)
    Q_PROPERTY(QObject *stderr READ stderrSink WRITE setStderrSink NOTIFY stderrChanged)
    Q_PROPERTY(QVariantMap environment READ environment WRITE setEnvironment NOTIFY environmentChanged)
    Q_PROPERTY(QString workingDirectory READ workingDirectory WRITE setWorkingDirectory NOTIFY workingDirectoryChanged)
    Q_PROPERTY(int processId READ processId NOTIFY runningChanged)

public:
    explicit Process(QObject *parent = nullptr);

    QStringList command() const { return m_command; }
    void setCommand(const QStringList &command);

    bool running() const;
    void setRunning(bool running);

    QObject *stdoutSink() const { return m_stdout; }
    void setStdoutSink(QObject *sink);
    QObject *stderrSink() const { return m_stderr; }
    void setStderrSink(QObject *sink);

    QVariantMap environment() const { return m_environment; }
    void setEnvironment(const QVariantMap &environment);

    QString workingDirectory() const { return m_workingDirectory; }
    void setWorkingDirectory(const QString &directory);

    int processId() const;

    Q_INVOKABLE void exec(const QVariant &command = QVariant());
    Q_INVOKABLE void write(const QString &data);
    Q_INVOKABLE void signal(int number);

Q_SIGNALS:
    void commandChanged();
    void runningChanged();
    void stdoutChanged();
    void stderrChanged();
    void environmentChanged();
    void workingDirectoryChanged();
    void started();
    void exited(int exitCode, int exitStatus);

private:
    void start();
    void drain(QProcess::ProcessChannel channel, QObject *sink);

    QProcess *m_process;
    QStringList m_command;
    QObject *m_stdout = nullptr;
    QObject *m_stderr = nullptr;
    QVariantMap m_environment;
    QString m_workingDirectory;
};

/* A file, read and written from QML. */
class FileView : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString path READ path WRITE setPath NOTIFY pathChanged)
    Q_PROPERTY(bool watchChanges READ watchChanges WRITE setWatchChanges NOTIFY watchChangesChanged)
    Q_PROPERTY(bool printErrors MEMBER m_printErrors)
    Q_PROPERTY(bool preload MEMBER m_preload)
    Q_PROPERTY(bool blockLoading MEMBER m_blockLoading)
    Q_PROPERTY(bool atomicWrites MEMBER m_atomicWrites)
    Q_PROPERTY(bool blockWrites MEMBER m_blockWrites)

public:
    explicit FileView(QObject *parent = nullptr) : QObject(parent) {}

    QString path() const { return m_path; }
    void setPath(const QString &path);

    bool watchChanges() const { return m_watch; }
    void setWatchChanges(bool watch);

    Q_INVOKABLE QString text();
    Q_INVOKABLE void setText(const QString &text);
    Q_INVOKABLE void reload();
    Q_INVOKABLE void writeAdapter() {}

Q_SIGNALS:
    void pathChanged();
    void watchChangesChanged();
    void loaded();
    void loadFailed();
    /// Carries why, because the shell reports the reason to whoever asked.
    void saveFailed(const QString &reason);
    void saved();
    void fileChanged();

private:
    QString m_path;
    QString m_text;
    bool m_watch = false;
    bool m_printErrors = true;
    bool m_preload = true;
    bool m_blockLoading = false;
    bool m_atomicWrites = false;
    bool m_blockWrites = false;
    bool m_loaded = false;
};

/* A unix socket. */
class Socket : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString path READ path WRITE setPath NOTIFY pathChanged)
    Q_PROPERTY(bool connected READ connected WRITE setConnected NOTIFY connectedChanged)
    Q_PROPERTY(QObject *parser READ parser WRITE setParser NOTIFY parserChanged)

public:
    explicit Socket(QObject *parent = nullptr);

    QString path() const { return m_path; }
    void setPath(const QString &path);
    bool connected() const;
    void setConnected(bool connected);
    QObject *parser() const { return m_parser; }
    void setParser(QObject *parser);

    Q_INVOKABLE void write(const QString &data);
    Q_INVOKABLE void flush();

Q_SIGNALS:
    void pathChanged();
    void connectedChanged();
    void parserChanged();
    void connectionStateChanged();
    void error(const QString &message);

private:
    QLocalSocket *m_socket;
    QString m_path;
    QObject *m_parser = nullptr;
};

/* Quickshell's root singleton: the parts the shell calls. */
class QuickshellGlobal : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString configDir READ configDir CONSTANT)
    Q_PROPERTY(QString shellDir READ configDir CONSTANT)
    Q_PROPERTY(int processId READ processId CONSTANT)
    Q_PROPERTY(QString appId READ appId CONSTANT)
    Q_PROPERTY(QVariantList screens READ screens NOTIFY screensChanged)

public:
    explicit QuickshellGlobal(QObject *parent = nullptr) : QObject(parent) {}

    QString configDir() const;
    int processId() const;
    QVariantList screens() const { return m_screens; }
    /// What the shell calls this compositor's own surfaces, so it can tell
    /// them apart from application windows.
    QString appId() const { return QStringLiteral("solium"); }

    Q_INVOKABLE void execDetached(const QVariant &command);
    Q_INVOKABLE QString iconPath(const QString &name, const QVariant &check = QVariant());
    Q_INVOKABLE QString env(const QString &name) const;

Q_SIGNALS:
    void screensChanged();

private:
    QVariantList m_screens;
};

/* The compositor's windows, as the shell reads them.
 *
 * Backs both `ToplevelManager` (Quickshell.Wayland) and `Hyprland`, because
 * they are two names for the same question and Solium answers both from one
 * window list. Filled from Rust through `solium_qml_set_windows`.
 */
class Toplevels : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QVariant toplevels READ toplevels NOTIFY changed)
    Q_PROPERTY(QVariant activeToplevel READ activeToplevel NOTIFY changed)
    Q_PROPERTY(QVariant monitors READ monitors NOTIFY changed)
    Q_PROPERTY(QVariant workspaces READ workspaces NOTIFY changed)

public:
    explicit Toplevels(QObject *parent = nullptr);
    ~Toplevels() override;

    QVariant toplevels() const;
    QVariant activeToplevel() const;
    QVariant monitors() const;
    QVariant workspaces() const;

    Q_INVOKABLE QVariant monitorFor(const QVariant &screen) const;
    Q_INVOKABLE void dispatch(const QString &command);
    Q_INVOKABLE void refreshToplevels() {}

    /* Called from the compositor when the window list changes. */
    static void update(const QByteArray &json);

Q_SIGNALS:
    void changed();

private:
    static QVariantList s_windows;
    static QVariant s_active;
};

/* Registers all of the above. Called once, before any scene is loaded. */
void solium_qml_register_compat();
