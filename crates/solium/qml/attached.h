/*
 * The attached `Solium` object, and the rows it hands out.
 *
 * Every `Solium.<name>` a hosted scene writes or reads is a property of
 * SoliumAttached. A scene is "hosted" when the compositor built it for one
 * instance of a `sol.surface` on one monitor; it then carries a SoliumHosting
 * record, found from any object of its tree through its QML context.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`,
 * `scripted::tests::a_surface_instance_is_hosted_on_its_monitor`.
 */
#ifndef SOLIUM_QML_ATTACHED_H
#define SOLIUM_QML_ATTACHED_H

#include <QtCore/QObject>
#include <QtCore/QByteArray>
#include <QtCore/QList>
#include <QtCore/QPointer>
#include <QtCore/QRectF>
#include <QtCore/QString>
#include <QtCore/QStringList>
#include <QtCore/QVariant>
#include <QtCore/QVariantMap>
#include <QtQml/QQmlParserStatus>
#include <QtQml/qqml.h>
#include <QtQuick/QQuickItem>

#include <utility>

class QQmlContext;

/* The URI every native type is registered under: "Solium", the URI the
 * QML-only module has too, so one `import Solium` reaches both.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
#define SOLIUM_NATIVE_URI "Solium"

/* One row of a model. Abstract: each kind of row declares its own typed
 * properties and its own `changed` signal. */
class SoliumRow : public QObject
{
    Q_OBJECT
public:
    using QObject::QObject;
    ~SoliumRow() override = default;

    QList<QByteArray> assign(const QVariantMap &next);
    QVariant value(const char *role) const { return values.value(QString::fromLatin1(role)); }
    virtual void announce() = 0;

    QVariantMap values;
    bool present = false;
};

/* A rectangle a row carries, `{ x, y, width, height }`, as QML's `rect`.
 * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`. */
QRectF solium_rect(const QVariant &value);

/* A monitor's row: what `Solium.monitor` is. Its name, its whole and work
 * areas in the global space, its scale, its transform and whether it is the
 * primary monitor, all announced by one `changed` per batch.
 * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`. */
class SoliumMonitor : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(QRectF whole READ whole NOTIFY changed)
    Q_PROPERTY(QRectF area READ area NOTIFY changed)
    Q_PROPERTY(double scale READ scale NOTIFY changed)
    Q_PROPERTY(QString transform READ transform NOTIFY changed)
    Q_PROPERTY(bool primary READ primary NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    QString name() const { return value("name").toString(); }
    QRectF whole() const { return solium_rect(value("whole")); }
    QRectF area() const { return solium_rect(value("area")); }
    double scale() const { return value("scale").toDouble(); }
    QString transform() const { return value("transform").toString(); }
    bool primary() const { return value("primary").toBool(); }
    void announce() override { emit changed(); }
signals:
    void changed();
};

/* `Solium.surface.reserve`: what this instance takes out of its monitor's
 * work area, per edge, in logical pixels; a negative edge is unset, and
 * leaves the edge to the `sol.surface` declaration. Ruling 10.
 * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
class SoliumReserve : public QObject
{
    Q_OBJECT
    Q_PROPERTY(int top READ top WRITE setTop NOTIFY changed)
    Q_PROPERTY(int right READ right WRITE setRight NOTIFY changed)
    Q_PROPERTY(int bottom READ bottom WRITE setBottom NOTIFY changed)
    Q_PROPERTY(int left READ left WRITE setLeft NOTIFY changed)
public:
    using QObject::QObject;
    int top() const { return m_edges[0]; }
    int right() const { return m_edges[1]; }
    int bottom() const { return m_edges[2]; }
    int left() const { return m_edges[3]; }
    void setTop(int value) { set(0, value); }
    void setRight(int value) { set(1, value); }
    void setBottom(int value) { set(2, value); }
    void setLeft(int value) { set(3, value); }
    const int *edges() const { return m_edges; }
    /* Whether an edge changed since the last take: true from the start, so
     * a scene that never sets one still says so once.
     * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
    bool dirty = true;
signals:
    void changed();

private:
    void set(int edge, int value)
    {
        if (m_edges[edge] != value) {
            m_edges[edge] = value;
            dirty = true;
            emit changed();
        }
    }
    int m_edges[4] = {-1, -1, -1, -1};
};

/* `Solium.surface`: this instance of its `sol.surface`, as a whole.
 * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
class SoliumSurfaceInfo : public QObject
{
    Q_OBJECT
    Q_PROPERTY(SoliumReserve *reserve READ reserve CONSTANT)
public:
    using QObject::QObject;
    SoliumReserve *reserve() { return &m_reserve; }

private:
    SoliumReserve m_reserve;
};

struct SoliumHosting;

/* `Grab { name; target; active; onDismissed }`: while active, its scene
 * holds the pointer; a press outside every active target of the scene
 * dismisses them all, newest first. Ruling 12.
 * `qml::hosted::tests::a_grab_is_held_while_active_and_dismissed_on_request`,
 * `qml::hosted::tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`. */
class SoliumGrab : public QObject, public QQmlParserStatus
{
    Q_OBJECT
    Q_INTERFACES(QQmlParserStatus)
    Q_PROPERTY(QString name READ name WRITE setName NOTIFY nameChanged)
    Q_PROPERTY(QObject *target READ target WRITE setTarget NOTIFY targetChanged)
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY activeChanged)
public:
    explicit SoliumGrab(QObject *parent = nullptr) : QObject(parent) {}
    ~SoliumGrab() override;
    void classBegin() override {}
    void componentComplete() override;
    QString name() const { return m_name; }
    void setName(const QString &name);
    QObject *target() const { return m_target; }
    void setTarget(QObject *target);
    /* The item a point is asked of: the target itself, or, for a Qt Quick
     * Controls `Popup`, which is no item, the item Qt draws it as, its
     * content item's parent.
     * `qml::hosted::tests::a_controls_popup_is_a_grabs_target`. */
    QQuickItem *target_item() const;
    bool active() const { return m_active; }
    void setActive(bool active);
    /* When it last became active, by a counter that only goes up.
     * `qml::hosted::tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`. */
    quint64 activated() const { return m_activated; }
    /* Its scene's hosting record is going, before the grab is. */
    void detach() { m_hosting = nullptr; }
signals:
    void nameChanged();
    void targetChanged();
    void activeChanged();
    void dismissed();

private:
    void mark();
    QString m_name;
    QPointer<QObject> m_target;
    bool m_active = false;
    quint64 m_activated = 0;
    SoliumHosting *m_hosting = nullptr;
};

/* `Solium.keyboard`, on one item: whether it wants the keyboard, and the
 * keys it claims while it holds it, as `sol.bind` spells them. The scene
 * holds the keyboard while any visible item wants it. Ruling 14.
 * `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`,
 * `qml::hosted::tests::an_invisible_field_does_not_hold_the_keyboard`. */
class SoliumKeyboard : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool wants READ wants WRITE setWants NOTIFY wantsChanged)
    Q_PROPERTY(QStringList claims READ claims WRITE setClaims NOTIFY claimsChanged)
public:
    SoliumKeyboard(QQuickItem *item, SoliumHosting *hosting);
    ~SoliumKeyboard() override;
    bool wants() const { return m_wants; }
    void setWants(bool wants);
    QStringList claims() const { return m_claims; }
    void setClaims(const QStringList &claims);
    QQuickItem *item() const { return m_item; }
    /* When it last came to want the keyboard, by a counter that only goes up.
     * `qml::hosted::tests::the_holder_is_the_focused_wanting_item_else_the_one_that_wanted_last`. */
    quint64 wantedAt() const { return m_wanted_at; }
    /* The compositor took the keyboard back while it wanted it: it holds
     * none until it asks anew, by coming to want it, being shown again or
     * taking active focus again, whatever `wants` is bound to.
     * `qml::hosted::tests::a_scene_let_go_of_takes_the_keyboard_again_only_when_asked_anew`. */
    void letGo();
    bool isLetGo() const { return m_let_go; }
    /* Its scene's hosting record is going, before it is. */
    void detach() { m_hosting = nullptr; }
signals:
    void wantsChanged();
    void claimsChanged();

private:
    void mark();
    QPointer<QQuickItem> m_item;
    SoliumHosting *m_hosting;
    bool m_wants = false;
    QStringList m_claims;
    quint64 m_wanted_at = 0;
    bool m_let_go = false;
};

/* What one hosted scene carries beside its object tree.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`,
 * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
struct SoliumHosting
{
    explicit SoliumHosting(QString monitor_name) : monitor(std::move(monitor_name)) {}
    ~SoliumHosting();
    SoliumHosting(const SoliumHosting &) = delete;
    SoliumHosting &operator=(const SoliumHosting &) = delete;
    QString monitor;
    SoliumSurfaceInfo surface_info;
    /* Every Grab of the scene, active or not.
     * `qml::hosted::tests::a_grab_is_held_while_active_and_dismissed_on_request`. */
    QList<QPointer<SoliumGrab>> grabs;
    /* Whether its grabs changed since the last take: true from the start, so
     * a scene with none active says so once, and a scene rebuilt for an edit
     * lets go of the grab the one before it held.
     * `qml::hosted::tests::a_scene_with_no_active_grab_says_so_once`. */
    bool grab_dirty = true;
    /* Every item of the scene that has a `Solium.keyboard`.
     * `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`. */
    QList<QPointer<SoliumKeyboard>> keyboards;
    /* Whether who wants the keyboard changed since the last take: true from
     * the start, so a scene says what it has at its first take.
     * `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`. */
    bool keyboard_dirty = true;
};

/* What every item reads as `Solium.<name>`.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
class SoliumAttached : public QObject
{
    Q_OBJECT
    Q_PROPERTY(SoliumMonitor *monitor READ monitor CONSTANT)
    /* `Solium.input`: true takes presses, "hover" only hover, false opts the
     * item out. Unset, the item's own handlers decide. Ruling 6.
     * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`. */
    Q_PROPERTY(QVariant input READ input WRITE setInput NOTIFY inputChanged)
    /* `Solium.surface`: the instance this scene is, its reserve above all.
     * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
    Q_PROPERTY(SoliumSurfaceInfo *surface READ surface CONSTANT)
    /* `Solium.keyboard`: whether this item wants the keyboard, and the keys
     * it claims. `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`. */
    Q_PROPERTY(SoliumKeyboard *keyboard READ keyboard CONSTANT)
public:
    explicit SoliumAttached(QObject *item);
    SoliumMonitor *monitor() const;
    SoliumSurfaceInfo *surface() const;
    QVariant input() const;
    void setInput(const QVariant &value);
    /* -1 unset, 0 opted out, 1 hover, 2 press, as solium_claim_at reads it.
     * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`. */
    int inputClaim() const { return m_input; }
    SoliumKeyboard *keyboard();

signals:
    void inputChanged();

private:
    QObject *m_item;
    int m_input = -1;
    SoliumKeyboard *m_keyboard = nullptr;
};

/* The name `Solium` in QML. It exists only to carry the attached object.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
class SoliumAttachedType : public QObject
{
    Q_OBJECT
    QML_ATTACHED(SoliumAttached)
public:
    static SoliumAttached *qmlAttachedProperties(QObject *object);
};

/* The hosting of the scene `object` belongs to, or null for a scene that is
 * not hosted on a monitor.
 * `qml::hosted::tests::a_scene_hosted_on_no_monitor_reads_an_absent_monitor`. */
SoliumHosting *solium_hosting_of(QObject *object);
/* Mark `context` as `hosting`'s, so solium_hosting_of finds it from every
 * object created in it or in a context below it.
 * `qml::hosted::tests::every_object_of_a_hosted_scene_finds_its_monitor_after_the_build`. */
void solium_hosting_mark(QQmlContext *context, SoliumHosting *hosting);
/* What the items under `scene_point` claim: 0 nothing, 1 hover, 2 a press.
 * The strongest claim of every visible, enabled, non-transparent item there,
 * inside its ancestors' clips and its own contains(), as Qt's own delivery
 * lets an item that takes no press leave it to one below. Ruling 6.
 * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`. */
int solium_claim_at(QQuickItem *item, const QPointF &scene_point);
/* The item of a hosted scene holding the keyboard: of the visible items
 * that want it and have not been let go of since, the one with active
 * focus, else the one that came to want it last; null when none does.
 * Ruling 14.
 * `qml::hosted::tests::the_holder_is_the_focused_wanting_item_else_the_one_that_wanted_last`,
 * `qml::hosted::tests::an_invisible_field_does_not_hold_the_keyboard`,
 * `qml::hosted::tests::a_scene_let_go_of_takes_the_keyboard_again_only_when_asked_anew`. */
SoliumKeyboard *solium_keyboard_holder(SoliumHosting *hosting);
/* The row for a connector name: until the compositor publishes one, an
 * absent row carrying the name. Created on first ask and never freed, so a
 * monitor that goes and comes back is the same row.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`,
 * `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`. */
SoliumMonitor *solium_monitor_row(const QString &name);
/* Register every native type, once, before the engine exists.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
void solium_qml_register_types();

#endif /* SOLIUM_QML_ATTACHED_H */
