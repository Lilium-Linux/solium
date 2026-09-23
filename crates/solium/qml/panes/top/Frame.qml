// The one layer of `panes/top`: the bar itself.
//
// Every colour and measurement comes from `Solium.Theme`, which the shell's own
// surfaces import too — one engine, one singleton, so changing a colour there
// changes the titlebars and the dock together rather than in two places that
// drift.
//
// Properties in (`title`, `focused`) are set from Rust each frame. `action` is
// the one property that flows the other way — a button sets it, the compositor
// takes it and clears it. One direction, one owner.

import QtQuick
import Solium

Item {
    id: frame

    // What this layer paints, set by the compositor from the `insets` that
    // `Pane.qml` declares. A style reserves space once for the whole pane, so
    // the number lives in the manifest and every layer is told it — the space
    // reserved and the space painted cannot be two different numbers, which is
    // what putting `insets` on `PaneStyle` rather than on `Layer` was for.
    //
    // Zero is what this reads if the file is built outside a pane — by
    // `--check-qml`, say — and drawing nothing is the honest answer there.
    property int insetTop: 0

    // Set by the compositor.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // Read by the compositor.
    property string action: ""

    // Which button the pointer is over, or empty. Read by the compositor to
    // decide whether a press starts a window drag — QML owns the button
    // layout, so QML is what knows. Duplicating the geometry in Rust would be
    // a mirror of state with two authorities.
    property string hovered: ""
    readonly property bool onButton: hovered !== ""

    // Shared look for the buttons, so a third one is three lines rather than a
    // copy of forty.
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

        Behavior on color { ColorAnimation { duration: Theme.quick } }
        Behavior on scale {
            NumberAnimation { duration: 100; easing.type: Easing.OutCubic }
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

    // The bar itself. The rest of this Item is over the client and stays
    // transparent, which is what "the frame covers the whole window" means.
    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: frame.insetTop
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        Behavior on color { ColorAnimation { duration: Theme.normal } }

        // A hairline where the frame meets the client, so the seam reads as
        // deliberate rather than as a gap.
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }

    // What a narrow tile leaves room for (#133). Below 150px the title was
    // given no width and the buttons never hid, so a crowded corner of a tiled
    // desktop drew fragments of text with the buttons on top of them. Pieces
    // are hidden with `visible` rather than squeezed: that takes a piece out
    // of the picture and out of the pointer's reach in one property, since an
    // invisible item receives no mouse events -- so a hidden button cannot set
    // `hovered`, which the compositor reads to decide whether a press on the
    // bar starts a drag. The `Row` lays out only its visible buttons, so close
    // stays against the right edge when maximise goes.
    //
    // The close button needs its own 13px, `Theme.margin` to its right and as
    // much again to its left: 37px. The maximise button needs another
    // `Theme.gap` and 13px on top: 59px. The title keeps the 150px this file
    // already held back from it for the buttons, and needs 24px of its own
    // before a few letters of it read as anything but a fragment. Under 37px
    // the bar is bare.
    readonly property bool roomForTitle: frame.width - 150 >= 24
    readonly property bool roomForClose: frame.width >= 37
    readonly property bool roomForMaximize: frame.width >= 59

    Text {
        anchors.centerIn: bar
        visible: frame.roomForTitle
        width: Math.min(implicitWidth, Math.max(frame.width - 150, 0))
        text: frame.title
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        color: frame.focused ? Theme.text : Theme.textDim
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }

        Behavior on color { ColorAnimation { duration: Theme.normal } }
    }

    Row {
        anchors {
            right: bar.right
            rightMargin: Theme.margin
            verticalCenter: bar.verticalCenter
        }
        spacing: Theme.gap

        FrameButton { name: "maximize"; tint: Theme.warning; visible: frame.roomForMaximize }
        FrameButton { name: "close"; tint: Theme.danger; visible: frame.roomForClose }
    }
}
