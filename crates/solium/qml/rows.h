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
    /* Apply one batch. False when a step does not match the rows held.
     * `qml::hosted::tests::a_batch_that_does_not_match_the_rows_held_is_refused`. */
    bool apply(const QJsonArray &ops);
    const QVector<SoliumRow *> &rows() const { return m_rows; }

signals:
    void countChanged();
    void applied();

private:
    QString key_of(const SoliumRow *row) const;
    void retire(SoliumRow *row);

    QHash<int, QByteArray> m_roles;
    QVector<SoliumRow *> m_rows;
    QHash<QString, SoliumRow *> m_by_key;
    Make m_make;
    Retire m_retire;
    QByteArray m_key_role;
    SoliumRow *m_absent = nullptr;
};

/* The store for a SOLIUM_QML_ROWS_* number, built on first use. */
SoliumRows *solium_rows(int model);

#endif /* SOLIUM_QML_ROWS_H */
