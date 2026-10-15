// The `bar` layer of `panes/glass`: the bar along the bottom of the window.
//
// The window buttons at the left, the title right after them, and the
// application's icon at the right. The bar has no fill at all, so what shows
// through it is the glass `effects.lua` puts behind the whole pane, and the
// bar and the window are one piece. A window without the keyboard is told
// apart by the shade round its edge (the `shade` effect `effects.lua` puts in
// front of it) and by its dimmer title.
//
// **Nothing here animates.** A focus change, a new title, a button under the
// pointer, each is one new picture and no more: the shade's fade is the
// compositor's to blend, and a bar drawn again on every frame of it cost a
// busy screen its frames
// (`decoration::tests::a_focus_change_draws_the_glass_bar_once`).
//
// **It is drawn at every size** while the window's size keeps changing, and
// never stretched: it declares no `resizeHz` (`Solium/Layer.qml`). A new size
// draws into the buffer the bar already has, which is why that is cheap
// (`qml::buffers`; `decoration::tests::the_glass_bars_numbers_change_what_it_draws`).
//
// The numbers to change are under "The look" below, beside the two in
// `Pane.qml`, the corners and the bar's height, and the shade's in
// `effects.lua`. The panes README lists them.

import QtQuick
import Solium

Item {
    id: frame

    // Set by the compositor, once: the bar's height, `Pane.qml`'s
    // `insets.bottom`. Zero is what it reads when the file is built outside a
    // pane, by `--check-qml`, and drawing nothing is right there.
    property int insetBottom: 0

    // Set by the compositor.
    property string title: ""
    property string appId: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // Read by the compositor. `onButton`, from `hovered`, keeps a press on a
    // button from dragging the window, as in `top/Frame.qml`.
    property string action: ""
    property string hovered: ""
    readonly property bool onButton: hovered !== ""

    // --- The look ----------------------------------------------------------
    // The application's icon, in logical pixels.
    property int iconSize: 16

    // Whether the icon is on show: an id set, room for it, and an icon the
    // theme has for that id. What the tests read.
    readonly property bool iconShown: icon.visible

    // The buttons, as the shipped styles draw them: grey, each turning its own
    // shade under the pointer and showing its glyph, `×` to close and `+` to
    // maximise (`tests/scenarios/pane-buttons-drawn.lua`), at once rather than
    // eased, as everything in this layer is.
    component FrameButton: Rectangle {
        id: button

        property color tint: Theme.control
        property string name: ""

        width: 13
        height: 13
        radius: width / 2
        color: pointer.containsMouse
               ? tint
               : (frame.focused ? Theme.control : Theme.controlInactive)
        scale: pointer.pressed ? 0.86 : 1.0

        Rectangle {
            anchors.centerIn: parent
            width: 7
            height: 1.5
            radius: 0.75
            antialiasing: true
            color: Theme.accentInk
            rotation: button.name === "close" ? 45 : 0
            visible: pointer.containsMouse
        }
        Rectangle {
            anchors.centerIn: parent
            width: 7
            height: 1.5
            radius: 0.75
            antialiasing: true
            color: Theme.accentInk
            rotation: button.name === "close" ? -45 : 90
            visible: pointer.containsMouse
        }

        MouseArea {
            id: pointer
            anchors.fill: parent
            hoverEnabled: true
            onClicked: frame.action = button.name

            // Cleared only by the button that set it, so moving from one
            // button straight onto another does not leave `hovered` empty.
            onContainsMouseChanged: {
                if (containsMouse) {
                    frame.hovered = button.name;
                } else if (frame.hovered === button.name) {
                    frame.hovered = "";
                }
            }
        }
    }

    // The band the bar is drawn in, along the window's bottom edge, with no
    // fill. The rest of this Item is over the client and stays transparent.
    Item {
        id: band

        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: frame.insetBottom
    }

    // What a narrow tile leaves room for (#133), as `top/Frame.qml` works it
    // out, from the left: close needs its 13px with `Theme.margin` either side,
    // 37px; maximise another `Theme.gap` and 13px, 59px. The icon needs its
    // own size and `Theme.margin` at the right, a `Theme.gap` clear of the
    // buttons: 84px at its 16. The title starts a `Theme.gap` after the
    // buttons and stops one before the icon, and needs 24px of its own: 117px.
    // Each piece is hidden with `visible` rather than squeezed, which also
    // takes a hidden button out of the pointer's reach.
    readonly property int buttonsEnd: Theme.margin + 13 + Theme.gap + 13
    readonly property int iconStart: frame.width - Theme.margin - frame.iconSize
    readonly property bool roomForClose: frame.width >= 13 + 2 * Theme.margin
    readonly property bool roomForMaximize: frame.width >= frame.buttonsEnd + Theme.margin
    readonly property bool roomForIcon: frame.iconStart - Theme.gap >= frame.buttonsEnd
    readonly property bool roomForTitle:
        frame.iconStart - Theme.gap - (frame.buttonsEnd + Theme.gap) >= 24

    // At the left, close at the outer edge as it is at the right in `top`.
    Row {
        id: buttons

        anchors {
            left: band.left
            leftMargin: Theme.margin
            verticalCenter: band.verticalCenter
        }
        spacing: Theme.gap

        FrameButton { name: "close"; tint: Theme.danger; visible: frame.roomForClose }
        FrameButton { name: "maximize"; tint: Theme.warning; visible: frame.roomForMaximize }
    }

    // Right after the buttons, up to the icon, or to the edge when there is
    // no icon to show.
    Text {
        anchors {
            left: buttons.right
            leftMargin: Theme.gap
            right: icon.visible ? icon.left : band.right
            rightMargin: icon.visible ? Theme.gap : Theme.margin
            verticalCenter: band.verticalCenter
        }
        visible: frame.roomForTitle
        text: frame.title
        elide: Text.ElideRight
        color: frame.focused ? Theme.text : Theme.textDim
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
    }

    // The application's icon, from the theme, by the window's app id. A name
    // the theme has no icon for, nor its fallback, shows none, and no id asks
    // for none. Loaded on the spot rather than on a thread: once per window,
    // when its id arrives, and so in the frame that tells it.
    Image {
        id: icon

        anchors {
            right: band.right
            rightMargin: Theme.margin
            verticalCenter: band.verticalCenter
        }
        width: frame.iconSize
        height: frame.iconSize
        fillMode: Image.PreserveAspectFit
        asynchronous: false
        source: frame.appId !== ""
                ? "image://solium/icon/" + encodeURIComponent(frame.appId) + "?size=" + frame.iconSize
                : ""
        visible: frame.roomForIcon && status === Image.Ready
    }
}
