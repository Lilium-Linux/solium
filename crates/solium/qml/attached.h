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
#include <QtCore/QRectF>
#include <QtCore/QString>
#include <QtCore/QVariant>
#include <QtCore/QVariantMap>
#include <QtQml/qqml.h>

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

/* What one hosted scene carries beside its object tree.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`,
 * `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`. */
struct SoliumHosting
{
    explicit SoliumHosting(QString monitor_name) : monitor(std::move(monitor_name)) {}
    QString monitor;
    SoliumSurfaceInfo surface_info;
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
public:
    explicit SoliumAttached(QObject *item);
    SoliumMonitor *monitor() const;
    SoliumSurfaceInfo *surface() const;
    QVariant input() const;
    void setInput(const QVariant &value);
    /* -1 unset, 0 opted out, 1 hover, 2 press, as solium_claim_at reads it.
     * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`. */
    int inputClaim() const { return m_input; }

signals:
    void inputChanged();

private:
    QObject *m_item;
    int m_input = -1;
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
class QQuickItem;
/* What the items under `scene_point` claim: 0 nothing, 1 hover, 2 a press.
 * The strongest claim of every visible, enabled, non-transparent item there,
 * inside its ancestors' clips and its own contains(), as Qt's own delivery
 * lets an item that takes no press leave it to one below. Ruling 6.
 * `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`. */
int solium_claim_at(QQuickItem *item, const QPointF &scene_point);
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
