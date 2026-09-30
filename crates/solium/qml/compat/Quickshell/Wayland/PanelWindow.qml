// A layer-shell panel, as the compositor hosts it.
//
// In Quickshell this is a window the compositor is asked for over
// wlr-layer-shell, and like any window it is not an Item: what is declared in
// it goes into its content item. Inside Solium there is nobody to ask -- the
// scene is being drawn by the compositor -- so the host draws the content item
// across the area the scene was given. The anchors, exclusive zone and the
// rest are kept so shell code that sets them still loads; the compositor
// decides placement. An Item cannot carry them: `anchors` is FINAL on Item,
// and Qt refuses a type that redeclares it. See
// `a_panel_window_is_drawn_through_its_content_item`.

import QtQuick

QtObject {
    id: panel

    readonly property PanelAnchors anchors: PanelAnchors {}
    property int exclusiveZone: 0
    property string layer: "top"
    property string namespace: ""
    property var screen: null
    property color color: "transparent"
    property bool visible: true
    property real implicitWidth: 0
    property real implicitHeight: 0
    readonly property alias width: content.width
    readonly property alias height: content.height

    readonly property Item contentItem: Rectangle {
        id: content
        color: panel.color
        visible: panel.visible
    }
    default property alias data: content.data
}
