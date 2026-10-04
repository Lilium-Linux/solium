/*
 * The pointer, as a scene reads it: `Solium.cursor`.
 *
 * The compositor publishes what the pointer is doing once a frame, while a
 * pointer scene is configured (`cursor.scene`): the named shape it shows, as
 * its CSS cursor name, whether a button is held, how fast it moves, the scale
 * of the monitor under it and the configured size. Every object of every
 * scene reads the same values through its own `Solium.cursor`; the pointer's
 * scene also sets `Solium.cursor.hotspot` on its root item, which is the
 * point of its picture that sits on the pointer.
 * `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`,
 * `qml::pointer::tests::the_hotspot_the_root_sets_is_the_scenes`.
 *
 * Neither type takes a name in the URI (Ruling 1a): `SoliumCursor` is
 * anonymous, reached only as `Solium.cursor`, and the state behind it is not
 * registered at all.
 */
#ifndef SOLIUM_QML_POINTER_H
#define SOLIUM_QML_POINTER_H

#include <QtCore/QObject>
#include <QtCore/QPointF>
#include <QtCore/QString>

class QJsonObject;

/* What the compositor last published, one per process.
 * `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`. */
class SoliumPointerState : public QObject
{
    Q_OBJECT
public:
    using QObject::QObject;
    QString shape() const { return m_shape; }
    bool pressed() const { return m_pressed; }
    QPointF velocity() const { return m_velocity; }
    double scale() const { return m_scale; }
    int size() const { return m_size; }

    /* Take what the compositor publishes: every value first, then each
     * notification for a value that changed, so a binding that reads two of
     * them sees both new.
     * `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`. */
    void publish(const QJsonObject &next);

signals:
    void shapeChanged();
    void pressedChanged();
    void velocityChanged();
    void scaleChanged();
    void sizeChanged();

private:
    QString m_shape = QStringLiteral("default");
    bool m_pressed = false;
    QPointF m_velocity;
    double m_scale = 1.0;
    int m_size = 24;
};

/* The one instance, made on first ask and never freed. */
SoliumPointerState *solium_pointer_state();

/* `Solium.cursor` on one object: the published pointer, read-only, and the
 * hotspot, which the pointer's scene writes on its root item.
 * `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`,
 * `qml::pointer::tests::the_hotspot_the_root_sets_is_the_scenes`. */
class SoliumCursor : public QObject
{
    Q_OBJECT
    /* The named shape, as its CSS cursor name: "default", "text",
     * "pointer", "ew-resize" and so on. */
    Q_PROPERTY(QString shape READ shape NOTIFY shapeChanged)
    /* Whether a mouse button is held. */
    Q_PROPERTY(bool pressed READ pressed NOTIFY pressedChanged)
    /* How fast the pointer moved over the last frame, in logical pixels a
     * second, along x and along y. */
    Q_PROPERTY(QPointF velocity READ velocity NOTIFY velocityChanged)
    /* The scale of the monitor the pointer is on. */
    Q_PROPERTY(double scale READ scale NOTIFY scaleChanged)
    /* The configured `cursor.size`, in logical pixels. */
    Q_PROPERTY(int size READ size NOTIFY sizeChanged)
    /* The point of the picture that sits on the pointer, in the scene's
     * logical pixels: (0, 0), the top-left corner, unless the scene says. */
    Q_PROPERTY(QPointF hotspot READ hotspot WRITE setHotspot NOTIFY hotspotChanged)
public:
    explicit SoliumCursor(QObject *parent);
    QString shape() const { return solium_pointer_state()->shape(); }
    bool pressed() const { return solium_pointer_state()->pressed(); }
    QPointF velocity() const { return solium_pointer_state()->velocity(); }
    double scale() const { return solium_pointer_state()->scale(); }
    int size() const { return solium_pointer_state()->size(); }
    QPointF hotspot() const { return m_hotspot; }
    void setHotspot(const QPointF &hotspot);

signals:
    void shapeChanged();
    void pressedChanged();
    void velocityChanged();
    void scaleChanged();
    void sizeChanged();
    void hotspotChanged();

private:
    QPointF m_hotspot;
};

#endif /* SOLIUM_QML_POINTER_H */
