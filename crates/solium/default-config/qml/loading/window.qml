import QtQuick

// Shadows the shipped loading scene. The compositor owns the arrival fade.
Item {
    id: pane
    property string program: ""
    property int waited: 0

    // The program's icon from Solium's provider, which resolves it through
    // the installed desktop files and icon theme.
    readonly property string iconSource: program !== ""
        ? "image://solium/icon/" + encodeURIComponent(program) + "?size=256" : ""
    readonly property bool hasIcon: icon.status === Image.Ready
    readonly property real iconSize: Math.min(160, Math.max(24, Math.min(width, height) * 0.40))

    Rectangle {
        anchors.fill: parent
        radius: 12
        color: Qt.rgba(0.20, 0.18, 0.26, 0.55)
        border.width: 1
        border.color: "#40936dff"
    }

    Column {
        anchors.centerIn: parent
        spacing: 28
        width: Math.max(0, pane.width - 32)

        Item {
            width: parent.width
            height: pane.hasIcon ? pane.iconSize : 28

            Image {
                id: icon
                anchors.centerIn: parent
                width: pane.iconSize
                height: width
                source: pane.iconSource
                sourceSize.width: 192
                sourceSize.height: 192
                fillMode: Image.PreserveAspectFit
                smooth: true
                mipmap: true
                visible: pane.hasIcon
            }

            Text {
                anchors.fill: parent
                visible: !pane.hasIcon
                text: pane.program || "Application"
                color: "#e9e8ec"
                font.family: "Noto Sans"
                font.pixelSize: 18
                font.weight: Font.Medium
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
        }

        Rectangle {
            id: track
            anchors.horizontalCenter: parent.horizontalCenter
            width: Math.min(112, parent.width)
            height: 3
            radius: 1.5
            color: "#30936dff"
            clip: true
            // Use Solium's clock; no independent timer or perpetual animation.
            Rectangle {
                width: track.width * 0.32
                height: track.height
                radius: track.radius
                color: "#c6b6ef"
                x: -width + (track.width + width) * ((pane.waited % 1200) / 1200)
            }
        }
    }
}
