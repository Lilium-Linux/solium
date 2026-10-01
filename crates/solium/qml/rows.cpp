#include "rows.h"

#include "host.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>
#include <QtCore/QJsonValue>
#include <QtCore/QList>
#include <QtCore/QMetaProperty>
#include <QtCore/QTimer>
#include <QtQml/QQmlEngine>

namespace {

constexpr int kGraceMs = 10000;

SoliumRow *make_monitor(QObject *parent)
{
    return new SoliumMonitor(parent);
}

} // namespace

SoliumRows::SoliumRows(const QMetaObject *row_type, Make make, Retire retire,
                       const char *key_role, QObject *parent)
    : QAbstractListModel(parent), m_make(make), m_retire(retire), m_key_role(key_role)
{
    int role = Qt::UserRole + 1;
    for (int i = row_type->propertyOffset(); i < row_type->propertyCount(); ++i) {
        m_roles.insert(role++, QByteArray(row_type->property(i).name()));
    }
    m_absent = m_make(this);
    QQmlEngine::setObjectOwnership(m_absent, QQmlEngine::CppOwnership);
}

int SoliumRows::rowCount(const QModelIndex &parent) const
{
    return parent.isValid() ? 0 : count();
}

QVariant SoliumRows::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || index.row() < 0 || index.row() >= m_rows.size()) {
        return {};
    }
    return m_rows.at(index.row())->property(m_roles.value(role).constData());
}

QHash<int, QByteArray> SoliumRows::roleNames() const
{
    return m_roles;
}

QString SoliumRows::key_of(const SoliumRow *row) const
{
    return row->value(m_key_role.constData()).toString();
}

SoliumRow *SoliumRows::row_for(const QString &key)
{
    if (SoliumRow *row = m_by_key.value(key); row != nullptr) {
        return row;
    }
    if (m_retire != Retire::Never) {
        return nullptr;
    }
    SoliumRow *row = m_make(this);
    QQmlEngine::setObjectOwnership(row, QQmlEngine::CppOwnership);
    row->values.insert(QString::fromUtf8(m_key_role), key);
    m_by_key.insert(key, row);
    return row;
}

QObject *SoliumRows::get(const QVariant &key)
{
    SoliumRow *row = row_for(key.toString());
    return row != nullptr ? row : m_absent;
}

void SoliumRows::retire(SoliumRow *row)
{
    row->present = false;
    if (m_retire == Retire::Never) {
        return;
    }
    const QString key = key_of(row);
    QTimer::singleShot(kGraceMs, row, [this, row, key]() {
        if (!row->present) {
            m_by_key.remove(key);
            row->deleteLater();
        }
    });
}

bool SoliumRows::apply(const QJsonArray &ops)
{
    const int was = count();
    struct Touched
    {
        SoliumRow *row;
        QList<QByteArray> roles;
    };
    QVector<Touched> touched;
    bool whole = true;
    for (const QJsonValue &each : ops) {
        const QJsonObject op = each.toObject();
        const QString kind = op.value(QStringLiteral("op")).toString();
        const QString key = op.value(QStringLiteral("key")).toString();
        const int at = op.value(QStringLiteral("at")).toInt(-1);
        if (kind == QStringLiteral("insert")) {
            if (at < 0 || at > m_rows.size()) {
                whole = false;
                break;
            }
            SoliumRow *row = m_by_key.value(key);
            if (row == nullptr) {
                row = m_make(this);
                QQmlEngine::setObjectOwnership(row, QQmlEngine::CppOwnership);
                m_by_key.insert(key, row);
            }
            row->assign(op.value(QStringLiteral("values")).toObject().toVariantMap());
            row->present = true;
            beginInsertRows(QModelIndex(), at, at);
            m_rows.insert(at, row);
            endInsertRows();
            touched.append(Touched{row, {}});
        } else if (kind == QStringLiteral("change")) {
            if (at < 0 || at >= m_rows.size() || key_of(m_rows.at(at)) != key) {
                whole = false;
                break;
            }
            SoliumRow *row = m_rows.at(at);
            const QList<QByteArray> roles =
                row->assign(op.value(QStringLiteral("values")).toObject().toVariantMap());
            if (!roles.isEmpty()) {
                touched.append(Touched{row, roles});
            }
        } else if (kind == QStringLiteral("remove")) {
            if (at < 0 || at >= m_rows.size() || key_of(m_rows.at(at)) != key) {
                whole = false;
                break;
            }
            beginRemoveRows(QModelIndex(), at, at);
            SoliumRow *row = m_rows.takeAt(at);
            endRemoveRows();
            retire(row);
            touched.append(Touched{row, {}});
        } else if (kind == QStringLiteral("move")) {
            const int from = op.value(QStringLiteral("from")).toInt(-1);
            const int to = op.value(QStringLiteral("to")).toInt(-1);
            if (from < 0 || from >= m_rows.size() || to < 0 || to >= m_rows.size()
                || key_of(m_rows.at(from)) != key) {
                whole = false;
                break;
            }
            if (from != to) {
                beginMoveRows(QModelIndex(), from, from, QModelIndex(), to > from ? to + 1 : to);
                m_rows.move(from, to);
                endMoveRows();
            }
        } else {
            whole = false;
            break;
        }
    }
    for (const Touched &each : touched) {
        SoliumRow *row = each.row;
        const QList<QByteArray> &roles = each.roles;
        row->announce();
        const int index_of = static_cast<int>(m_rows.indexOf(row));
        if (index_of >= 0 && !roles.isEmpty()) {
            QList<int> ids;
            for (auto it = m_roles.cbegin(); it != m_roles.cend(); ++it) {
                if (roles.contains(it.value())) {
                    ids.append(it.key());
                }
            }
            emit dataChanged(index(index_of), index(index_of), ids);
        }
    }
    if (count() != was) {
        emit countChanged();
    }
    emit applied();
    if (!whole) {
        qWarning("a model batch did not match the rows held, and was applied only up to that step");
    }
    return whole;
}

SoliumRows *solium_rows(int model)
{
    static SoliumRows *monitors = nullptr;
    if (QCoreApplication::instance() == nullptr) {
        return nullptr;
    }
    switch (model) {
    case SOLIUM_QML_ROWS_MONITORS:
        if (monitors == nullptr) {
            monitors = new SoliumRows(&SoliumMonitor::staticMetaObject, make_monitor,
                                      SoliumRows::Retire::Never, "name");
        }
        return monitors;
    default:
        return nullptr;
    }
}

extern "C" int solium_qml_rows_apply(int model, const char *ops_json)
{
    SoliumRows *rows = solium_rows(model);
    if (rows == nullptr || ops_json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(ops_json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isArray()) {
        return 0;
    }
    return rows->apply(document.array()) ? 1 : 0;
}
