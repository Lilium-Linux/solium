// Snapping's own scene (#455): the preview of where a dragged window will go,
// and the holders between windows snapped side by side. One instance on every
// monitor, over the windows; `lua/snap.lua` declares it and hands it what to
// draw, and nothing in the compositor knows this file exists.
//
// It draws and takes no input. A holder lies over the seam between two
// windows, and the press there goes on to their own resize borders, which
// show the resize cursor and move the seam as a tiled seam moves.
// `crates/solium/tests/scenarios/snap-drawn.lua`, through
// `scenario::tests::every_scenario_on_the_qt_thread_passes`.

import QtQuick
import Solium

Item {
    id: root

    // Where a window let go now goes: `{ monitor, kind, x, y, w, h }` in this
    // monitor's own pixels, or false.
    property var zone: false
    // Each holder: `{ monitor, vertical, x, y, w, h, dragging }`, the seam as
    // a line -- `w` 0 for a vertical one, `h` 0 for a horizontal one.
    property var seams: []
    // `floating.snap.drag.preview` and `.divider`: how to draw them.
    property var preview: ({})
    property var divider: ({})

    Solium.input: false

    function value(table, key, fallback) {
        return table && table[key] !== undefined && table[key] !== null ? table[key] : fallback
    }

    // A colour as the configuration writes one, "#rrggbb" or "#rrggbbaa", the
    // alpha last as everywhere else in it -- QML's own eight digits put it
    // first.
    function colour(text, fallback) {
        const given = typeof text === "string" ? text : fallback
        const hex = /^#([0-9a-fA-F]{6})([0-9a-fA-F]{2})?$/.exec(given)
        if (!hex)
            return given
        const channel = at => parseInt(hex[1].substr(at, 2), 16) / 255
        return Qt.rgba(channel(0), channel(2), channel(4), hex[2] ? parseInt(hex[2], 16) / 255 : 1)
    }

    // The monitor this instance is on; "" for a scene on none.
    readonly property string screen: Solium.monitor ? Solium.monitor.name : ""
    readonly property bool previewing: !!zone && zone.monitor === screen
    readonly property int duration: value(preview, "duration", 160)
    readonly property real inset: value(preview, "inset", 6)

    // The preview. It appears where its zone is, growing a little as it
    // fades in; moved to another zone while shown, it slides and stretches
    // there; and it fades out where it last was.
    Rectangle {
        id: box

        // The zone drawn, kept once there is none so it fades out in place.
        property var at: ({ x: 0, y: 0, w: 0, h: 0 })
        // Whether the geometry glides: only while something of it is seen,
        // so a preview appearing is at its zone from its first frame.
        readonly property bool seen: opacity > 0.01

        x: at.x + root.inset
        y: at.y + root.inset
        width: Math.max(0, at.w - root.inset * 2)
        height: Math.max(0, at.h - root.inset * 2)
        visible: seen
        opacity: root.previewing ? 1 : 0
        scale: root.previewing ? 1 : 0.96
        radius: root.value(root.preview, "radius", 12)
        color: root.colour(root.preview.fill, "#ffffff24")
        border.color: root.colour(root.preview.border, "#ffffff80")
        border.width: root.value(root.preview, "border_width", 2)

        Behavior on opacity { NumberAnimation { duration: root.duration; easing.type: Easing.OutCubic } }
        Behavior on scale { NumberAnimation { duration: root.duration; easing.type: Easing.OutCubic } }
        Behavior on x { enabled: box.seen; NumberAnimation { duration: root.duration; easing.type: Easing.OutCubic } }
        Behavior on y { enabled: box.seen; NumberAnimation { duration: root.duration; easing.type: Easing.OutCubic } }
        Behavior on width { enabled: box.seen; NumberAnimation { duration: root.duration; easing.type: Easing.OutCubic } }
        Behavior on height { enabled: box.seen; NumberAnimation { duration: root.duration; easing.type: Easing.OutCubic } }
    }

    // Asked here rather than read off `previewing`, which may not have
    // heard of the new zone yet when this runs.
    function follow() {
        if (!!zone && zone.monitor === screen)
            box.at = zone
    }
    onZoneChanged: follow()
    onScreenChanged: follow()
    Component.onCompleted: follow()

    // A holder: a pill across the middle of each seam on this monitor, longer
    // and brighter while its seam is being dragged.
    Repeater {
        // A list as the configuration hands one over; an empty Lua table
        // arrives as an object with no length.
        model: root.seams && root.seams.length > 0 ? root.seams : []

        delegate: Rectangle {
            required property var modelData

            readonly property bool vertical: modelData.vertical === true
            readonly property bool held: modelData.dragging === true && Solium.cursor.pressed
            readonly property real span: vertical ? modelData.h : modelData.w
            readonly property real thick: root.value(root.divider, "thickness", 6)
            readonly property real long: Math.min(span, root.value(root.divider, "length", 56) * (held ? 1.5 : 1))

            visible: modelData.monitor === root.screen && span > 0
            width: vertical ? thick : long
            height: vertical ? long : thick
            x: vertical ? modelData.x - thick / 2 : modelData.x + (span - long) / 2
            y: vertical ? modelData.y + (span - long) / 2 : modelData.y - thick / 2
            radius: thick / 2
            color: root.colour(root.divider.color, Theme.accent)
            opacity: held ? 1 : root.value(root.divider, "opacity", 0.75)
        }
    }
}
