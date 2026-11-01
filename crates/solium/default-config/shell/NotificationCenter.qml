// Notifications for the taskbar: banners in the bottom-left corner, above
// the bar, and the list of earlier ones the bell opens. Solium holds them
// (`Notifications.banners`, `Notifications.history`); this only draws them
// and says what was clicked.
//
// Banners go to the monitor the sender's window is on, or else to the active
// one, so each shows once. Hovering one stops its time.

import QtQuick
import Solium

Item {
    id: root

    // The bar (Shell.qml), for its colours and height.
    required property Item shell
    // The list of earlier ones, opened by the bell.
    property bool open: false

    readonly property bool present: typeof Notifications !== "undefined"
    readonly property int cardWidth: 380
    readonly property int shown: 4

    function here(monitor) {
        return monitor ? monitor === Solium.monitor.name : Solium.monitor.active
    }

    function when(received) {
        const at = new Date(received)
        const minutes = Math.floor((Date.now() - received) / 60000)
        if (minutes < 1)
            return "now"
        if (minutes < 60)
            return minutes + " min"
        return Qt.formatTime(at, "HH:mm")
    }

    // One notification, as a banner and as a row in the list.
    component Card: Rectangle {
        id: card
        required property var row
        property bool compact: false
        readonly property bool critical: row.urgency === "critical"

        width: root.cardWidth
        height: content.implicitHeight + 24
        radius: 12
        color: "#f21c1b1d"
        border { width: 1; color: critical ? "#e5534b" : "#2e2c32" }

        HoverHandler { id: hover }
        TapHandler {
            onTapped: {
                if (card.row.hasDefault)
                    Notifications.invoke(card.row.id, "default")
                else
                    Notifications.dismiss(card.row.id)
            }
        }

        Image {
            id: picture
            x: 12; y: 12
            width: 36; height: 36
            source: card.row.image || card.row.icon || ""
            sourceSize { width: 72; height: 72 }
            fillMode: Image.PreserveAspectFit
            asynchronous: true
            visible: status === Image.Ready
        }

        Column {
            id: content
            anchors {
                left: picture.visible ? picture.right : parent.left
                leftMargin: 12
                right: parent.right; rightMargin: 12
                top: parent.top; topMargin: 12
            }
            spacing: 3

            Row {
                width: parent.width
                Text {
                    width: parent.width - age.implicitWidth - close.width
                    text: card.row.appName
                    textFormat: Text.PlainText
                    color: root.shell.textDim
                    font { pixelSize: 11; family: root.shell.font }
                    elide: Text.ElideRight
                }
                Text {
                    id: age
                    text: root.when(card.row.received)
                    color: root.shell.textDim
                    font { pixelSize: 11; family: root.shell.font }
                }
                // Room for the close button, which sits over the corner.
                Item { id: close; width: 22; height: 1 }
            }
            Text {
                width: parent.width
                text: card.row.summary
                textFormat: Text.PlainText
                color: root.shell.text
                font { pixelSize: 14; family: root.shell.font; weight: Font.DemiBold }
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                visible: text.length > 0
                text: card.row.bodyRich
                textFormat: Text.StyledText
                color: root.shell.text
                opacity: 0.85
                linkColor: root.shell.accent
                font { pixelSize: 13; family: root.shell.font }
                wrapMode: Text.Wrap
                maximumLineCount: card.compact ? 2 : 4
                elide: Text.ElideRight
            }
            // The sender's own buttons ("Reply", "Mark as read").
            Row {
                visible: (card.row.actions || []).length > 0
                topPadding: 6
                spacing: 6
                Repeater {
                    model: card.row.actions || []
                    delegate: Rectangle {
                        required property var modelData
                        width: label.implicitWidth + 20
                        height: 26
                        radius: 6
                        color: press.pressed ? root.shell.pressed
                             : over.hovered ? root.shell.hover : root.shell.active
                        Text {
                            id: label
                            anchors.centerIn: parent
                            text: modelData.label
                            textFormat: Text.PlainText
                            color: root.shell.text
                            font { pixelSize: 12; family: root.shell.font }
                        }
                        HoverHandler { id: over }
                        TapHandler {
                            id: press
                            onTapped: Notifications.invoke(card.row.id, modelData.key)
                        }
                    }
                }
            }
        }

        // ×: put it away without opening it.
        Rectangle {
            anchors { right: parent.right; top: parent.top; margins: 8 }
            width: 22; height: 22
            radius: 11
            visible: hover.hovered
            color: shut.hovered ? root.shell.pressed : root.shell.hover
            Text {
                anchors.centerIn: parent
                text: "×"
                color: root.shell.text
                font { pixelSize: 15; family: root.shell.font }
            }
            HoverHandler { id: shut }
            TapHandler { onTapped: Notifications.dismiss(card.row.id) }
        }
    }

    // Banners: newest at the bottom, nearest the bar.
    ListView {
        id: banners
        anchors {
            left: parent.left; leftMargin: 12
            bottom: parent.bottom; bottomMargin: root.shell.barHeight + 12
        }
        width: root.cardWidth
        // The whole height, not the banners': a list that shrank as one left
        // put it outside its own bounds, and Qt plays no exit for a row out
        // of view -- the top one vanished. The empty part takes no clicks:
        // the banners do, the list itself opts out.
        height: root.height - root.shell.barHeight - 24
        Solium.input: false
        visible: !root.open
        verticalLayoutDirection: ListView.BottomToTop
        spacing: 8
        interactive: false
        model: root.present ? Notifications.banners : null

        add: Transition {
            NumberAnimation { property: "x"; from: -(root.cardWidth + 24); duration: 320; easing.type: Easing.OutCubic }
            NumberAnimation { property: "opacity"; from: 0; to: 1; duration: 200 }
        }
        // Out past the left edge, the way it came in.
        remove: Transition {
            NumberAnimation { property: "x"; to: -(root.cardWidth + 24); duration: 360; easing.type: Easing.InCubic }
            NumberAnimation { property: "opacity"; to: 0; duration: 360; easing.type: Easing.InQuad }
        }
        displaced: Transition {
            NumberAnimation { property: "y"; duration: 260; easing.type: Easing.OutCubic }
        }

        delegate: Item {
            id: slot
            required property int index
            required property var model

            // Which screen it is on is decided once, when it arrives: the
            // sender's window's, or else whichever was active then. Read
            // live, one that came from no window followed the pointer from
            // screen to screen.
            property bool arrivedHere: false
            // What it shows, kept: a row going away has nothing left to
            // read, and the card would collapse instead of sliding out.
            property var kept: ({})
            function capture() {
                if (slot.model.id === undefined)
                    return
                slot.kept = {
                    id: slot.model.id, monitor: slot.model.monitor, appName: slot.model.appName,
                    received: slot.model.received, summary: slot.model.summary,
                    bodyRich: slot.model.bodyRich, image: slot.model.image, icon: slot.model.icon,
                    urgency: slot.model.urgency, actions: slot.model.actions,
                    hasDefault: slot.model.hasDefault,
                }
            }
            readonly property int revision: model.revision === undefined ? -1 : model.revision
            onRevisionChanged: capture()
            Component.onCompleted: {
                capture()
                arrivedHere = root.here(slot.kept.monitor)
            }

            readonly property bool mine: arrivedHere && index < root.shown
            width: root.cardWidth
            height: mine ? banner.height : 0
            visible: mine

            Card {
                id: banner
                row: slot.kept
                // Its time stops while the pointer rests on it.
                readonly property bool holding: hovering.hovered
                HoverHandler { id: hovering }
                onHoldingChanged: holding ? Notifications.hold(slot.kept.id)
                                          : Notifications.release(slot.kept.id)
            }
        }
    }

    // A click anywhere else closes the list.
    MouseArea {
        anchors.fill: parent
        visible: root.open
        onPressed: root.open = false
    }

    // The list of earlier ones.
    Rectangle {
        id: panel
        visible: root.open
        anchors {
            right: parent.right; rightMargin: 8
            bottom: parent.bottom; bottomMargin: root.shell.barHeight + 8
        }
        width: root.cardWidth + 24
        height: Math.min(header.height + Math.max(list.contentHeight, 72) + 24,
                         root.height - root.shell.barHeight - 48)
        radius: 14
        color: root.shell.barColor
        border { width: 1; color: "#2e2c32" }
        // Swallows clicks so they do not reach the closer under it.
        MouseArea { anchors.fill: parent }

        Item {
            id: header
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 12 }
            height: 30

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "Notifications"
                color: root.shell.text
                font { pixelSize: 15; family: root.shell.font; weight: Font.DemiBold }
            }
            Row {
                anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                spacing: 6

                // Do not disturb: banners stop, the list still fills.
                Rectangle {
                    width: dndText.implicitWidth + 20
                    height: 26
                    radius: 6
                    color: root.present && Notifications.dnd ? root.shell.accent
                         : dndHover.hovered ? root.shell.hover : root.shell.active
                    Text {
                        id: dndText
                        anchors.centerIn: parent
                        text: "Do not disturb"
                        color: root.present && Notifications.dnd ? "#150f26" : root.shell.text
                        font { pixelSize: 12; family: root.shell.font }
                    }
                    HoverHandler { id: dndHover }
                    TapHandler { onTapped: Notifications.setDnd(!Notifications.dnd) }
                }
                Rectangle {
                    visible: root.present && Notifications.history.count > 0
                    width: clearText.implicitWidth + 20
                    height: 26
                    radius: 6
                    color: clearHover.hovered ? root.shell.hover : root.shell.active
                    Text {
                        id: clearText
                        anchors.centerIn: parent
                        text: "Clear all"
                        color: root.shell.text
                        font { pixelSize: 12; family: root.shell.font }
                    }
                    HoverHandler { id: clearHover }
                    TapHandler { onTapped: Notifications.dismissAll() }
                }
            }
        }

        ListView {
            id: list
            anchors {
                left: parent.left; right: parent.right; top: header.bottom; bottom: parent.bottom
                leftMargin: 12; rightMargin: 12; topMargin: 8; bottomMargin: 12
            }
            clip: true
            spacing: 8
            model: root.present ? Notifications.history : null
            delegate: Card {
                required property var model
                row: model
                compact: true
            }
            removeDisplaced: Transition {
                NumberAnimation { property: "y"; duration: 220; easing.type: Easing.OutCubic }
            }
        }

        Text {
            anchors.centerIn: list
            visible: list.count === 0
            text: "No notifications"
            color: root.shell.textDim
            font { pixelSize: 13; family: root.shell.font }
        }
    }
}
