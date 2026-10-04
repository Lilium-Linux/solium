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

/* The store for a SOLIUM_QML_ROWS_* number, built on first use.
 * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`. */
SoliumRows *solium_rows(int model);

#endif /* SOLIUM_QML_ROWS_H */
