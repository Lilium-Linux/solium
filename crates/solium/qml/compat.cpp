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
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>
#include <QtGui/QIcon>
#include <QtQml/QQmlEngine>
#include <QtQuick/QQuickImageProvider>
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
        // The reason, not just the fact: the shell shows it to whoever asked
        // for the save.
        Q_EMIT saveFailed(file.errorString());
        return;
    }
    if (file.write(text.toUtf8()) < 0) {
        Q_EMIT saveFailed(file.errorString());
        return;
    }
    file.close();
    Q_EMIT saved();
}

Socket::Socket(QObject *parent) : QObject(parent), m_socket(new QLocalSocket(this))
{
    QObject::connect(m_socket, &QLocalSocket::connected, this, [this]() {
        Q_EMIT connectedChanged();
        Q_EMIT connectionStateChanged();
    });
    QObject::connect(m_socket, &QLocalSocket::disconnected, this, [this]() {
        Q_EMIT connectedChanged();
        Q_EMIT connectionStateChanged();
    });
    QObject::connect(m_socket, &QLocalSocket::readyRead, this, [this]() {
        if (m_parser == nullptr) {
            m_socket->readAll();
            return;
        }
        const QString chunk = QString::fromUtf8(m_socket->readAll());
        if (auto *collector = qobject_cast<StdioCollector *>(m_parser)) {
            collector->append(chunk);
        } else if (auto *parser = qobject_cast<SplitParser *>(m_parser)) {
            parser->append(chunk);
        }
    });
}

void Socket::setPath(const QString &path)
{
    if (path == m_path) {
        return;
    }
    m_path = path;
    Q_EMIT pathChanged();
}

bool Socket::connected() const
{
    return m_socket->state() == QLocalSocket::ConnectedState;
}

void Socket::setConnected(bool connected)
{
    if (connected == this->connected()) {
        return;
    }
    if (connected) {
        if (m_path.isEmpty()) {
            Q_EMIT error(QStringLiteral("no path"));
            return;
        }
        m_socket->connectToServer(m_path);
    } else {
        m_socket->disconnectFromServer();
    }
}

void Socket::setParser(QObject *parser)
{
    if (parser == m_parser) {
        return;
    }
    m_parser = parser;
    Q_EMIT parserChanged();
}

void Socket::write(const QString &data)
{
    if (connected()) {
        m_socket->write(data.toUtf8());
    }
}

void Socket::flush()
{
    m_socket->flush();
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
    if (QIcon::hasThemeIcon(name)) {
        return QStringLiteral("image://theme/") + name;
    }
    // The checking form answers with nothing rather than a broken path, so
    // callers can fall back to something they know exists.
    return check.toBool() ? QString() : QStringLiteral("image://theme/") + name;
}

QString QuickshellGlobal::env(const QString &name) const
{
    return QString::fromUtf8(qgetenv(name.toUtf8().constData()));
}

QVariantList Toplevels::s_windows;
QVariant Toplevels::s_active;

namespace {
/* Every live instance, so an update can tell them all. */
QList<Toplevels *> &liveToplevels()
{
    static QList<Toplevels *> instances;
    return instances;
}
} // namespace

Toplevels::Toplevels(QObject *parent) : QObject(parent)
{
    liveToplevels().append(this);
}

Toplevels::~Toplevels()
{
    liveToplevels().removeAll(this);
}

QVariant Toplevels::toplevels() const
{
    // The shell reads `toplevels.values`, which is how Quickshell's object
    // models present themselves.
    QVariantMap model;
    model.insert(QStringLiteral("values"), s_windows);
    return model;
}

QVariant Toplevels::activeToplevel() const
{
    return s_active;
}

QVariant Toplevels::monitors() const
{
    QVariantMap model;
    model.insert(QStringLiteral("values"), QVariantList());
    return model;
}

QVariant Toplevels::workspaces() const
{
    QVariantMap model;
    model.insert(QStringLiteral("values"), QVariantList());
    return model;
}

QVariant Toplevels::monitorFor(const QVariant &) const
{
    return {};
}

void Toplevels::dispatch(const QString &command)
{
    // Hyprland's dispatchers have Solium equivalents, but they are bindings
    // rather than a command language. Logged rather than silently dropped, so
    // a shell action that does nothing says why.
    qWarning("Hyprland.dispatch(%s) has no Solium mapping yet", qPrintable(command));
}

void Toplevels::update(const QByteArray &json)
{
    QJsonParseError parsed{};
    const auto document = QJsonDocument::fromJson(json, &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return;
    }
    const auto object = document.object();
    s_windows = object.value(QStringLiteral("windows")).toArray().toVariantList();
    const auto active = object.value(QStringLiteral("active"));
    s_active = active.isNull() ? QVariant() : active.toObject().toVariantMap();

    for (auto *instance : liveToplevels()) {
        Q_EMIT instance->changed();
    }
}

extern "C" void solium_qml_set_windows(const char *json)
{
    if (json != nullptr) {
        Toplevels::update(QByteArray(json));
    }
}

/* Serves icons from the icon theme to QML, so `image://theme/firefox` works.
 *
 * `iconPath` answers with such a URL, and without a provider behind it every
 * icon in the shell is a broken image. */
class ThemeIconProvider : public QQuickImageProvider
{
public:
    ThemeIconProvider() : QQuickImageProvider(QQuickImageProvider::Pixmap) {}

    QPixmap requestPixmap(const QString &id, QSize *size, const QSize &requested) override
    {
        const int width = requested.width() > 0 ? requested.width() : 64;
        const int height = requested.height() > 0 ? requested.height() : 64;
        QIcon icon = QIcon::fromTheme(id);
        if (icon.isNull()) {
            icon = QIcon::fromTheme(QStringLiteral("application-x-executable"));
        }
        QPixmap pixmap = icon.pixmap(QSize(width, height));
        if (size != nullptr) {
            *size = pixmap.size();
        }
        return pixmap;
    }
};

void solium_qml_install_icons(QQmlEngine *engine)
{
    engine->addImageProvider(QStringLiteral("theme"), new ThemeIconProvider());
}

void solium_qml_register_compat()
{
    // Registered by hand rather than through the CMake type registrar, because
    // this builds with cc and moc alone.
    qmlRegisterType<Process>("Quickshell.Io", 1, 0, "Process");
    qmlRegisterType<StdioCollector>("Quickshell.Io", 1, 0, "StdioCollector");
    qmlRegisterType<SplitParser>("Quickshell.Io", 1, 0, "SplitParser");
    qmlRegisterType<FileView>("Quickshell.Io", 1, 0, "FileView");
    qmlRegisterType<Socket>("Quickshell.Io", 1, 0, "Socket");

    // The shell reaches these through the root module too.
    qmlRegisterType<Process>("Quickshell", 1, 0, "Process");
    qmlRegisterType<StdioCollector>("Quickshell", 1, 0, "StdioCollector");
    qmlRegisterType<SplitParser>("Quickshell", 1, 0, "SplitParser");
    qmlRegisterType<FileView>("Quickshell", 1, 0, "FileView");
    qmlRegisterType<Socket>("Quickshell", 1, 0, "Socket");

    qmlRegisterSingletonType<Toplevels>(
        "Quickshell.Wayland", 1, 0, "ToplevelManager",
        [](QQmlEngine *, QJSEngine *) -> QObject * { return new Toplevels(); });
    qmlRegisterSingletonType<Toplevels>(
        "Quickshell.Hyprland", 1, 0, "Hyprland",
        [](QQmlEngine *, QJSEngine *) -> QObject * { return new Toplevels(); });

    qmlRegisterSingletonType<QuickshellGlobal>(
        "Quickshell", 1, 0, "Quickshell",
        [](QQmlEngine *, QJSEngine *) -> QObject * { return new QuickshellGlobal(); });
}
