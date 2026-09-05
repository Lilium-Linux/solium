// An item sized to the one thing inside it.
import QtQuick
Item {
    default property alias content: inner.data
    property int margin: 0
    implicitWidth: inner.childrenRect.width + margin * 2
    implicitHeight: inner.childrenRect.height + margin * 2
    Item { id: inner; anchors.fill: parent; anchors.margins: parent.margin }
}
