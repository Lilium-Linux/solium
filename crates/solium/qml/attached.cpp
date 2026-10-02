#include "attached.h"

#include "host.h"
#include "rows.h"

#include <QtQml/QQmlContext>
#include <QtQml/QQmlEngine>
#include <QtCore/QMetaObject>
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

SoliumHosting::~SoliumHosting()
{
    for (const QPointer<SoliumGrab> &grab : grabs) {
        if (grab != nullptr) {
            grab->detach();
        }
    }
}

namespace {

/* How many times a grab has become active, so the newest of a scene's
 * active grabs is the one that counted last.
 * `qml::hosted::tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`. */
quint64 g_activations = 0;

} // namespace

void SoliumGrab::componentComplete()
{
    m_hosting = solium_hosting_of(this);
    if (m_hosting != nullptr) {
        m_hosting->grabs.append(this);
        mark();
    }
}

SoliumGrab::~SoliumGrab()
{
    if (m_hosting != nullptr) {
        m_hosting->grabs.removeAll(this);
        m_hosting->grab_dirty = true;
    }
}

void SoliumGrab::mark()
{
    if (m_hosting != nullptr) {
        m_hosting->grab_dirty = true;
    }
}

void SoliumGrab::setName(const QString &name)
{
    if (name != m_name) {
        m_name = name;
        mark();
        emit nameChanged();
    }
}

void SoliumGrab::setTarget(QQuickItem *target)
{
    if (target != m_target) {
        m_target = target;
        emit targetChanged();
    }
}

void SoliumGrab::setActive(bool active)
{
    if (active != m_active) {
        m_active = active;
        if (active) {
            m_activated = ++g_activations;
        }
        mark();
        emit activeChanged();
    }
}

SoliumAttached::SoliumAttached(QObject *item) : QObject(item), m_item(item) {}

SoliumMonitor *SoliumAttached::monitor() const
{
    const SoliumHosting *hosting = solium_hosting_of(m_item);
    return solium_monitor_row(hosting != nullptr ? hosting->monitor : QString());
}

SoliumSurfaceInfo *SoliumAttached::surface() const
{
    /* A scene that is not hosted gets one that nothing reads, so a binding to
     * it is harmless.
     * `qml::hosted::tests::an_unhosted_scene_may_bind_a_reserve_and_reserves_nothing`. */
    static SoliumSurfaceInfo *inert = nullptr;
    SoliumHosting *hosting = solium_hosting_of(m_item);
    if (hosting != nullptr) {
        return &hosting->surface_info;
    }
    if (inert == nullptr) {
        inert = new SoliumSurfaceInfo();
        QQmlEngine::setObjectOwnership(inert, QQmlEngine::CppOwnership);
    }
    return inert;
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

/* What `Solium.input` says of an item: -1 unset, else its claim. A Qt Quick
 * Controls `Popup` is not an item, and Qt draws it with an item of its own,
 * the popup's child, so what a scene writes on the `Popup` is read there.
 * `qml::hosted::tests::solium_input_on_a_controls_popup_is_its_items`. */
int declared_input(QQuickItem *item)
{
    QObject *popup = item->inherits("QQuickPopupItem") ? item->parent() : nullptr;
    for (QObject *owner : {static_cast<QObject *>(item), popup}) {
        if (owner == nullptr) {
            continue;
        }
        auto *attached = qobject_cast<SoliumAttached *>(
            qmlAttachedPropertiesObject<SoliumAttachedType>(owner, false));
        if (attached != nullptr && attached->inputClaim() >= 0) {
            return attached->inputClaim();
        }
    }
    return -1;
}

/* Whether something hears `object`'s `signal`, a normalized signature, a
 * handler in QML included. QObject::isSignalConnected is protected; a
 * pointer to it formed through a class derived from QObject may be called
 * on any QObject.
 * `qml::hosted::tests::a_text_takes_a_press_only_on_a_link`. */
struct SignalPeek : QObject {
    static bool heard(const QObject *object, const char *signal)
    {
        const QMetaObject *meta = object->metaObject();
        const int index = meta->indexOfSignal(signal);
        return index >= 0 && (object->*(&SignalPeek::isSignalConnected))(meta->method(index));
    }
};

/* Whether `item` is a Text with a link at `local`, a point in its own
 * coordinates, that something hears activated. Qt gives every Text the left
 * button (QQuickTextPrivate::init) and lets a press go unless linkActivated
 * has a receiver and a link is under it (QQuickText::mousePressEvent,
 * qquicktext.cpp:2975-2979, asks isLinkActivatedConnected, then the
 * anchorAt that linkAt asks too, :3268), so a Text takes a press there and
 * nowhere else.
 * `qml::hosted::tests::a_text_takes_a_press_only_on_a_link`. */
bool on_a_link(QQuickItem *item, const QPointF &local)
{
    if (!item->inherits("QQuickText") || !SignalPeek::heard(item, "linkActivated(QString)")) {
        return false;
    }
    QString link;
    QMetaObject::invokeMethod(item, "linkAt", Qt::DirectConnection, Q_RETURN_ARG(QString, link),
                              Q_ARG(qreal, local.x()), Q_ARG(qreal, local.y()));
    return !link.isEmpty();
}

/* What one item claims for itself at `local`, a point in its own
 * coordinates, before its children are asked. Its handlers first: Qt gives
 * an item with any pointer handler every mouse button, to hand presses on to
 * the handlers, so an item with only a HoverHandler accepts every button and
 * still takes no press itself, and neither does one whose handlers are all
 * disabled, since Qt hands a disabled handler nothing. A Text takes a press
 * only on a link it handles.
 * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`,
 * `qml::hosted::tests::a_disabled_handler_claims_nothing`,
 * `qml::hosted::tests::a_text_takes_a_press_only_on_a_link`. */
int own_claim(QQuickItem *item, const QPointF &local)
{
    const int declared = declared_input(item);
    if (declared >= 0) {
        return declared;
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
        if (takes_presses_itself(item) || on_a_link(item, local)) {
            return 2;
        }
        return hover_handler ? 1 : 0;
    }
    if (on_a_link(item, local)
        || (!item->inherits("QQuickText") && item->acceptedMouseButtons() != Qt::NoButton)) {
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
    const QPointF local = item->mapFromScene(scene_point);
    const bool inside = item->contains(local);
    if (item->clip() && !inside) {
        return 0;
    }
    int claim = inside ? own_claim(item, local) : 0;
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
    /* The groups `Solium.surface.reserve` is reached through, nameless for
     * the same reason.
     * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
    qmlRegisterAnonymousType<SoliumSurfaceInfo>(SOLIUM_NATIVE_URI, 1);
    qmlRegisterAnonymousType<SoliumReserve>(SOLIUM_NATIVE_URI, 1);
    /* Named, since a scene writes one: `Grab { ... }`.
     * `qml::hosted::tests::a_grab_is_held_while_active_and_dismissed_on_request`. */
    qmlRegisterType<SoliumGrab>(SOLIUM_NATIVE_URI, 1, 0, "Grab");
}
