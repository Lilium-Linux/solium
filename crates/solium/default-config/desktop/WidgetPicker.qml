import QtQuick

Column {
    id:picker
    required property var desktop
    spacing:12
    Repeater {
        model:[{kind:'clock',name:'Clock & date',description:'See the time and today’s date.',sizes:['2x2','4x2']},
            {kind:'calendar',name:'Calendar',description:'Browse the current month and nearby dates.',sizes:['2x2','4x2']},
            {kind:'media',name:'Now playing',description:'Control playback from your active player.',sizes:['4x2','4x4']},
            {kind:'stats',name:'System',description:'Check CPU, memory and network activity.',sizes:['2x2','4x2']}]
        Column {
            required property var modelData
            width:picker.width;spacing:6
            Text {leftPadding:8;text:modelData.name;color:picker.desktop.foreground;font{pixelSize:14;weight:Font.DemiBold}}
            Rectangle {width:parent.width;height:110;radius:18;color:picker.desktop.prefs.dark?'#332d3e':'#e9e1f3'
                Loader {anchors {fill:parent;margins:12}
source:'widgets/'+({clock:'Clock',calendar:'Calendar',media:'NowPlaying',stats:'Stats'})[modelData.kind]+'.qml'
                    onLoaded:item.desktop=picker.desktop}
            }
            Text {leftPadding:8;width:parent.width-8;text:modelData.description;wrapMode:Text.WordWrap;color:picker.desktop.secondary;font.pixelSize:12}
            Row {spacing:4
                Repeater {model:modelData.sizes;Action {required property string modelData;width:98;text:'Add '+modelData;checked:true;dark:picker.desktop.prefs.dark
                    property string kind:parent.parent.modelData.kind
                    onTriggered:picker.desktop.addWidget(kind,modelData)}}
            }
        }
    }
}
