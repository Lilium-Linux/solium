#include "attached.h"

#include "host.h"
#include "rows.h"

#include <QtQml/QQmlContext>
#include <QtQml/QQmlEngine>

namespace {

/* The dynamic property a hosted scene's context carries.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
constexpr const char kHosting[] = "_soliumHosting";

} // namespace

QRectF solium_rect(const QVariant &value)
{
    const QVariantMap map = value.toMap();
    return QRectF(map.value(QStringLiteral("x")).toDouble(),
                  map.value(QStringLiteral("y")).toDouble(),
                  map.value(QStringLiteral("width")).toDouble(),
                  map.value(QStringLiteral("height")).toDouble());
}

QList<QByteArray> SoliumRow::assign(const QVariantMap &next)
{
    QList<QByteArray> changed;
    for (auto it = next.cbegin(); it != next.cend(); ++it) {
        if (values.value(it.key()) != it.value()) {
            values.insert(it.key(), it.value());
            changed.append(it.key().toUtf8());
        }
    }
    return changed;
}

SoliumHosting *solium_hosting_of(QObject *object)
{
    for (QQmlContext *context = object != nullptr ? qmlContext(object) : nullptr;
         context != nullptr; context = context->parentContext()) {
        const QVariant marked = context->property(kHosting);
        if (marked.isValid()) {
            return static_cast<SoliumHosting *>(marked.value<void *>());
        }
    }
    return nullptr;
}

void solium_hosting_mark(QQmlContext *context, SoliumHosting *hosting)
{
    context->setProperty(kHosting, QVariant::fromValue(static_cast<void *>(hosting)));
}

SoliumMonitor *solium_monitor_row(const QString &name)
{
    SoliumRows *rows = solium_rows(SOLIUM_QML_ROWS_MONITORS);
    SoliumRow *row = rows != nullptr ? rows->row_for(name) : nullptr;
    if (row != nullptr) {
        return static_cast<SoliumMonitor *>(row);
    }
    static SoliumMonitor *absent = nullptr;
    if (absent == nullptr) {
        absent = new SoliumMonitor();
        QQmlEngine::setObjectOwnership(absent, QQmlEngine::CppOwnership);
    }
    return absent;
}

SoliumAttached::SoliumAttached(QObject *item) : QObject(item), m_item(item) {}

SoliumMonitor *SoliumAttached::monitor() const
{
    const SoliumHosting *hosting = solium_hosting_of(m_item);
    return solium_monitor_row(hosting != nullptr ? hosting->monitor : QString());
}

SoliumAttached *SoliumAttachedType::qmlAttachedProperties(QObject *object)
{
    return new SoliumAttached(object);
}

void solium_qml_register_types()
{
    qmlRegisterUncreatableType<SoliumAttachedType>(
        SOLIUM_NATIVE_URI, 1, 0, "Solium",
        QStringLiteral("Solium is an attached object: write Solium.monitor on an item"));
    /* Known to QML, so a scene reads a row's properties, but with no name: a
     * name in the URI `Solium` would shadow a file of the same name beside a
     * shell's scene, a `Monitor.qml`, in every scene that imports `Solium`.
     * `qml::hosted::tests::a_shell_file_named_like_a_row_is_still_the_shells`. */
    qmlRegisterAnonymousType<SoliumMonitor>(SOLIUM_NATIVE_URI, 1);
}
