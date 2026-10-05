// Quick search (04-ui.md §4.7), first version: a centred panel on the
// active monitor, a text field with the live keyboard layout beside it, and
// results grouped into applications, open windows and a few compositor
// commands.
//
// **Where the matching happens, and why not here.** `Apps` and `Windows`,
// the QML models every other piece of this bar reads, are built for live
// delegates (`qml/preview/Dock.qml`'s own module doc goes through the same
// choice): there is no documented way to walk either one as a plain array
// from JavaScript, only through a `Repeater` or `ListView`, and nothing
// exposes a positional `get(i)` the way `Apps.get(id)` does by name. Lua's
// `sol.apps()` and `sol.windows()` are exactly that plain array, so this
// file sends what is typed with `Solium.send("search.query", { text: ... })`
// and `lua/preview/search.lua` answers with the ranked rows, the same round
// trip `preview/dock.lua` already uses for its pins (`Shell.qml`'s
// `searchOpen`/`searchResults` properties). Activating an app or a window
// row skips that round trip entirely and sends the native action straight,
// exactly as `qml/preview/DockIcon.qml` already does.
//
// **Cut for this first version** (04-ui.md's own Showcase/P1 marks, plus
// what "small" asked for beyond them, both listed in `search.lua`'s own
// module doc too): the detail pane, a row's own actions (Tab), Alt+digits,
// Ctrl+Return, the empty-query suggestions, the settings and desktop-file
// sources, and the container transform from the bar's own search button --
// the bar has no search button yet either. The layout-correction pass
// ("us,ru correction with no table") is not here either; see `search.lua`'s
// module doc for why.
//
// No `QtQuick.Controls` anywhere in this tree (`build.rs` links only
// `Qt6Quick`), so the field below is a plain `TextInput`, styled by hand,
// not a Controls `TextField` -- the one difference from `docs/shell-boundary.md`'s
// own inline example.

import QtQuick
import Solium

Item {
    id: root

    // `Shell.qml`'s own properties, written here by name -- see this file's
    // own module doc, and `Shell.qml`'s for why they are plain items rather
    // than a model of their own. Read-only from here: a row's own click or
    // Escape closes the panel at once, locally (`shown` below), and tells
    // Lua, which is what eventually turns this back to `false` -- writing
    // straight to a property Shell.qml binds from outside would silently
    // break that binding the first time this file ever closed the panel
    // itself (Qt Quick's own rule: an imperative assignment replaces a
    // binding, it does not coexist with it).
    property bool open: false
    property var results: []

    // The panel's own, locally-assignable state, kept in step with `open`
    // whenever Lua's side of it changes -- see the property doc above.
    property bool shown: false
    onOpenChanged: shown = open

    anchors.fill: parent
    visible: root.shown && Solium.monitor.active

    // A `QVariantList` crossing from Lua iterates fine but fails
    // `Array.isArray` (`qml/preview/Dock.qml`'s own note, measured the same
    // way): `rows.length` and indexing both want a real array.
    function toArray(value) {
        if (Array.isArray(value)) {
            return value
        }
        const out = []
        if (value) {
            for (let i = 0; i < value.length; i++) {
                out.push(value[i])
            }
        }
        return out
    }
    readonly property var rows: toArray(root.results)

    // A header row (`search.lua`'s own `append`) is not a result: selection
    // and activation both skip it.
    function selectable(index) {
        return index >= 0 && index < rows.length && rows[index].kind !== "header"
    }

    function firstSelectable() {
        for (let i = 0; i < rows.length; i++) {
            if (rows[i].kind !== "header") {
                return i
            }
        }
        return -1
    }

    function move(delta) {
        if (rows.length === 0) {
            return
        }
        let index = list.currentIndex
        for (let step = 0; step < rows.length; step++) {
            index += delta
            if (index < 0 || index >= rows.length) {
                return
            }
            if (selectable(index)) {
                list.currentIndex = index
                return
            }
        }
    }

    function activate(index) {
        if (!selectable(index)) {
            return
        }
        const row = rows[index]
        if (row.kind === "app") {
            Solium.send("apps.launch", { id: row.id })
        } else if (row.kind === "window") {
            Solium.send("windows.focus", { id: row.windowId })
        } else if (row.kind === "command") {
            Solium.send("search.run", { key: row.key })
        }
        close()
    }

    function close() {
        field.text = ""
        root.shown = false
        Solium.send("search.closed")
    }

    // `Qt.callLater`, not a direct assignment: a `ListView` resets its own
    // `currentIndex` to -1 the moment its `model` changes identity, which a
    // fresh array from Lua does on every keystroke -- so a direct
    // assignment here was overwritten right back to -1 by the view's own
    // reaction to that same change, measured directly (no row ever showed
    // selected). Deferred, this runs after the view has settled on the new
    // model.
    onRowsChanged: Qt.callLater(function() { list.currentIndex = root.firstSelectable() })
    onVisibleChanged: if (visible) {
        field.text = ""
        field.forceActiveFocus()
    }

    // The click-outside grab (docs/shell-boundary.md, "Popups that hold the
    // pointer"): held only while open, so nothing claims the pointer the
    // rest of the time, and left at the default `outside_click` ("swallow"),
    // which is what "a click outside closes" (04-ui.md) asks for.
    Grab {
        name: "search"
        target: panel
        // Gated the same as `visible`, not just `shown`: `shown` is one
        // value shared by every monitor's own instance of this scene
        // (`searchOpen` crosses once from Lua to all of them), so without
        // the same `Solium.monitor.active` check a monitor the panel is not
        // even drawn on would still hold a grab on it -- swallowing a click
        // over a window there for no panel anyone can see.
        active: root.shown && Solium.monitor.active
        onDismissed: root.close()
    }

    Rectangle {
        id: panel
        anchors.horizontalCenter: parent.horizontalCenter
        // The scene's own item tree is one canvas per monitor, local-origin
        // (docs/shell-boundary.md: "a canvas... the size of its whole
        // monitor"), but `Solium.monitor.area`/`whole` are rectangles in the
        // *global* space ("a scene that wants its own coordinates subtracts
        // `whole.x` and `whole.y`") -- so 22% of the work area's height,
        // from the work area's own top (below a reserving dock, say), is
        // this, not `area.y` on its own.
        y: (Solium.monitor.area.y - Solium.monitor.whole.y) + Solium.monitor.area.height * 0.22
        width: 420
        height: content.implicitHeight + 16
        radius: 14
        color: Theme.surface
        border { width: 1; color: Theme.edge }

        Column {
            id: content
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 8 }
            spacing: 8

            Rectangle {
                width: parent.width
                height: 40
                radius: 10
                color: Theme.surfaceInactive
                border { width: 1; color: Theme.edge }

                Row {
                    anchors.fill: parent
                    anchors.margins: 8
                    spacing: 8

                    TextInput {
                        id: field
                        width: parent.width - layoutChip.width - parent.spacing
                        anchors.verticalCenter: parent.verticalCenter
                        color: Theme.text
                        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                        clip: true
                        focus: true
                        selectByMouse: true

                        // The keyboard, when this field asks
                        // (docs/shell-boundary.md, "The keyboard, when an
                        // item asks"): held only while visible and focused,
                        // given back the moment either stops.
                        Solium.keyboard.wants: activeFocus
                        Solium.keyboard.claims: ["Escape", "Return", "Up", "Down"]

                        onTextChanged: Solium.send("search.query", { text: text })

                        Keys.onPressed: function(event) {
                            if (event.key === Qt.Key_Escape) {
                                root.close()
                                event.accepted = true
                            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                                root.activate(list.currentIndex)
                                event.accepted = true
                            } else if (event.key === Qt.Key_Up) {
                                root.move(-1)
                                event.accepted = true
                            } else if (event.key === Qt.Key_Down) {
                                root.move(1)
                                event.accepted = true
                            }
                        }
                    }

                    LayoutChip { id: layoutChip; anchors.verticalCenter: parent.verticalCenter }
                }
            }

            ListView {
                id: list
                width: parent.width
                height: Math.min(420, contentHeight)
                clip: true
                model: root.rows
                currentIndex: -1

                delegate: Item {
                    width: list.width
                    height: modelData.kind === "header" ? 22 : 40

                    Text {
                        visible: modelData.kind === "header"
                        anchors { left: parent.left; leftMargin: 6; verticalCenter: parent.verticalCenter }
                        text: modelData.title
                        color: Theme.textDim
                        font { pixelSize: Theme.fontSize - 1; family: Theme.fontFamily; bold: true }
                    }

                    Rectangle {
                        visible: modelData.kind !== "header"
                        anchors.fill: parent
                        radius: 6
                        color: index === list.currentIndex ? Theme.control : "transparent"

                        Row {
                            anchors.fill: parent
                            anchors.margins: 6
                            spacing: 8

                            Image {
                                width: 24
                                height: 24
                                anchors.verticalCenter: parent.verticalCenter
                                asynchronous: true
                                fillMode: Image.PreserveAspectFit
                                readonly property string iconName: modelData.icon || ""
                                visible: iconName.length > 0
                                source: visible ? ("image://solium/icon/" + encodeURIComponent(iconName) + "?size=24") : ""
                            }

                            Column {
                                anchors.verticalCenter: parent.verticalCenter
                                Text {
                                    text: modelData.title || ""
                                    color: Theme.text
                                    font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                                }
                                Text {
                                    visible: (modelData.subtitle || "").length > 0
                                    text: modelData.subtitle || ""
                                    color: Theme.textDim
                                    font { pixelSize: Theme.fontSize - 1; family: Theme.fontFamily }
                                }
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.activate(index)
                        }
                    }
                }
            }

            Text {
                width: parent.width
                visible: root.rows.length === 0 && field.text.length > 0
                text: "Nothing found for '" + field.text + "'"
                color: Theme.textDim
                font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                wrapMode: Text.Wrap
            }
        }
    }
}
