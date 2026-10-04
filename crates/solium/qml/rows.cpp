#include "rows.h"

#include "host.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>
#include <QtCore/QJsonValue>
#include <QtCore/QList>
#include <QtCore/QMetaProperty>
#include <QtCore/QStringList>
#include <QtCore/QTimer>
#include <QtQml/QQmlEngine>

namespace {

constexpr int kGraceMs = 10000;

SoliumRow *make_monitor(QObject *parent)
{
    return new SoliumMonitor(parent);
}

SoliumRow *make_window(QObject *parent)
{
    return new SoliumWindow(parent);
}

SoliumWindow *window_at(const QAbstractItemModel *model, int row)
{
    const auto *rows = qobject_cast<const SoliumRows *>(model);
    return rows != nullptr && row >= 0 && row < rows->rows().size()
               ? qobject_cast<SoliumWindow *>(rows->rows().at(row))
               : nullptr;
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

bool SoliumRows::fits(const QJsonArray &ops) const
{
    QStringList keys;
    keys.reserve(m_rows.size());
    for (const SoliumRow *row : m_rows) {
        keys.append(key_of(row));
    }
    for (const QJsonValue &each : ops) {
        const QJsonObject op = each.toObject();
        const QString kind = op.value(QStringLiteral("op")).toString();
        const QString key = op.value(QStringLiteral("key")).toString();
        const int at = op.value(QStringLiteral("at")).toInt(-1);
        const int size = static_cast<int>(keys.size());
        if (kind == QStringLiteral("insert")) {
            if (at < 0 || at > size || keys.contains(key)) {
                return false;
            }
            keys.insert(at, key);
        } else if (kind == QStringLiteral("change")) {
            if (at < 0 || at >= size || keys.at(at) != key) {
                return false;
            }
        } else if (kind == QStringLiteral("remove")) {
            if (at < 0 || at >= size || keys.at(at) != key) {
                return false;
            }
            keys.removeAt(at);
        } else if (kind == QStringLiteral("move")) {
            const int from = op.value(QStringLiteral("from")).toInt(-1);
            const int to = op.value(QStringLiteral("to")).toInt(-1);
            if (from < 0 || from >= size || to < 0 || to >= size || keys.at(from) != key) {
                return false;
            }
            keys.move(from, to);
        } else {
            return false;
        }
    }
    return true;
}

bool SoliumRows::apply(const QJsonArray &ops)
{
    if (!fits(ops)) {
        qWarning("a model batch did not match the rows held, and none of it was applied");
        return false;
    }
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

SoliumWindowRows::SoliumWindowRows()
    : SoliumRows(&SoliumWindow::staticMetaObject, make_window, Retire::AfterGrace, "id")
{
    QQmlEngine::setObjectOwnership(&m_focused, QQmlEngine::CppOwnership);
    QObject::connect(this, &SoliumRows::applied, this, [this]() { follow(); });
}

void SoliumWindowRows::follow()
{
    const SoliumRow *now = nullptr;
    for (const SoliumRow *row : rows()) {
        if (row->value("focused").toBool()) {
            now = row;
        }
    }
    // Taken whole, not merged: with nothing focused the facade is as empty as
    // the absent row, not the window focused last.
    // `qml::hosted::tests::the_focused_facade_is_empty_with_nothing_focused`.
    const bool present = now != nullptr;
    const QVariantMap next = present ? now->values : QVariantMap();
    if (next != m_focused.values || present != m_focused.present) {
        m_focused.values = next;
        m_focused.present = present;
        m_focused.announce();
    }
}

SoliumWindowList::SoliumWindowList(QObject *parent) : QSortFilterProxyModel(parent)
{
    setSourceModel(solium_rows(SOLIUM_QML_ROWS_WINDOWS));
    setDynamicSortFilter(true);
    QObject::connect(this, &QAbstractItemModel::rowsInserted, this, &SoliumWindowList::countChanged);
    QObject::connect(this, &QAbstractItemModel::rowsRemoved, this, &SoliumWindowList::countChanged);
    QObject::connect(this, &QAbstractItemModel::layoutChanged, this, &SoliumWindowList::countChanged);
    QObject::connect(this, &QAbstractItemModel::modelReset, this, &SoliumWindowList::countChanged);
}

template <typename Change>
void SoliumWindowList::refilterWith(Change &&change)
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 10, 0)
    beginFilterChange();
    change();
    endFilterChange(QSortFilterProxyModel::Direction::Rows);
#else
    change();
    invalidateFilter();
#endif
    emit changed();
    emit countChanged();
}

void SoliumWindowList::refilter(QString &field, const QString &value)
{
    if (field != value) {
        refilterWith([&field, &value]() { field = value; });
    }
}

void SoliumWindowList::setOnStage(const QVariant &value)
{
    if (value != m_on_stage) {
        refilterWith([this, &value]() { m_on_stage = value; });
    }
}

void SoliumWindowList::setSortBy(const QString &value)
{
    if (value != m_sort) {
        m_sort = value;
        sort(value.isEmpty() ? -1 : 0);
        invalidate();
        emit changed();
    }
}

bool SoliumWindowList::filterAcceptsRow(int source_row, const QModelIndex &) const
{
    const SoliumWindow *window = window_at(sourceModel(), source_row);
    if (window == nullptr) {
        return false;
    }
    return (m_monitor.isEmpty() || window->monitor() == m_monitor)
           && (m_workspace.isEmpty() || window->workspace() == m_workspace)
           && (m_app.isEmpty() || window->appId() == m_app)
           && (!m_on_stage.isValid() || window->onStage() == m_on_stage.toBool());
}

bool SoliumWindowList::lessThan(const QModelIndex &left, const QModelIndex &right) const
{
    const SoliumWindow *a = window_at(sourceModel(), left.row());
    const SoliumWindow *b = window_at(sourceModel(), right.row());
    if (a == nullptr || b == nullptr || m_sort != QStringLiteral("mru")) {
        return left.row() < right.row();
    }
    return a->focusOrder() < b->focusOrder();
}

SoliumRows *solium_rows(int model)
{
    static SoliumRows *monitors = nullptr;
    static SoliumRows *windows = nullptr;
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
    case SOLIUM_QML_ROWS_WINDOWS:
        if (windows == nullptr) {
            windows = new SoliumWindowRows();
        }
        return windows;
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
