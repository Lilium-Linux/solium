/*
 * The keyboard, as every scene reads it: `Keyboard` in `import Solium`,
 * written unqualified like `Theme`. The live layout, its names, the locks,
 * and `changed(what)` once for each real change of one of them.
 * `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
 *
 * A singleton because there is one keyboard and every scene, a window's frame
 * as much as a hosted shell, reads the same one. It has no row types, so
 * nothing of a shell's own is shadowed by one (Ruling 1a); the one name it
 * takes in the URI is `Keyboard` itself.
 */
#ifndef SOLIUM_QML_KEYBOARD_H
#define SOLIUM_QML_KEYBOARD_H

#include <QtCore/QObject>
#include <QtCore/QString>
#include <QtCore/QStringList>

class QJsonObject;

class SoliumKeyboard : public QObject
{
    Q_OBJECT
    /* The live layout's index into `layouts`, from 0. */
    Q_PROPERTY(int layout READ layout NOTIFY layoutChanged)
    /* Its name, "Russian", and its short name, "RU". */
    Q_PROPERTY(QString layoutName READ layoutName NOTIFY layoutChanged)
    Q_PROPERTY(QString layoutShort READ layoutShort NOTIFY layoutChanged)
    /* Every layout's name, in order. */
    Q_PROPERTY(QStringList layouts READ layouts NOTIFY layoutsChanged)
    Q_PROPERTY(bool caps READ caps NOTIFY capsChanged)
    Q_PROPERTY(bool num READ num NOTIFY numChanged)
public:
    using QObject::QObject;
    int layout() const { return m_layout; }
    QString layoutName() const { return m_layout_name; }
    QString layoutShort() const { return m_layout_short; }
    QStringList layouts() const { return m_layouts; }
    bool caps() const { return m_caps; }
    bool num() const { return m_num; }

    /* Take what the compositor publishes: every value first, then each
     * property's own notification, then `changed(what)` for each name in
     * `changed`, in order.
     * `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`. */
    void publish(const QJsonObject &next);

signals:
    void layoutChanged();
    void layoutsChanged();
    void capsChanged();
    void numChanged();
    /* A real change: "layout", "caps" or "num". */
    void changed(const QString &what);

private:
    int m_layout = 0;
    QString m_layout_name;
    QString m_layout_short;
    QStringList m_layouts;
    bool m_caps = false;
    bool m_num = false;
};

/* The one instance, made on first ask and never freed. */
SoliumKeyboard *solium_keyboard();

#endif /* SOLIUM_QML_KEYBOARD_H */
