#include "attached.h"

#include "host.h"
#include "rows.h"

#include <QtQml/QQmlContext>
#include <QtQml/QQmlEngine>
#include <QtQuick/QQuickItem>

#include <algorithm>

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

QVariant SoliumAttached::input() const
{
    switch (m_input) {
    case 0:
        return false;
    case 1:
        return QStringLiteral("hover");
    case 2:
        return true;
    default:
        return {};
    }
}

void SoliumAttached::setInput(const QVariant &value)
{
    int next = -1;
    if (value.typeId() == QMetaType::Bool) {
        next = value.toBool() ? 2 : 0;
    } else if (value.toString() == QStringLiteral("hover")) {
        next = 1;
    } else {
        qWarning("Solium.input takes true, false or \"hover\"");
    }
    if (next != m_input) {
        m_input = next;
        emit inputChanged();
    }
}

namespace {

/* Whether an item takes presses itself, whatever handlers it has: the
 * Qt Quick types that do.
 * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`. */
bool takes_presses_itself(const QQuickItem *item)
{
    for (const char *type : {"QQuickMouseArea", "QQuickControl", "QQuickFlickable",
                             "QQuickPathView", "QQuickMultiPointTouchArea", "QQuickTextInput",
                             "QQuickTextEdit"}) {
        if (item->inherits(type)) {
            return true;
        }
    }
    return false;
}

/* What one item claims for itself, before its children are asked. Its
 * handlers first: Qt gives an item with any pointer handler every mouse
 * button, to hand presses on to the handlers, so an item with only a
 * HoverHandler accepts every button and still takes no press itself, and
 * neither does one whose handlers are all disabled, since Qt hands a
 * disabled handler nothing.
 * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`,
 * `qml::hosted::tests::a_disabled_handler_claims_nothing`. */
int own_claim(QQuickItem *item)
{
    auto *attached = qobject_cast<SoliumAttached *>(
        qmlAttachedPropertiesObject<SoliumAttachedType>(item, false));
    if (attached != nullptr && attached->inputClaim() >= 0) {
        return attached->inputClaim();
    }
    bool handlers = false;
    bool hover_handler = false;
    for (QObject *child : item->children()) {
        if (!child->inherits("QQuickPointerHandler")) {
            continue;
        }
        handlers = true;
        if (!child->property("enabled").toBool()) {
            continue;
        }
        if (child->inherits("QQuickHoverHandler")) {
            hover_handler = true;
        } else {
            return 2;
        }
    }
    if (handlers) {
        if (takes_presses_itself(item)) {
            return 2;
        }
        return hover_handler ? 1 : 0;
    }
    if (item->acceptedMouseButtons() != Qt::NoButton) {
        return 2;
    }
    return item->acceptHoverEvents() ? 1 : 0;
}

} // namespace

int solium_claim_at(QQuickItem *item, const QPointF &scene_point)
{
    if (item == nullptr || !item->isVisible() || item->opacity() <= 0.0 || !item->isEnabled()) {
        return 0;
    }
    const bool inside = item->contains(item->mapFromScene(scene_point));
    if (item->clip() && !inside) {
        return 0;
    }
    int claim = inside ? own_claim(item) : 0;
    for (QQuickItem *child : item->childItems()) {
        if (claim == 2) {
            break;
        }
        claim = std::max(claim, solium_claim_at(child, scene_point));
    }
    return claim;
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
