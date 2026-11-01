import QtQuick

// Stretched in the real pane; presenting that pane at a square cancels it.
Item {
    id:art
    property var windowValues:({})
    readonly property var flight:windowValues.desktopFlight||null
    property bool dormant:!flight
    opacity:flight&&!timing.revealed?1:0
    FlightVisibility{id:timing;values:art.windowValues}
    Behavior on opacity{enabled:timing.revealed;NumberAnimation{duration:80}}
    Item {width:parent.height;height:parent.height;transform:Scale{xScale:art.height>0?art.width/art.height:1}
        Rectangle{anchors.fill:parent;radius:width/4;color:'#62516f';visible:image.status!==Image.Ready}
        Image{id:image;anchors.fill:parent;source:art.flight&&art.flight.icon?'image://solium/icon/'+encodeURIComponent(art.flight.icon)+'?size=256':'';sourceSize{width:256;height:256} fillMode:Image.Stretch;mipmap:true}
    }
}
