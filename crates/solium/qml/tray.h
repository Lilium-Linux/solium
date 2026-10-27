/*
 * `Tray`: the items applications put in the tray, for scenes (#363).
 *
 * `items` is a keyed row model fed by Rust like every other model
 * (`crate::models`), with the service's own values (`available`, `serving`,
 * `owner`). An item's menu is a value of its row, a tree of entries, so a
 * shell draws its own menu. The calls a scene makes on an item
 * (`item.activate(x, y)`, `item.clickMenu(entry)`) are queued for Rust,
 * which makes them on the bus and tells Lua: the direct path, as
 * `Notifications`' are. Nothing here decides anything.
 */
#ifndef SOLIUM_QML_TRAY_H
#define SOLIUM_QML_TRAY_H

#include "rows.h"

#include <QtCore/QObject>
#include <QtCore/QVariant>
#include <QtGui/QImage>

/* One item: a row of `items`. An item that goes reads `present: false` for a
 * short grace, so an exit animation bound to it can finish. */
class SoliumTrayItem : public SoliumRow
{
    Q_OBJECT
    Q_PROPERTY(bool present READ isPresent NOTIFY changed)
    Q_PROPERTY(bool valid READ isPresent NOTIFY changed)
    Q_PROPERTY(int id READ id NOTIFY changed)
    Q_PROPERTY(QString service READ service NOTIFY changed)
    Q_PROPERTY(QString itemId READ itemId NOTIFY changed)
    Q_PROPERTY(QString category READ category NOTIFY changed)
    Q_PROPERTY(QString title READ title NOTIFY changed)
    Q_PROPERTY(QString status READ status NOTIFY changed)
    Q_PROPERTY(QString icon READ icon NOTIFY changed)
    Q_PROPERTY(QString iconName READ iconName NOTIFY changed)
    Q_PROPERTY(QString attentionIcon READ attentionIcon NOTIFY changed)
    Q_PROPERTY(QString overlayIcon READ overlayIcon NOTIFY changed)
    Q_PROPERTY(QString tooltipTitle READ tooltipTitle NOTIFY changed)
    Q_PROPERTY(QString tooltipBody READ tooltipBody NOTIFY changed)
    Q_PROPERTY(bool itemIsMenu READ itemIsMenu NOTIFY changed)
    Q_PROPERTY(bool hasMenu READ hasMenu NOTIFY changed)
    Q_PROPERTY(bool menuOpen READ menuOpen NOTIFY changed)
    Q_PROPERTY(QVariantList menu READ menu NOTIFY changed)
public:
    using SoliumRow::SoliumRow;
    bool isPresent() const { return present; }
    int id() const { return value("id").toInt(); }
    QString service() const { return value("service").toString(); }
    QString itemId() const { return value("itemId").toString(); }
    QString category() const { return value("category").toString(); }
    QString title() const { return value("title").toString(); }
    QString status() const { return value("status").toString(); }
    QString icon() const { return value("icon").toString(); }
    QString iconName() const { return value("iconName").toString(); }
    QString attentionIcon() const { return value("attentionIcon").toString(); }
    QString overlayIcon() const { return value("overlayIcon").toString(); }
    QString tooltipTitle() const { return value("tooltipTitle").toString(); }
    QString tooltipBody() const { return value("tooltipBody").toString(); }
    bool itemIsMenu() const { return value("itemIsMenu").toBool(); }
    bool hasMenu() const { return value("hasMenu").toBool(); }
    bool menuOpen() const { return value("menuOpen").toBool(); }
    QVariantList menu() const { return value("menu").toList(); }
    void announce() override { emit changed(); }

    /* A primary click: its menu opens instead for an item that is one.
     * `x` and `y` are where, on the screen, for an item that places a window
     * of its own there. */
    Q_INVOKABLE void activate(int x = 0, int y = 0);
    /* A middle click. */
    Q_INVOKABLE void secondaryActivate(int x = 0, int y = 0);
    /* A wheel: `delta` in steps of 120 a notch, `orientation` `vertical` or
     * `horizontal`. */
    Q_INVOKABLE void scroll(int delta, const QString &orientation = QStringLiteral("vertical"));
    /* The item draws its own menu, if it has one. */
    Q_INVOKABLE void contextMenu(int x = 0, int y = 0);
    /* Its menu, for the scene to draw: `menuOpen` turns true and `menu` fills
     * once it is read. An item with no menu to hand over is asked to show
     * its own. */
    Q_INVOKABLE void openMenu(int x = 0, int y = 0);
    Q_INVOKABLE void closeMenu();
    /* An entry of its open menu, by id: the item is told, and the menu
     * closes. */
    Q_INVOKABLE void clickMenu(int entry);
    /* An entry that opens a submenu is about to be shown: the item may fill
     * it now. */
    Q_INVOKABLE void openSubmenu(int entry);
signals:
    void changed();
};

class SoliumTrayRows : public SoliumRows
{
    Q_OBJECT
public:
    SoliumTrayRows();
};

/* `Tray`, as a scene names it. A lock's scenes read one with no items. */
class SoliumTray : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool available READ available NOTIFY stateChanged)
    Q_PROPERTY(bool serving READ serving NOTIFY stateChanged)
    Q_PROPERTY(QVariantMap owner READ owner NOTIFY stateChanged)
    Q_PROPERTY(QObject *items READ items CONSTANT)
public:
    explicit SoliumTray(bool redacted, QObject *parent = nullptr);
    bool available() const { return m_state.value(QStringLiteral("available")).toBool(); }
    bool serving() const { return m_state.value(QStringLiteral("serving")).toBool(); }
    QVariantMap owner() const { return m_state.value(QStringLiteral("owner")).toMap(); }
    QObject *items() const;
    void setState(const QVariantMap &state);

    /* The item with this id, or an absent row. */
    Q_INVOKABLE QObject *get(int id) const;
    /* Every open menu closed, as a click outside them asks. */
    Q_INVOKABLE void closeMenus();
signals:
    void stateChanged();

private:
    bool m_redacted;
    QVariantMap m_state;
};

/* The `Tray` a scene names: the session's, or, `redacted`, the lock's. */
SoliumTray *solium_tray(bool redacted);

/* The picture behind `image://solium/tray/<id>/<kind>/<revision>`, or a null
 * image. Called from the image provider, on whatever thread it runs on. */
QImage solium_tray_image(quint32 id, const QString &kind);

#endif
