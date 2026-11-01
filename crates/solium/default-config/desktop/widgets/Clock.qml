import QtQuick

Item {
    id:clock
    property var desktop: null
    property var options: ({})
    property date now:new Date()
    readonly property color ink:desktop?desktop.foreground:'#f2eff8'
    readonly property real fontScale:desktop?desktop.prefs.fontScale:1
    Timer {interval:1000;running:clock.visible;repeat:true;onTriggered:clock.now=new Date()}
    Column {anchors {centerIn:parent}
width:parent.width;spacing:5
        Text{width:parent.width;text:Qt.formatDate(clock.now,'dddd').toUpperCase();color:'#b393ff';font {family:'Noto Sans';pixelSize:Math.max(11,Math.min(14,clock.height/9))*clock.fontScale;weight:Font.DemiBold}
elide:Text.ElideRight}
        Text{width:parent.width;text:Qt.formatTime(clock.now,clock.options.twelveHour?'h:mm AP':'HH:mm');color:clock.ink;font {family:'Noto Sans';pixelSize:Math.max(22,Math.min(clock.options.twelveHour?42:54,clock.width/(clock.options.twelveHour?4:3.1),clock.height*0.44))*clock.fontScale;weight:Font.Light}
fontSizeMode:Text.Fit;minimumPixelSize:14}
        Text{width:parent.width;text:Qt.formatDate(clock.now,'d MMMM yyyy');color:clock.ink;opacity:0.72;font {family:'Noto Sans';pixelSize:13*clock.fontScale}
elide:Text.ElideRight}
    }
}
