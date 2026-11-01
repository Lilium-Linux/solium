import QtQuick
import "Layout.js" as Layout

Item {
    id: controls
    required property var desktop
    anchors.fill:parent
    visible:desktop.panel!==""
    z:200000
    MouseArea {anchors.fill:parent;onClicked:controls.desktop.panel="";acceptedButtons:Qt.AllButtons}
    Rectangle {
        id:card
        width:Math.min(360,controls.width-32)
        height:Math.min(controls.height-64,content.height+64)
        x:Math.max(16,Math.min(controls.width-width-16,controls.desktop.panelX))
        y:Math.max(16,Math.min(controls.height-height-16,controls.desktop.panelY))
        radius:22;color:controls.desktop.surface;border{width:1;color:controls.desktop.prefs.dark?"#4f435f":"#d8cde8"}
        MouseArea{anchors.fill:parent}
        Text {x:18;y:17;width:parent.width-60;text:({menu:"Desktop",icon:"Desktop item",settings:"Desktop settings",apps:"Add application",widgets:"Add widget"})[controls.desktop.panel]||"Desktop"
            color:controls.desktop.foreground;font {family:"Noto Sans";pixelSize:17;weight:Font.DemiBold}
textFormat:Text.PlainText}
        Action {x:parent.width-48;y:10;width:36;text:"×";dark:controls.desktop.prefs.dark;onTriggered:controls.desktop.panel=""}
        Flickable {anchors{left:parent.left;right:parent.right;top:parent.top;bottom:parent.bottom;topMargin:54;bottomMargin:10;leftMargin:10;rightMargin:10}
            contentHeight:content.height;clip:true;boundsBehavior:Flickable.StopAtBounds
            Column {
                id:content;width:parent.width;spacing:3
                Column {width:parent.width;visible:controls.desktop.panel==='menu';spacing:2
                    Repeater {model:["Edit desktop","Add widget","Add application","Arrange items","Open Desktop folder","Desktop settings"]
                        Action {required property int index;required property string modelData;width:parent.width;text:modelData;dark:controls.desktop.prefs.dark
                            onTriggered:{var d=controls.desktop;if(index===0){d.editing=true;d.panel=''}else if(index===1)d.panel='widgets';else if(index===2)d.panel='apps';else if(index===3){d.commit(Layout.arrange(d.state,d.output,d.area.width,d.area.height));d.panel=''}else if(index===4){d.send('openFolder');d.panel=''}else d.panel='settings'} }
                    }
                }
                Column {width:parent.width;visible:controls.desktop.panel==='icon';spacing:2
                    Action {width:parent.width;text:"Open";dark:controls.desktop.prefs.dark;onTriggered:controls.desktop.openItem(controls.desktop.menuId)}
                    Action {width:parent.width;text:"Hide icon";dark:controls.desktop.prefs.dark;onTriggered:controls.desktop.hideIcon(controls.desktop.menuId)}
                    Repeater {model:controls.desktop.outputs
                        Action {required property string modelData;width:parent.width;text:"Move to "+modelData;dark:controls.desktop.prefs.dark
                            onTriggered:{var d=controls.desktop,s=Layout.copy(d.state);if(s.items[d.menuId]){s.items[d.menuId].monitor=modelData;s.items[d.menuId].grid={col:-1,row:-1};s.items[d.menuId].free=null;d.commit(s)}d.panel=''} }
                    }
                    Action {width:parent.width;text:"Edit desktop";dark:controls.desktop.prefs.dark;onTriggered:{controls.desktop.editing=true;controls.desktop.panel=''}}
                }
                Column {width:parent.width;visible:controls.desktop.panel==='settings';spacing:2
                    Repeater {model:[{key:'grid',label:'Snap to grid'},{key:'labels',label:'Icon labels'},{key:'dark',label:'Dark appearance'},{key:'transparency',label:'Translucent controls'},{key:'reduceMotion',label:'Reduce motion'}]
                        Action {required property var modelData;width:parent.width;text:(controls.desktop.prefs[modelData.key]?"✓  ":"     ")+modelData.label;checked:!!controls.desktop.prefs[modelData.key];dark:controls.desktop.prefs.dark
                            onTriggered:controls.desktop.updateSetting(modelData.key,!controls.desktop.prefs[modelData.key])}
                    }
                    Action {width:parent.width;text:"Open: "+(controls.desktop.prefs.open==='double'?"double-click":"single-click");dark:controls.desktop.prefs.dark
                        onTriggered:controls.desktop.updateSetting('open',controls.desktop.prefs.open==='double'?'single':'double')}
                    Repeater {model:[{key:'icon',label:'Icon size',step:8,min:32,max:96},{key:'cell',label:'Grid spacing',step:8,min:80,max:180},{key:'margin',label:'Margins',step:8,min:0,max:96},{key:'fontScale',label:'Text scale',step:0.1,min:0.75,max:2},{key:'motionMs',label:'Animation (ms)',step:100,min:0,max:2000}]
                        Row {required property var modelData;width:parent.width;height:40
                            Text {width:parent.width-88;anchors.verticalCenter:parent.verticalCenter;leftPadding:12;text:modelData.label+": "+Math.round(controls.desktop.prefs[modelData.key]*100)/100;color:controls.desktop.foreground;font.pixelSize:13}
                            Action {width:40;text:"−";dark:controls.desktop.prefs.dark;onTriggered:controls.desktop.updateSetting(modelData.key,Math.max(modelData.min,Math.round((controls.desktop.prefs[modelData.key]-modelData.step)*100)/100))}
                            Action {width:40;text:"+";dark:controls.desktop.prefs.dark;onTriggered:controls.desktop.updateSetting(modelData.key,Math.min(modelData.max,Math.round((controls.desktop.prefs[modelData.key]+modelData.step)*100)/100))}
                        }
                    }
                    Action {width:parent.width;text:"Reset appearance and spacing";dark:controls.desktop.prefs.dark;onTriggered:{var s=Layout.copy(controls.desktop.state);s.settings=Layout.defaults().settings;controls.desktop.commit(s)}}
                    Text {leftPadding:12;topPadding:14;bottomPadding:5;text:"Hidden desktop icons";color:controls.desktop.secondary;font.pixelSize:12}
                    Repeater {model:Object.keys(controls.desktop.state.items).filter(function(id){return controls.desktop.state.items[id].hidden})
                        Action {required property string modelData;width:parent.width;text:"Restore "+modelData.split('/').pop();dark:controls.desktop.prefs.dark;onTriggered:controls.desktop.restoreIcon(modelData)}
                    }
                }
                Column {width:parent.width;visible:controls.desktop.panel==='apps';spacing:2
                    Rectangle {width:parent.width;height:40;radius:10;color:controls.desktop.prefs.dark?'#302b39':'#e9e1f3'
                        TextInput {id:appSearch;anchors {fill:parent;margins:10}
color:controls.desktop.foreground;font.pixelSize:13;selectByMouse:true
                            Text{visible:!appSearch.text&&!appSearch.activeFocus;text:"Search installed applications";color:controls.desktop.secondary;font.pixelSize:13} }
                    }
                    Repeater {model:controls.desktop.applications.filter(function(a){return a.name.toLowerCase().indexOf(appSearch.text.toLowerCase())>=0})
                        Action {required property var modelData;width:parent.width;text:modelData.name;dark:controls.desktop.prefs.dark
                            onTriggered:{controls.desktop.send('addApplication',{id:modelData.id});controls.desktop.panel=''}}
                    }
                }
                WidgetPicker {width:parent.width;visible:controls.desktop.panel==='widgets';desktop:controls.desktop}
            }
        }
    }
}
