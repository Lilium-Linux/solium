#include "attached.h"

#include "host.h"
#include "keyboard.h"
#include "rows.h"

#include <QtQml/QQmlContext>
#include <QtQml/QQmlEngine>
#include <QtCore/QCoreApplication>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonValue>
#include <QtCore/QMetaObject>
#include <QtQuick/QQuickItem>

#include <algorithm>

namespace {

/* The dynamic property a hosted scene's context carries.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
constexpr const char kHosting[] = "_soliumHosting";

/* The item Qt draws a Qt Quick Controls `Popup` as: its child of type
 * QQuickPopupItem, which the popup makes with itself, so it is there from
 * the popup's first binding, before any content item is; null for any
 * other object.
 * `qml::hosted::tests::a_field_in_a_popup_that_wants_the_keyboard_takes_the_keys`,
 * `qml::hosted::tests::a_controls_popup_is_a_grabs_target`. */
QQuickItem *popup_item_of(QObject *object)
{
    if (object == nullptr || !object->inherits("QQuickPopup")) {
        return nullptr;
    }
    for (QObject *child : object->children()) {
        if (child->inherits("QQuickPopupItem")) {
            return qobject_cast<QQuickItem *>(child);
        }
    }
    return nullptr;
}

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
    for (const QPointer<SoliumKeyboard> &keyboard : keyboards) {
        if (keyboard != nullptr) {
            keyboard->detach();
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

void SoliumGrab::setTarget(QObject *target)
{
    if (target != m_target) {
        m_target = target;
        /* A target that is neither an item nor a Popup has no points, so
         * every press dismisses the grab: the log says so, once.
         * `qml::hosted::tests::a_controls_popup_is_a_grabs_target`. */
        if (target != nullptr && !m_target_warned && qobject_cast<QQuickItem *>(target) == nullptr
            && !target->inherits("QQuickPopup")) {
            m_target_warned = true;
            qWarning("a Grab's target is an item or a Popup; this %s has no points, so every "
                     "press dismisses the grab",
                     target->metaObject()->className());
        }
        emit targetChanged();
    }
}

QQuickItem *SoliumGrab::target_item() const
{
    if (auto *item = qobject_cast<QQuickItem *>(m_target.data())) {
        return item;
    }
    return popup_item_of(m_target.data());
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

namespace {

/* How many times an item has come to want the keyboard, so the one that did
 * last is known.
 * `qml::hosted::tests::the_holder_is_the_focused_wanting_item_else_the_one_that_wanted_last`. */
quint64 g_wants = 0;

} // namespace

SoliumKeyboard::SoliumKeyboard(QQuickItem *item, SoliumHosting *hosting)
    : QObject(item), m_item(item), m_hosting(hosting)
{
    if (m_hosting != nullptr) {
        m_hosting->keyboards.append(this);
    }
    /* Whether it holds the keyboard follows whether it is visible and
     * whether it has active focus, and being shown again or taking active
     * focus again is asking anew after a let-go.
     * `qml::hosted::tests::an_invisible_field_does_not_hold_the_keyboard`,
     * `qml::hosted::tests::a_scene_let_go_of_takes_the_keyboard_again_only_when_asked_anew`. */
    if (item != nullptr) {
        QObject::connect(item, &QQuickItem::visibleChanged, this, [this]() {
            if (m_item != nullptr && m_item->isVisible()) {
                m_let_go = false;
            }
            mark();
        });
        QObject::connect(item, &QQuickItem::activeFocusChanged, this, [this]() {
            if (m_item != nullptr && m_item->hasActiveFocus()) {
                m_let_go = false;
            }
            mark();
        });
    }
}

SoliumKeyboard::~SoliumKeyboard()
{
    if (m_hosting != nullptr) {
        m_hosting->keyboards.removeAll(this);
        m_hosting->keyboard_dirty = true;
    }
}

void SoliumKeyboard::mark()
{
    if (m_hosting != nullptr) {
        m_hosting->keyboard_dirty = true;
    }
}

void SoliumKeyboard::setWants(bool wants)
{
    if (wants != m_wants) {
        m_wants = wants;
        if (wants) {
            m_wanted_at = ++g_wants;
            m_let_go = false;
        }
        mark();
        emit wantsChanged();
    }
}

void SoliumKeyboard::letGo()
{
    m_let_go = true;
    mark();
}

void SoliumKeyboard::focusEntered()
{
    if (m_let_go) {
        m_let_go = false;
        mark();
    }
}

void SoliumKeyboard::setClaims(const QStringList &claims)
{
    if (claims != m_claims) {
        m_claims = claims;
        mark();
        emit claimsChanged();
    }
}

SoliumKeyboard *solium_keyboard_holder(SoliumHosting *hosting)
{
    SoliumKeyboard *holder = nullptr;
    if (hosting == nullptr) {
        return holder;
    }
    for (const QPointer<SoliumKeyboard> &each : hosting->keyboards) {
        if (each == nullptr || !each->wants() || each->isLetGo() || each->item() == nullptr
            || !each->item()->isVisible()) {
            continue;
        }
        if (each->item()->hasActiveFocus()) {
            return each.data();
        }
        if (holder == nullptr || each->wantedAt() > holder->wantedAt()) {
            holder = each.data();
        }
    }
    return holder;
}

SoliumStatus &SoliumStatus::instance()
{
    static SoliumStatus *status = nullptr;
    if (status == nullptr) {
        status = new SoliumStatus();
    }
    return *status;
}

SoliumAttached::SoliumAttached(QObject *item) : QObject(item), m_item(item)
{
    QObject::connect(&SoliumStatus::instance(), &SoliumStatus::changed, this,
                     &SoliumAttached::statusChanged);
}

QString SoliumAttached::status() const
{
    return SoliumStatus::instance().text;
}

SoliumKeyboard *SoliumAttached::keyboard()
{
    /* On an item, the item's. On a Qt Quick Controls `Popup`, which is no
     * item, the item Qt draws it as, whose visibility and active focus are
     * the popup's own, as `Solium.input` and a `Grab`'s target read a popup.
     * `qml::hosted::tests::a_field_in_a_popup_that_wants_the_keyboard_takes_the_keys`.
     * In a scene that is not hosted, one nothing reads.
     * `qml::hosted::tests::an_unhosted_scene_may_bind_the_keyboard_and_holds_nothing`.
     * On any other object of a hosted scene, one nothing reads either, and
     * the log says so, once for the object.
     * `qml::hosted::tests::a_field_in_a_popup_that_wants_the_keyboard_takes_the_keys`. */
    if (m_keyboard == nullptr) {
        QQuickItem *item = qobject_cast<QQuickItem *>(m_item);
        if (item == nullptr) {
            item = popup_item_of(m_item);
        }
        SoliumHosting *hosting = solium_hosting_of(m_item);
        if (item == nullptr && hosting != nullptr) {
            qWarning("Solium.keyboard holds the keyboard only on an item or a Popup; "
                     "this %s will hold nothing",
                     m_item != nullptr ? m_item->metaObject()->className() : "object");
        }
        m_keyboard = new SoliumKeyboard(item, item != nullptr ? hosting : nullptr);
        if (item == nullptr) {
            m_keyboard->setParent(this);
        }
    }
    return m_keyboard;
}

SoliumCursor *SoliumAttached::cursor()
{
    /* One per object, made when first read or written: the published values
     * are the same in every one, and the hotspot is the object's own, read
     * from the scene's root.
     * `qml::pointer::tests::the_hotspot_the_root_sets_is_the_scenes`. */
    if (m_cursor == nullptr) {
        m_cursor = new SoliumCursor(this);
    }
    return m_cursor;
}

void SoliumAttached::setRegion(const QString &region)
{
    if (region != m_region) {
        m_region = region;
        emit regionChanged();
    }
}

void SoliumAttached::setMaterial(const QVariant &material)
{
    if (material != m_material) {
        m_material = material;
        emit materialChanged();
    }
}

void SoliumAttached::send(const QString &action, const QJSValue &data)
{
    /* A scene that is not hosted has no surface to send from, so nothing is
     * queued.
     * `qml::hosted::tests::an_unhosted_scene_may_send_and_queues_nothing`. */
    SoliumHosting *hosting = solium_hosting_of(m_item);
    if (hosting == nullptr) {
        static bool warned = false;
        if (!warned) {
            warned = true;
            qWarning("Solium.send in a scene the compositor did not host for a sol.surface "
                     "does nothing");
        }
        return;
    }
    /* In an object, so a bare value or nothing at all is still one JSON
     * document. `qml::hosted::tests::solium_send_queues_every_action_with_its_data_in_order`. */
    const QJsonObject wrapped{
        {QStringLiteral("data"),
         QJsonValue::fromVariant(data.toVariant(QJSValue::ConvertJSObjects))}};
    hosting->actions.append({action, QJsonDocument(wrapped).toJson(QJsonDocument::Compact)});
}

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
    /* `Solium.keyboard`'s group, nameless for the same reason.
     * `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`. */
    qmlRegisterAnonymousType<SoliumKeyboard>(SOLIUM_NATIVE_URI, 1);
    /* `Solium.cursor`'s group, nameless for the same reason.
     * `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`. */
    qmlRegisterAnonymousType<SoliumCursor>(SOLIUM_NATIVE_URI, 1);
    /* Named, since a scene writes one: `Grab { ... }`.
     * `qml::hosted::tests::a_grab_is_held_while_active_and_dismissed_on_request`. */
    qmlRegisterType<SoliumGrab>(SOLIUM_NATIVE_URI, 1, 0, "Grab");
    /* `Keyboard`, unqualified like `Theme`, in every scene.
     * `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`. */
    qmlRegisterSingletonInstance(SOLIUM_NATIVE_URI, 1, 0, "Keyboard", solium_keyboard());
    /* The models, as singletons, each the one store Rust publishes into.
     * `qml::hosted::tests::the_monitors_model_lists_every_row_and_changes_one_role_at_a_time`. */
    qmlRegisterSingletonInstance(SOLIUM_NATIVE_URI, 1, 0, "Monitors",
                                 solium_rows(SOLIUM_QML_ROWS_MONITORS));
    /* A window's row is nameless like a monitor's: a named `Window` would
     * hide Qt Quick's `Window` in every scene that imports both.
     * `qml::hosted::tests::a_quick_window_is_still_qt_quicks_beside_the_windows_model`. */
    qmlRegisterAnonymousType<SoliumWindow>(SOLIUM_NATIVE_URI, 1);
    qmlRegisterSingletonInstance(SOLIUM_NATIVE_URI, 1, 0, "Windows",
                                 qobject_cast<SoliumWindowRows *>(
                                     solium_rows(SOLIUM_QML_ROWS_WINDOWS)));
    /* Named, since a scene writes one: `WindowList { ... }`.
     * `qml::hosted::tests::the_windows_model_filters_sorts_and_keeps_its_facades`. */
    qmlRegisterType<SoliumWindowList>(SOLIUM_NATIVE_URI, 1, 0, "WindowList");
    /* A workspace's row is nameless like a monitor's, so a shell's own
     * `Workspace.qml` is its own.
     * `qml::hosted::tests::a_shell_file_named_like_a_workspace_is_still_the_shells`. */
    qmlRegisterAnonymousType<SoliumWorkspace>(SOLIUM_NATIVE_URI, 1);
    qmlRegisterSingletonInstance(SOLIUM_NATIVE_URI, 1, 0, "Workspaces",
                                 qobject_cast<SoliumWorkspaceRows *>(
                                     solium_rows(SOLIUM_QML_ROWS_WORKSPACES)));
    /* Named, since a scene writes one: `WorkspaceList { ... }`.
     * `qml::hosted::tests::the_workspaces_model_its_list_and_its_facades`. */
    qmlRegisterType<SoliumWorkspaceList>(SOLIUM_NATIVE_URI, 1, 0, "WorkspaceList");
}

/* What `Solium.status` reads: the text `sol.status` set.
 * `qml::hosted::tests::the_workspaces_model_its_list_and_its_facades`. */
extern "C" int solium_qml_set_status(const char *text)
{
    if (QCoreApplication::instance() == nullptr) {
        return 0;
    }
    SoliumStatus &status = SoliumStatus::instance();
    const QString next = QString::fromUtf8(text != nullptr ? text : "");
    if (next != status.text) {
        status.text = next;
        emit status.changed();
    }
    return 1;
}
