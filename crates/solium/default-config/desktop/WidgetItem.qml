import QtQuick
import "Layout.js" as Layout

Rectangle {
    id:card
    required property var desktop
    required property var entry
    property bool dragging: false
    radius:26
    color:desktop.prefs.dark ? (desktop.prefs.transparency?"#ee272331":"#272331") : (desktop.prefs.transparency?"#f5f7f2fd":"#f7f2fd")
    border{width:desktop.editing?1.5:1;color:desktop.editing?"#a38aff":desktop.prefs.dark?"#51445f":"#e6dcef"}
    clip:false
    Accessible.role:Accessible.Grouping
    Accessible.name:entry.kind+" widget"
    Loader {anchors{fill:parent;margins:16;topMargin:card.desktop.editing?42:16;bottomMargin:card.desktop.editing?38:16}
        source:({clock:'widgets/Clock.qml',calendar:'widgets/Calendar.qml',media:'widgets/NowPlaying.qml',stats:'widgets/Stats.qml'})[card.entry.kind]||''
        onLoaded:{item.desktop=card.desktop;item.options=Qt.binding(function(){return card.entry.options||{}})}
    }
    Rectangle {visible:desktop.editing;width:parent.width;height:32;color:"transparent"
        Text{anchors {left:parent.left;leftMargin:16;verticalCenter:parent.verticalCenter}
text:"⠿  "+({clock:"Clock",calendar:"Calendar",media:"Now playing",stats:"System"})[card.entry.kind];color:card.desktop.secondary;font.pixelSize:12}
        MouseArea {anchors {left:parent.left;right:remove.left;top:parent.top;bottom:parent.bottom}
cursorShape:Qt.SizeAllCursor
            property real sx:0;property real sy:0;property bool started:false
            onPressed:function(mouse){var p=mapToItem(null,mouse.x,mouse.y);sx=p.x;sy=p.y;started=false;card.desktop.choose(card.entry.id,0)}
            onPositionChanged:function(mouse){if(!pressed)return;var p=mapToItem(null,mouse.x,mouse.y);if(!started&&Math.hypot(p.x-sx,p.y-sy)>5){started=true;card.desktop.beginDrag(card.entry.id,sx,sy)}if(started)card.desktop.moveDrag(p.x,p.y)}
            onReleased:{if(started)card.desktop.endDrag(false);started=false}
            onCanceled:{if(started)card.desktop.endDrag(true);started=false}
        }
        Action{id:remove;anchors.right:parent.right;width:34;height:30;text:"×";dark:card.desktop.prefs.dark;onTriggered:card.desktop.removeWidget(card.entry.id)}
    }
    Row {visible:desktop.editing;anchors {bottom:parent.bottom;bottomMargin:5;horizontalCenter:parent.horizontalCenter}
spacing:3
        Repeater{model:Layout.families[card.entry.kind]||[]
            Action{required property string modelData;width:49;height:28;text:modelData;checked:card.entry.size===modelData;dark:card.desktop.prefs.dark;onTriggered:card.desktop.resizeWidget(card.entry.id,modelData)} }
        Action{width:36;height:28;text:"⋯";dark:card.desktop.prefs.dark;onTriggered:optionsMenu.visible=!optionsMenu.visible}
    }
    MouseArea {anchors.fill:parent;acceptedButtons:Qt.RightButton;onClicked:card.desktop.editing=true;propagateComposedEvents:true;z:-1}
    Rectangle{id:optionsMenu;visible:false;z:100;x:8;y:card.height+8;width:200;height:optionColumn.height+12;radius:12;color:card.desktop.surface;border{width:1;color:"#706087"}
        Column{id:optionColumn;anchors {left:parent.left;right:parent.right;top:parent.top;margins:6}
spacing:2
            Action{width:parent.width;text:card.entry.options&&card.entry.options.twelveHour?"Use 24-hour clock":"Use 12-hour clock";visible:card.entry.kind==='clock';height:visible?36:0;dark:card.desktop.prefs.dark
                onTriggered:{var s=Layout.copy(card.desktop.state);s.items[card.entry.id].options.twelveHour=!(card.entry.options&&card.entry.options.twelveHour);card.desktop.commit(s);optionsMenu.visible=false}}
            Repeater{model:card.desktop.outputs;Action{required property string modelData;width:parent.width;text:"Move to "+modelData;dark:card.desktop.prefs.dark
                onTriggered:{var s=Layout.copy(card.desktop.state);s.items[card.entry.id].monitor=modelData;s.items[card.entry.id].grid={col:-1,row:-1};s.items[card.entry.id].free=null;card.desktop.commit(s);optionsMenu.visible=false}}}
        }
    }
}
