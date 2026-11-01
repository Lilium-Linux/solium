import QtQuick

Item {
    id:calendar
    property var desktop:null
    property var options:({})
    property date today:new Date()
    property int offset:0
    readonly property date month:new Date(today.getFullYear(),today.getMonth()+offset,1)
    readonly property int leading:(month.getDay()+6)%7
    readonly property int days:new Date(month.getFullYear(),month.getMonth()+1,0).getDate()
    readonly property color ink:desktop?desktop.foreground:'#f2eff8'
    Timer{interval:60000;running:true;repeat:true;onTriggered:calendar.today=new Date()}
    Text{x:0;y:0;width:parent.width-56;text:Qt.formatDate(calendar.month,'MMMM yyyy');color:'#b393ff';font {pixelSize:13;weight:Font.DemiBold}
elide:Text.ElideRight}
    Row {anchors {right:parent.right;top:parent.top}
spacing:3
        Text{text:'‹';color:calendar.ink;font.pixelSize:18;width:24;height:24;horizontalAlignment:Text.AlignHCenter;MouseArea{anchors.fill:parent;onClicked:calendar.offset--}}
        Text{text:'›';color:calendar.ink;font.pixelSize:18;width:24;height:24;horizontalAlignment:Text.AlignHCenter;MouseArea{anchors.fill:parent;onClicked:calendar.offset++}}
    }
    Grid {x:0;y:28;width:parent.width;columns:7;rowSpacing:1
        Repeater {model:49
            Rectangle {required property int index
                readonly property int day:index-7-calendar.leading+1
                readonly property bool heading:index<7
                readonly property bool current:!heading&&day===calendar.today.getDate()&&calendar.month.getMonth()===calendar.today.getMonth()&&calendar.month.getFullYear()===calendar.today.getFullYear()
                width:calendar.width/7;height:Math.max(10,Math.min(23,(calendar.height-30)/7));radius:height/2;color:current?'#936dff':'transparent'
                Text{anchors.centerIn:parent;text:parent.heading?['M','T','W','T','F','S','S'][parent.index]:parent.day>0&&parent.day<=calendar.days?parent.day:'';color:parent.current?'white':calendar.ink;opacity:parent.heading?0.5:1;font.pixelSize:Math.max(8,Math.min(12,parent.height*0.72))}
            }
        }
    }
}
