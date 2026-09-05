/*
 * See compat.h. Matched to what the Lilium shell actually calls, counted from
 * its source.
 */

#include "compat.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QDir>
#include <QtCore/QFile>
#include <QtCore/QFileSystemWatcher>
#include <QtCore/QTextStream>
#include <QtGui/QIcon>
#include <QtQml/QQmlEngine>
#include <QtQml/qqml.h>

#include <csignal>

void StdioCollector::append(const QString &chunk)
{
    if (chunk.isEmpty()) {
        return;
    }
    m_text += chunk;
    Q_EMIT textChanged();
}

void StdioCollector::finish()
{
    Q_EMIT streamFinished();
}

void SplitParser::setSplitMarker(const QString &marker)
{
    if (marker == m_marker) {
        return;
    }
    m_marker = marker;
    Q_EMIT splitMarkerChanged();
}

void SplitParser::append(const QString &chunk)
{
    m_buffer += chunk;
    if (m_marker.isEmpty()) {
        return;
    }
    // Everything before the last marker is complete; whatever follows is a
    // partial line and waits for more.
    int at = 0;
    while ((at = m_buffer.indexOf(m_marker)) >= 0) {
        Q_EMIT read(m_buffer.left(at));
        m_buffer.remove(0, at + m_marker.size());
    }
}

void SplitParser::finish()
{
    // A stream that ends without a trailing marker still ended with data.
    if (!m_buffer.isEmpty()) {
        Q_EMIT read(m_buffer);
        m_buffer.clear();
    }
}

Process::Process(QObject *parent) : QObject(parent), m_process(new QProcess(this))
{
    QObject::connect(m_process, &QProcess::started, this, [this]() {
        Q_EMIT runningChanged();
        Q_EMIT started();
    });

    QObject::connect(m_process, &QProcess::readyReadStandardOutput, this,
                     [this]() { drain(QProcess::StandardOutput, m_stdout); });
    QObject::connect(m_process, &QProcess::readyReadStandardError, this,
                     [this]() { drain(QProcess::StandardError, m_stderr); });

    QObject::connect(m_process, &QProcess::finished, this,
                     [this](int code, QProcess::ExitStatus status) {
                         // Drained before the signal: a handler that reads
                         // `stdout.text` on exit must see everything, and the
                         // last chunk often arrives with the exit itself.
                         drain(QProcess::StandardOutput, m_stdout);
                         drain(QProcess::StandardError, m_stderr);
                         if (auto *collector = qobject_cast<StdioCollector *>(m_stdout)) {
                             collector->finish();
                         }
                         if (auto *parser = qobject_cast<SplitParser *>(m_stdout)) {
                             parser->finish();
                         }
                         if (auto *collector = qobject_cast<StdioCollector *>(m_stderr)) {
                             collector->finish();
                         }
                         Q_EMIT runningChanged();
                         Q_EMIT exited(code, static_cast<int>(status));
                     });
}

void Process::drain(QProcess::ProcessChannel channel, QObject *sink)
{
    if (sink == nullptr) {
        return;
    }
    m_process->setReadChannel(channel);
    const QString chunk = QString::fromUtf8(channel == QProcess::StandardOutput
                                                ? m_process->readAllStandardOutput()
                                                : m_process->readAllStandardError());
    if (chunk.isEmpty()) {
        return;
    }
    if (auto *collector = qobject_cast<StdioCollector *>(sink)) {
        collector->append(chunk);
    } else if (auto *parser = qobject_cast<SplitParser *>(sink)) {
        parser->append(chunk);
    }
}

void Process::setCommand(const QStringList &command)
{
    if (command == m_command) {
        return;
    }
    m_command = command;
    Q_EMIT commandChanged();
}

bool Process::running() const
{
    return m_process->state() != QProcess::NotRunning;
}

void Process::setRunning(bool running)
{
    if (running == this->running()) {
        return;
    }
    if (running) {
        start();
    } else {
        m_process->terminate();
    }
}

int Process::processId() const
{
    return static_cast<int>(m_process->processId());
}

void Process::setStdoutSink(QObject *sink)
{
    if (sink == m_stdout) {
        return;
    }
    m_stdout = sink;
    Q_EMIT stdoutChanged();
}

void Process::setStderrSink(QObject *sink)
{
    if (sink == m_stderr) {
        return;
    }
    m_stderr = sink;
    Q_EMIT stderrChanged();
}

void Process::setEnvironment(const QVariantMap &environment)
{
    m_environment = environment;
    Q_EMIT environmentChanged();
}

void Process::setWorkingDirectory(const QString &directory)
{
    if (directory == m_workingDirectory) {
        return;
    }
    m_workingDirectory = directory;
    Q_EMIT workingDirectoryChanged();
}

void Process::start()
{
    if (m_command.isEmpty()) {
        qWarning("Process started with no command");
        return;
    }
    if (!m_workingDirectory.isEmpty()) {
        m_process->setWorkingDirectory(m_workingDirectory);
    }
    if (!m_environment.isEmpty()) {
        auto environment = QProcessEnvironment::systemEnvironment();
        for (auto entry = m_environment.constBegin(); entry != m_environment.constEnd(); ++entry) {
            environment.insert(entry.key(), entry.value().toString());
        }
        m_process->setProcessEnvironment(environment);
    }

    const QString program = m_command.first();
    const QStringList arguments = m_command.mid(1);
    m_process->start(program, arguments);
}

void Process::exec(const QVariant &command)
{
    if (command.isValid()) {
        if (command.canConvert<QStringList>()) {
            setCommand(command.toStringList());
        } else if (command.canConvert<QVariantMap>()) {
            // The object form: { command: [...], environment: {...} }
            const auto map = command.toMap();
            if (map.contains(QStringLiteral("command"))) {
                setCommand(map.value(QStringLiteral("command")).toStringList());
            }
            if (map.contains(QStringLiteral("environment"))) {
                setEnvironment(map.value(QStringLiteral("environment")).toMap());
            }
            if (map.contains(QStringLiteral("workingDirectory"))) {
                setWorkingDirectory(map.value(QStringLiteral("workingDirectory")).toString());
            }
        }
    }
    start();
}

void Process::write(const QString &data)
{
    if (running()) {
        m_process->write(data.toUtf8());
    }
}

void Process::signal(int number)
{
    const auto pid = m_process->processId();
    if (pid > 0) {
        ::kill(static_cast<pid_t>(pid), number);
    }
}

void FileView::setPath(const QString &path)
{
    if (path == m_path) {
        return;
    }
    m_path = path;
    m_loaded = false;
    Q_EMIT pathChanged();
    reload();
}

void FileView::setWatchChanges(bool watch)
{
    if (watch == m_watch) {
        return;
    }
    m_watch = watch;
    Q_EMIT watchChangesChanged();
}

void FileView::reload()
{
    QFile file(m_path);
    if (!file.open(QIODevice::ReadOnly | QIODevice::Text)) {
        if (m_printErrors) {
            qWarning("FileView could not read %s", qPrintable(m_path));
        }
        Q_EMIT loadFailed();
        return;
    }
    m_text = QString::fromUtf8(file.readAll());
    m_loaded = true;
    Q_EMIT loaded();
}

QString FileView::text()
{
    if (!m_loaded && !m_path.isEmpty()) {
        reload();
    }
    return m_text;
}

void FileView::setText(const QString &text)
{
    m_text = text;
    QFile file(m_path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate | QIODevice::Text)) {
        if (m_printErrors) {
            qWarning("FileView could not write %s", qPrintable(m_path));
        }
        return;
    }
    file.write(text.toUtf8());
}

QString QuickshellGlobal::configDir() const
{
    const QByteArray fromEnv = qgetenv("SOLIUM_SHELL_DIR");
    if (!fromEnv.isEmpty()) {
        return QString::fromUtf8(fromEnv);
    }
    return QDir::homePath() + QStringLiteral("/.config/solium");
}

int QuickshellGlobal::processId() const
{
    return static_cast<int>(QCoreApplication::applicationPid());
}

void QuickshellGlobal::execDetached(const QVariant &command)
{
    QStringList parts;
    if (command.canConvert<QStringList>()) {
        parts = command.toStringList();
    } else if (command.canConvert<QVariantMap>()) {
        parts = command.toMap().value(QStringLiteral("command")).toStringList();
    }
    if (parts.isEmpty()) {
        qWarning("execDetached called with no command");
        return;
    }
    QProcess::startDetached(parts.first(), parts.mid(1));
}

QString QuickshellGlobal::iconPath(const QString &name, const QVariant &check)
{
    const QIcon icon = QIcon::fromTheme(name);
    if (icon.isNull()) {
        // The checking form answers with nothing rather than a broken path, so
        // callers can fall back.
        return check.toBool() ? QString() : name;
    }
    return QStringLiteral("image://theme/") + name;
}

QString QuickshellGlobal::env(const QString &name) const
{
    return QString::fromUtf8(qgetenv(name.toUtf8().constData()));
}

void solium_qml_register_compat()
{
    // Registered by hand rather than through the CMake type registrar, because
    // this builds with cc and moc alone.
    qmlRegisterType<Process>("Quickshell.Io", 1, 0, "Process");
    qmlRegisterType<StdioCollector>("Quickshell.Io", 1, 0, "StdioCollector");
    qmlRegisterType<SplitParser>("Quickshell.Io", 1, 0, "SplitParser");
    qmlRegisterType<FileView>("Quickshell.Io", 1, 0, "FileView");

    // The shell reaches these through the root module too.
    qmlRegisterType<Process>("Quickshell", 1, 0, "Process");
    qmlRegisterType<StdioCollector>("Quickshell", 1, 0, "StdioCollector");
    qmlRegisterType<SplitParser>("Quickshell", 1, 0, "SplitParser");
    qmlRegisterType<FileView>("Quickshell", 1, 0, "FileView");

    qmlRegisterSingletonType<QuickshellGlobal>(
        "Quickshell", 1, 0, "Quickshell",
        [](QQmlEngine *, QJSEngine *) -> QObject * { return new QuickshellGlobal(); });
}
