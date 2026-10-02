#include "keyboard.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>
#include <QtQml/QQmlEngine>

void SoliumKeyboard::publish(const QJsonObject &next)
{
    const int layout = next.value(QStringLiteral("layout")).toInt();
    const QString name = next.value(QStringLiteral("layoutName")).toString();
    const QString short_name = next.value(QStringLiteral("layoutShort")).toString();
    QStringList layouts;
    for (const QJsonValue &each : next.value(QStringLiteral("layouts")).toArray()) {
        layouts.append(each.toString());
    }
    const bool caps = next.value(QStringLiteral("caps")).toBool();
    const bool num = next.value(QStringLiteral("num")).toBool();

    const bool layout_moved =
        layout != m_layout || name != m_layout_name || short_name != m_layout_short;
    const bool layouts_moved = layouts != m_layouts;
    const bool caps_moved = caps != m_caps;
    const bool num_moved = num != m_num;
    m_layout = layout;
    m_layout_name = name;
    m_layout_short = short_name;
    m_layouts = layouts;
    m_caps = caps;
    m_num = num;

    if (layouts_moved) {
        emit layoutsChanged();
    }
    if (layout_moved) {
        emit layoutChanged();
    }
    if (caps_moved) {
        emit capsChanged();
    }
    if (num_moved) {
        emit numChanged();
    }
    for (const QJsonValue &what : next.value(QStringLiteral("changed")).toArray()) {
        emit changed(what.toString());
    }
}

SoliumKeyboard *solium_keyboard()
{
    static SoliumKeyboard *instance = nullptr;
    if (instance == nullptr) {
        instance = new SoliumKeyboard();
        QQmlEngine::setObjectOwnership(instance, QQmlEngine::CppOwnership);
    }
    return instance;
}

/* What `Solium::publish_models` hands over once a frame when the keyboard
 * moved: the values, and the changes since the last batch Qt took. 0 when Qt
 * has not started, so the compositor sends it again.
 * `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`. */
extern "C" int solium_qml_keyboard_publish(const char *json)
{
    if (QCoreApplication::instance() == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    solium_keyboard()->publish(document.object());
    return 1;
}
