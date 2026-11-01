import QtQuick

Column {
    id:stats
    property var desktop:null
    property var options:({})
    readonly property var dataPoints:desktop?desktop.stats:({})
    readonly property color ink:desktop?desktop.foreground:'#f2eff8'
    function rate(n){return n===undefined||n<0?'—':n>=1048576?(n/1048576).toFixed(1)+' MB/s':(n/1024).toFixed(0)+' KB/s'}
    spacing:Math.max(4,Math.min(14,height/12))
    Text{text:'SYSTEM';color:'#b393ff';font{pixelSize:12;weight:Font.DemiBold}}
    Repeater {model:[{name:'CPU',value:stats.dataPoints.cpu},{name:'Memory',value:stats.dataPoints.memory}]
        Column{required property var modelData;width:stats.width;spacing:4
            Text{width:parent.width;text:modelData.name+'  '+(modelData.value===undefined||modelData.value<0?'—':Math.round(modelData.value)+'%');color:stats.ink;font.pixelSize:13}
            Rectangle{width:parent.width;height:5;radius:3;color:'#604b425b';Rectangle{height:parent.height;width:parent.width*Math.max(0,Math.min(1,(modelData.value||0)/100));radius:3;color:'#a38aff'}}
        }
    }
    Text{width:parent.width;text:'↓ '+stats.rate(stats.dataPoints.down)+'    ↑ '+stats.rate(stats.dataPoints.up);color:stats.ink;opacity:0.65;font.pixelSize:11;elide:Text.ElideRight}
}
