/*
 * Keyed rows, as a list model.
 *
 * One SoliumRows per model, each holding rows of one SoliumRow type. Rust
 * diffs the rows and sends the steps (`crate::models::diff`); this applies
 * them in order, writes every row's new values first, and only then announces
 * each row it touched, once.
 * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
 */
#ifndef SOLIUM_QML_ROWS_H
#define SOLIUM_QML_ROWS_H

#include <QtCore/QAbstractListModel>
#include <QtCore/QByteArray>
#include <QtCore/QHash>
#include <QtCore/QJsonArray>
#include <QtCore/QSortFilterProxyModel>
#include <QtCore/QVariant>
#include <QtCore/QVector>

#include "attached.h"

class SoliumRows : public QAbstractListModel
{
    Q_OBJECT
    Q_PROPERTY(int count READ count NOTIFY countChanged)
public:
    using Make = SoliumRow *(*)(QObject *parent);
    enum class Retire {
        /* A row that goes is kept, absent, so its key coming back finds the
         * same row object again.
         * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`. */
        Never,
        AfterGrace,
    };

    SoliumRows(const QMetaObject *row_type, Make make, Retire retire, const char *key_role,
               QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;
    int count() const { return static_cast<int>(m_rows.size()); }

    /* The row for `key`, live or gone; with Retire::Never, until one is
     * published, an absent row carrying the key, made on first ask.
     * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
    SoliumRow *row_for(const QString &key);
    Q_INVOKABLE QObject *get(const QVariant &key);
    /* Apply one batch whole. False, with nothing applied, when any step does
     * not match the rows held, as an insert of a row already held does not.
     * `qml::hosted::tests::a_batch_that_does_not_match_the_rows_held_is_refused`,
     * `qml::hosted::tests::a_refused_batch_takes_none_of_its_steps`. */
    bool apply(const QJsonArray &ops);
    const QVector<SoliumRow *> &rows() const { return m_rows; }

signals:
    void countChanged();
    void applied();

private:
    QString key_of(const SoliumRow *row) const;
    /* Whether every step matches the rows held, each step taken in turn on a
     * copy of their keys.
     * `qml::hosted::tests::a_refused_batch_takes_none_of_its_steps`. */
    bool fits(const QJsonArray &ops) const;
    void retire(SoliumRow *row);

    QHash<int, QByteArray> m_roles;
    QVector<SoliumRow *> m_rows;
    QHash<QString, SoliumRow *> m_by_key;
    Make m_make;
    Retire m_retire;
    QByteArray m_key_role;
    SoliumRow *m_absent = nullptr;
};

/* A window's row: one row of `Windows`, and what `Windows.get(id)` and
 * `Windows.focused` answer. A window that goes reads `present` and `valid`
 * false (Ruling 17).
 * `qml::hosted::tests::the_windows_model_filters_sorts_and_keeps_its_facades`,
 * `state::tests::real_client::reflow_on_close::hosted::a_window_row_carries_where_it_lives_and_its_focus`. */
class SoliumWindow : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(int id READ id NOTIFY changed)
    Q_PROPERTY(QString title READ title NOTIFY changed)
    Q_PROPERTY(QString appId READ appId NOTIFY changed)
    Q_PROPERTY(int pid READ pid NOTIFY changed)
    Q_PROPERTY(bool xwayland READ xwayland NOTIFY changed)
    Q_PROPERTY(QString monitor READ monitor NOTIFY changed)
    Q_PROPERTY(QString workspace READ workspace NOTIFY changed)
    Q_PROPERTY(bool focused READ focused NOTIFY changed)
    Q_PROPERTY(int focusOrder READ focusOrder NOTIFY changed)
    Q_PROPERTY(bool urgent READ urgent NOTIFY changed)
    Q_PROPERTY(bool fullscreen READ fullscreen NOTIFY changed)
    Q_PROPERTY(bool maximized READ maximized NOTIFY changed)
    Q_PROPERTY(bool modal READ modal NOTIFY changed)
    Q_PROPERTY(int parent READ parentWindow NOTIFY changed)
    Q_PROPERTY(QString state READ state NOTIFY changed)
    Q_PROPERTY(bool onStage READ onStage NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    int id() const { return value("id").toInt(); }
    QString title() const { return value("title").toString(); }
    QString appId() const { return value("appId").toString(); }
    int pid() const { return value("pid").toInt(); }
    bool xwayland() const { return value("xwayland").toBool(); }
    QString monitor() const { return value("monitor").toString(); }
    QString workspace() const { return value("workspace").toString(); }
    bool focused() const { return value("focused").toBool(); }
    int focusOrder() const { return value("focusOrder").toInt(); }
    bool urgent() const { return value("urgent").toBool(); }
    bool fullscreen() const { return value("fullscreen").toBool(); }
    bool maximized() const { return value("maximized").toBool(); }
    bool modal() const { return value("modal").toBool(); }
    int parentWindow() const { return value("parent").toInt(); }
    QString state() const { return value("state").toString(); }
    bool onStage() const { return value("onStage").toBool(); }
    void announce() override { emit changed(); }
signals:
    void changed();
};

/* `Windows`: every window, and `Windows.focused`, a facade that is never null
 * and follows focus.
 * `qml::hosted::tests::the_windows_model_filters_sorts_and_keeps_its_facades`. */
class SoliumWindowRows : public SoliumRows
{
    Q_OBJECT
    Q_PROPERTY(SoliumWindow *focused READ focused CONSTANT)
public:
    SoliumWindowRows();
    SoliumWindow *focused() { return &m_focused; }

private:
    void follow();
    SoliumWindow m_focused;
};

/* `WindowList { monitor; workspace; app; onStage; sort: "mru" }`: a filtered,
 * sorted view of `Windows`, never reset. An empty filter, or an `onStage`
 * left unset, keeps every window; `sort` is `""`, the order windows opened,
 * or `"mru"`, the most recently focused first (Ruling 18).
 * `qml::hosted::tests::the_windows_model_filters_sorts_and_keeps_its_facades`. */
class SoliumWindowList : public QSortFilterProxyModel
{
    Q_OBJECT
    Q_PROPERTY(QString monitor READ monitor WRITE setMonitor NOTIFY changed)
    Q_PROPERTY(QString workspace READ workspace WRITE setWorkspace NOTIFY changed)
    Q_PROPERTY(QString app READ app WRITE setApp NOTIFY changed)
    Q_PROPERTY(QVariant onStage READ onStage WRITE setOnStage NOTIFY changed)
    Q_PROPERTY(QString sort READ sortBy WRITE setSortBy NOTIFY changed)
    Q_PROPERTY(int count READ count NOTIFY countChanged)
public:
    explicit SoliumWindowList(QObject *parent = nullptr);
    QString monitor() const { return m_monitor; }
    void setMonitor(const QString &value) { refilter(m_monitor, value); }
    QString workspace() const { return m_workspace; }
    void setWorkspace(const QString &value) { refilter(m_workspace, value); }
    QString app() const { return m_app; }
    void setApp(const QString &value) { refilter(m_app, value); }
    QVariant onStage() const { return m_on_stage; }
    void setOnStage(const QVariant &value);
    QString sortBy() const { return m_sort; }
    void setSortBy(const QString &value);
    int count() const { return rowCount(); }
signals:
    void changed();
    void countChanged();

protected:
    bool filterAcceptsRow(int source_row, const QModelIndex &source_parent) const override;
    bool lessThan(const QModelIndex &left, const QModelIndex &right) const override;

private:
    template <typename Change> void refilterWith(Change &&change);
    void refilter(QString &field, const QString &value);
    QString m_monitor, m_workspace, m_app, m_sort;
    QVariant m_on_stage;
};

/* A workspace's row: one row of `Workspaces`, and what its facades answer.
 * Its key is `<group>/<id>`. A workspace no longer declared reads `present`
 * and `valid` false (Ruling 17).
 * `qml::hosted::tests::the_workspaces_model_its_list_and_its_facades`,
 * `state::tests::real_client::reflow_on_close::hosted::workspace_rows_count_their_windows_and_say_which_is_shown`. */
class SoliumWorkspace : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(QString key READ key NOTIFY changed)
    Q_PROPERTY(QString id READ id NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(int col READ col NOTIFY changed)
    Q_PROPERTY(int row READ row NOTIFY changed)
    Q_PROPERTY(QString group READ group NOTIFY changed)
    Q_PROPERTY(QStringList monitors READ monitors NOTIFY changed)
    Q_PROPERTY(bool active READ active NOTIFY changed)
    Q_PROPERTY(bool focused READ focused NOTIFY changed)
    Q_PROPERTY(int occupied READ occupied NOTIFY changed)
    Q_PROPERTY(bool urgent READ urgent NOTIFY changed)
    Q_PROPERTY(bool hidden READ hidden NOTIFY changed)
    Q_PROPERTY(bool hasFullscreen READ hasFullscreen NOTIFY changed)
    Q_PROPERTY(QVariantList windows READ windows NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    QString key() const { return value("key").toString(); }
    QString id() const { return value("id").toString(); }
    QString name() const { return value("name").toString(); }
    int col() const { return value("col").toInt(); }
    int row() const { return value("row").toInt(); }
    QString group() const { return value("group").toString(); }
    QStringList monitors() const { return value("monitors").toStringList(); }
    bool active() const { return value("active").toBool(); }
    bool focused() const { return value("focused").toBool(); }
    int occupied() const { return value("occupied").toInt(); }
    bool urgent() const { return value("urgent").toBool(); }
    bool hidden() const { return value("hidden").toBool(); }
    bool hasFullscreen() const { return value("hasFullscreen").toBool(); }
    QVariantList windows() const { return value("windows").toList(); }
    void announce() override { emit changed(); }
signals:
    void changed();
};

/* `Workspaces`: every workspace; `current`, what the active monitor shows;
 * `showing(monitor)`, a stable facade for what one monitor shows, which is
 * what a per-monitor bar binds, since a singleton cannot know which instance
 * asks; and the declared `arrangement`. Ruling 19.
 * `qml::hosted::tests::the_workspaces_model_its_list_and_its_facades`,
 * `models::tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`. */
class SoliumWorkspaceRows : public SoliumRows
{
    Q_OBJECT
    Q_PROPERTY(SoliumWorkspace *current READ current CONSTANT)
    Q_PROPERTY(QVariantMap arrangement READ arrangement NOTIFY arrangementChanged)
public:
    SoliumWorkspaceRows();
    SoliumWorkspace *current() { return &m_current; }
    Q_INVOKABLE SoliumWorkspace *showing(const QString &monitor);
    QVariantMap arrangement() const { return m_arrangement; }
    void setArrangement(const QVariantMap &arrangement);
signals:
    void arrangementChanged();

private:
    void follow();
    static void copy(SoliumWorkspace &facade, const SoliumRow *row);
    SoliumWorkspace m_current;
    QHash<QString, SoliumWorkspace *> m_showing;
    QVariantMap m_arrangement;
};

/* `WorkspaceList { monitor }`: the workspaces of the group a monitor is in;
 * an empty `monitor` keeps every workspace.
 * `qml::hosted::tests::the_workspaces_model_its_list_and_its_facades`. */
class SoliumWorkspaceList : public QSortFilterProxyModel
{
    Q_OBJECT
    Q_PROPERTY(QString monitor READ monitor WRITE setMonitor NOTIFY changed)
    Q_PROPERTY(int count READ count NOTIFY countChanged)
public:
    explicit SoliumWorkspaceList(QObject *parent = nullptr);
    QString monitor() const { return m_monitor; }
    void setMonitor(const QString &monitor);
    int count() const { return rowCount(); }
signals:
    void changed();
    void countChanged();

protected:
    bool filterAcceptsRow(int source_row, const QModelIndex &source_parent) const override;

private:
    QString m_monitor;
};

/* An installed application's row: one row of `Apps` (03 §3.2.13). A pinned
 * app that is no longer installed reads `present` and `valid` false
 * (Ruling 17), the same ghost-not-a-hole convention as a window or a
 * workspace that has gone. */
class SoliumApp : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(QString id READ id NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(QString genericName READ genericName NOTIFY changed)
    Q_PROPERTY(QString icon READ icon NOTIFY changed)
    Q_PROPERTY(QStringList categories READ categories NOTIFY changed)
    Q_PROPERTY(QStringList keywords READ keywords NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    /* The key role, so even a ghost `get(id)` -- made with no row ever
     * applied -- answers its own id: `SoliumRows::row_for` inserts the
     * requested key under this same role name for exactly that reason
     * (`rows.cpp`, and `SoliumWindow::id`/`SoliumWorkspace::key` do the same). */
    QString id() const { return value("id").toString(); }
    QString name() const { return value("name").toString(); }
    QString genericName() const { return value("genericName").toString(); }
    QString icon() const { return value("icon").toString(); }
    QStringList categories() const { return value("categories").toStringList(); }
    QStringList keywords() const { return value("keywords").toStringList(); }
    void announce() override { emit changed(); }
signals:
    void changed();
};

/* `Apps`: every installed, visible application, and whether the first scan
 * has completed (`Solium.Apps.ready`, 04-ui.md §4.6: a pin's slot exists
 * before the index is ready; whether it is a ghost is not known until it
 * is). */
class SoliumAppRows : public SoliumRows
{
    Q_OBJECT
    Q_PROPERTY(QObject *entries READ entries CONSTANT)
    Q_PROPERTY(bool ready READ ready NOTIFY readyChanged)
public:
    SoliumAppRows();
    QObject *entries() { return this; }
    bool ready() const { return m_ready; }
    void setReady(bool ready);

signals:
    void readyChanged();

private:
    bool m_ready = false;
};

/* One entry of the desktop folder: one row of `Folder` (04-ui.md §4.9). A
 * file that leaves the folder is gone outright -- no ghost, unlike `Apps`'
 * `Retire::Never` -- so `SoliumFolderRows` below retires it `AfterGrace`,
 * the same lifecycle `Windows` and `Workspaces` already keep for things
 * that come and go rather than a fixed, pinned registry. */
class SoliumFolderEntry : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(QString uri READ uri NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(QString displayName READ displayName NOTIFY changed)
    Q_PROPERTY(QString mime READ mime NOTIFY changed)
    Q_PROPERTY(QString icon READ icon NOTIFY changed)
    Q_PROPERTY(bool isDir READ isDir NOTIFY changed)
    Q_PROPERTY(bool isLauncher READ isLauncher NOTIFY changed)
    Q_PROPERTY(bool trusted READ trusted NOTIFY changed)
    Q_PROPERTY(bool hidden READ hidden NOTIFY changed)
    Q_PROPERTY(double modified READ modified NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    /* The key role, so even a ghost reached during its 10 s grace answers
     * its own uri -- the same reason `SoliumApp::id` reads it off `value()`
     * rather than a cached field. */
    QString uri() const { return value("uri").toString(); }
    QString name() const { return value("name").toString(); }
    QString displayName() const { return value("displayName").toString(); }
    QString mime() const { return value("mime").toString(); }
    QString icon() const { return value("icon").toString(); }
    bool isDir() const { return value("isDir").toBool(); }
    bool isLauncher() const { return value("isLauncher").toBool(); }
    bool trusted() const { return value("trusted").toBool(); }
    bool hidden() const { return value("hidden").toBool(); }
    double modified() const { return value("modified").toDouble(); }
    void announce() override { emit changed(); }
signals:
    void changed();
};

/* `Folder`: every entry of the desktop directory, live through inotify
 * (`crate::folder::Watcher`). Empty, and costing nothing to scan, with no
 * desktop directory configured (04-ui.md §4.9). */
class SoliumFolderRows : public SoliumRows
{
    Q_OBJECT
    Q_PROPERTY(QObject *entries READ entries CONSTANT)
public:
    SoliumFolderRows();
    QObject *entries() { return this; }
};

/* The store for a SOLIUM_QML_ROWS_* number, built on first use.
 * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`. */
SoliumRows *solium_rows(int model);

#endif /* SOLIUM_QML_ROWS_H */
