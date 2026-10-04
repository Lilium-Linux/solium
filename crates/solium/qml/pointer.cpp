#include "pointer.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>
#include <QtQml/QQmlEngine>

#include <cmath>

void SoliumPointerState::publish(const QJsonObject &next)
{
    const QString shape = next.value(QStringLiteral("shape")).toString(m_shape);
    const bool pressed = next.value(QStringLiteral("pressed")).toBool(m_pressed);
    const QJsonObject moving = next.value(QStringLiteral("velocity")).toObject();
    const QPointF velocity(moving.value(QStringLiteral("x")).toDouble(m_velocity.x()),
                           moving.value(QStringLiteral("y")).toDouble(m_velocity.y()));
    const double scale = next.value(QStringLiteral("scale")).toDouble(m_scale);
    const int size = next.value(QStringLiteral("size")).toInt(m_size);

    const bool shape_moved = shape != m_shape;
    const bool pressed_moved = pressed != m_pressed;
    const bool velocity_moved = velocity != m_velocity;
    const bool scale_moved = scale != m_scale;
    const bool size_moved = size != m_size;
    m_shape = shape;
    m_pressed = pressed;
    m_velocity = velocity;
    m_scale = scale;
    m_size = size;

    if (shape_moved) {
        emit shapeChanged();
    }
    if (pressed_moved) {
        emit pressedChanged();
    }
    if (velocity_moved) {
        emit velocityChanged();
    }
    if (scale_moved) {
        emit scaleChanged();
    }
    if (size_moved) {
        emit sizeChanged();
    }
}

SoliumPointerState *solium_pointer_state()
{
    static SoliumPointerState *instance = nullptr;
    if (instance == nullptr) {
        instance = new SoliumPointerState();
        QQmlEngine::setObjectOwnership(instance, QQmlEngine::CppOwnership);
    }
    return instance;
}

SoliumCursor::SoliumCursor(QObject *parent) : QObject(parent)
{
    SoliumPointerState *state = solium_pointer_state();
    QObject::connect(state, &SoliumPointerState::shapeChanged, this, &SoliumCursor::shapeChanged);
    QObject::connect(state, &SoliumPointerState::pressedChanged, this,
                     &SoliumCursor::pressedChanged);
    QObject::connect(state, &SoliumPointerState::velocityChanged, this,
                     &SoliumCursor::velocityChanged);
    QObject::connect(state, &SoliumPointerState::scaleChanged, this, &SoliumCursor::scaleChanged);
    QObject::connect(state, &SoliumPointerState::sizeChanged, this, &SoliumCursor::sizeChanged);
}

void SoliumCursor::setHotspot(const QPointF &hotspot)
{
    /* A point that is not a number is no point of the picture: the corner.
     * `qml::pointer::tests::the_hotspot_the_root_sets_is_the_scenes`. */
    const QPointF next = std::isfinite(hotspot.x()) && std::isfinite(hotspot.y()) ? hotspot
                                                                                  : QPointF();
    if (next != m_hotspot) {
        m_hotspot = next;
        emit hotspotChanged();
    }
}

/* What `models::pointer::publish` hands over once a frame when the pointer
 * moved: the values, as one JSON object. 0 when Qt has not started, so the
 * compositor sends it again.
 * `qml::pointer::tests::a_published_pointer_reaches_solium_cursor`. */
extern "C" int solium_qml_pointer_publish(const char *json)
{
    if (QCoreApplication::instance() == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    solium_pointer_state()->publish(document.object());
    return 1;
}
