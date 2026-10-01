/*
 * The attached `Solium` object, and the rows it hands out.
 *
 * Every `Solium.<name>` a hosted scene writes or reads is a property of
 * SoliumAttached (Section 2, rule 8). A scene is "hosted" when the compositor
 * built it for one instance of a `sol.surface` on one monitor; it then carries
 * a SoliumHosting record, found from any object of its tree through its QML
 * context. `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`.
 */
#ifndef SOLIUM_QML_ATTACHED_H
#define SOLIUM_QML_ATTACHED_H

#include <QtCore/QObject>
#include <QtCore/QByteArray>
#include <QtCore/QList>
#include <QtCore/QString>
#include <QtCore/QVariant>
#include <QtCore/QVariantMap>
#include <QtQml/qqml.h>

class QQmlContext;

/* The URI every native type is registered under. Ruling 1: "Solium", beside
 * the QML-only module of the same name, or "Solium.Native" if the two cannot
 * share it. `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
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

/* A monitor's row: what `Solium.monitor` is. Task 3 adds the geometry.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
class SoliumMonitor : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    QString name() const { return value("name").toString(); }
    void announce() override { emit changed(); }
signals:
    void changed();
};

/* What one hosted scene carries beside its object tree.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
struct SoliumHosting
{
    QString monitor;
};

/* What every item reads as `Solium.<name>`.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
class SoliumAttached : public QObject
{
    Q_OBJECT
    Q_PROPERTY(SoliumMonitor *monitor READ monitor CONSTANT)
public:
    explicit SoliumAttached(QObject *item);
    SoliumMonitor *monitor() const;

private:
    QObject *m_item;
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
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
void solium_hosting_mark(QQmlContext *context, SoliumHosting *hosting);
/* The row for a connector name: until Task 3 publishes one, an absent row
 * carrying the name. Created on first ask and never freed (Ruling 4).
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
SoliumMonitor *solium_monitor_row(const QString &name);
/* Register every native type, once, before the engine exists.
 * `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`. */
void solium_qml_register_types();

#endif /* SOLIUM_QML_ATTACHED_H */
