import QtQuick
import Solium
import ".."

Item {
    id:media
    property var desktop:null
    property var options:({})
    readonly property var player:typeof Media !== 'undefined' ? Media.active : null
    readonly property color ink:desktop?desktop.foreground:'#f2eff8'
    readonly property bool available:!!player&&!!player.present
    Rectangle {id:cover;x:0;y:0;width:Math.min(76,parent.height*0.53);height:width;radius:16;color:'#46374f';clip:true
        Image{anchors.fill:parent;source:media.available?media.player.art:'';fillMode:Image.PreserveAspectCrop}
        Text{visible:!media.available||!media.player.art;anchors.centerIn:parent;text:'♪';color:'#bd9aff';font.pixelSize:34}}
    Column {anchors {left:cover.right;right:parent.right;leftMargin:14;top:parent.top}
spacing:7
        Text{width:parent.width;text:media.available?(media.player.title||'Unknown track'):'Nothing playing';textFormat:Text.PlainText;color:media.ink;font {pixelSize:14;weight:Font.DemiBold}
maximumLineCount:2;wrapMode:Text.WordWrap;elide:Text.ElideRight}
        Text{width:parent.width;text:media.available?(media.player.artist||media.player.album||''):'Start playback in a music or video app';textFormat:Text.PlainText;color:media.ink;opacity:0.6;font.pixelSize:12;elide:Text.ElideRight}
    }
    Row {anchors {horizontalCenter:parent.horizontalCenter;bottom:parent.bottom}
spacing:8
        Action{width:60;height:34;text:'‹‹';enabled:media.available&&media.player.canGoPrevious;dark:!media.desktop||media.desktop.prefs.dark;onTriggered:media.player.previous()}
        Action{width:70;height:34;text:media.available&&media.player.playing?'Pause':'Play';enabled:media.available&&(media.player.playing?media.player.canPause:media.player.canPlay);checked:true;dark:!media.desktop||media.desktop.prefs.dark;onTriggered:media.player.playPause()}
        Action{width:60;height:34;text:'››';enabled:media.available&&media.player.canGoNext;dark:!media.desktop||media.desktop.prefs.dark;onTriggered:media.player.next()}
    }
}
