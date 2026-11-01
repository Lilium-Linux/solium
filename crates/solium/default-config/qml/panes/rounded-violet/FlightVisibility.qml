import QtQuick

Item {
    id:timing
    property var values:({})
    readonly property var flight:values.desktopFlight||null
    property int serial:0
    property bool revealed:true
    onFlightChanged:{
        if(flight&&flight.serial!==serial){serial=flight.serial;revealed=false;reveal.restart()}
        else if(!flight){revealed=true;reveal.stop()}
    }
    Timer{id:reveal;interval:timing.flight?120*timing.flight.pace:120;onTriggered:timing.revealed=true}
}
