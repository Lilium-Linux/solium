import QtQuick

Item {
    id: tile
    property var entry: ({})
    property var settings: ({icon:64,labels:true,fontScale:1,reduceMotion:false})
    property bool selected: false
    property bool editing: false
    property bool artHidden: false
    property bool touchInput: false
    signal picked(int modifiers)
    signal opened(var origin)
    signal menuRequested(real px,real py)
    signal dragStarted(real px,real py)
    signal dragMoved(real px,real py)
    signal dragEnded(bool canceled)
    signal editRequested()
    function artRect() {var p=art.mapToItem(null,0,0);return{x:p.x,y:p.y,size:art.width}}
    Rectangle { anchors.fill: parent; radius: 16; color: tile.selected ? "#40936dff" : "transparent"; border { color:tile.selected?"#aaad8aff":"transparent";width:1 } }
    Item {
        id: art
        anchors { top: parent.top; topMargin: 5; horizontalCenter: parent.horizontalCenter }
        width: Math.min(tile.settings.icon||64,parent.width-12); height: width
        opacity: tile.artHidden ? 0 : 1
        scale: pointer.pressed ? 0.94 : 1
        Behavior on scale { NumberAnimation { duration: tile.settings.reduceMotion ? 0 : 120 } }
        Image { id: image; anchors.fill:parent; source:tile.entry.icon ? "image://solium/icon/"+encodeURIComponent(tile.entry.icon)+"?size="+(art.width*2) : ""; sourceSize.width:art.width*2;sourceSize.height:art.height*2;fillMode:Image.PreserveAspectFit;mipmap:true }
        Rectangle { visible:image.status!==Image.Ready;anchors.fill:parent;radius:14;color:tile.entry.isDir?"#7354b5":"#596077"
            Text { anchors.centerIn:parent;text:tile.entry.isDir?"▰":(tile.entry.name||"?").slice(0,1);color:"white";font.pixelSize:28;textFormat:Text.PlainText } }
        Rectangle { visible:tile.entry.isLauncher&&!tile.entry.trusted;anchors {right:parent.right;bottom:parent.bottom}
width:18;height:18;radius:7;color:"#56416e"
            Text { anchors.centerIn:parent;text:"!";color:"white";font.pixelSize:12 } }
    }
    Text { anchors {top:art.bottom;topMargin:5;left:parent.left;right:parent.right}
visible:tile.settings.labels
        text:tile.entry.displayName||tile.entry.name||"";textFormat:Text.PlainText;horizontalAlignment:Text.AlignHCenter
        color:"#ffffff";style:Text.Outline;styleColor:"#80000000";font {family:"Noto Sans";pixelSize:12*(tile.settings.fontScale||1)}
maximumLineCount:2;wrapMode:Text.Wrap;elide:Text.ElideRight }
    Accessible.role: Accessible.Button
    Accessible.name: entry.displayName||entry.name||"Desktop item"
    MouseArea {
        id:pointer;anchors.fill:parent;acceptedButtons:Qt.LeftButton|Qt.RightButton
        property real startX:0;property real startY:0;property bool moving:false;property bool held:false
        onPressed: function(mouse){
            var p=mapToItem(null,mouse.x,mouse.y);startX=p.x;startY=p.y;moving=false;held=false
            tile.touchInput=mouse.source!==Qt.MouseEventNotSynthesized
            if(mouse.button===Qt.RightButton){tile.menuRequested(p.x,p.y);return}
            tile.picked(mouse.modifiers)
        }
        onPositionChanged:function(mouse){if(!pressed||pressedButtons!==Qt.LeftButton)return
            var p=mapToItem(null,mouse.x,mouse.y)
            if(!moving&&Math.hypot(p.x-startX,p.y-startY)>8){if(tile.touchInput&&!tile.editing)return;moving=true;tile.dragStarted(startX,startY)}
            if(moving)tile.dragMoved(p.x,p.y)
        }
        onReleased:function(mouse){if(mouse.button!==Qt.LeftButton)return
            if(moving){tile.dragEnded(false);moving=false;mouse.accepted=true}
            else if(!held&&!tile.editing&&(tile.touchInput||tile.settings.open==='single'))tile.opened(tile.artRect())
        }
        onCanceled:{if(moving)tile.dragEnded(true);moving=false}
        onDoubleClicked:function(mouse){if(!moving&&!tile.editing&&!held&&tile.settings.open!=='single')tile.opened(tile.artRect())}
        onPressAndHold:{held=true;tile.editRequested()}
    }
}
