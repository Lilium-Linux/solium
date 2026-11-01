import QtQuick
import QtQml.Models
import Solium
import "Layout.js" as Layout

Item {
    id: root
    property var state: Layout.defaults()
    property int revision: 0
    property var outputs: []
    property string primary: ""
    property string desktopPath: ""
    property string saveError: ""
    property var stats: ({})
    property var flights: ({})
    property var files: []
    property var applications: []
    property bool editing: false
    property string panel: ""
    property real panelX: 32
    property real panelY: 32
    property string menuId: ""
    property var selected: ({})
    property var preview: null
    property string dragId: ""
    property real dragX: 0
    property real dragY: 0
    property real dragStartX: 0
    property real dragStartY: 0
    property real dragOffsetX: 0
    property real dragOffsetY: 0
    property real dragScroll: 0
    property real lastPointerY: 0
    property var dragBase: null
    property real canvasHeight: height
    property real canvasWidth: width
    property string trustUri: ""
    property var trustData: ({})
    property string focusedId: ""
    readonly property var prefs: state.settings || Layout.defaults().settings
    readonly property string output: Solium.monitor.name
    readonly property rect area: Qt.rect(Solium.monitor.area.x-Solium.monitor.whole.x,
        Solium.monitor.area.y-Solium.monitor.whole.y,Solium.monitor.area.width,Solium.monitor.area.height)
    readonly property color foreground: prefs.dark ? "#f2eff8" : "#292332"
    readonly property color secondary: prefs.dark ? "#b4abbe" : "#675f72"
    readonly property color surface: prefs.dark ? (prefs.transparency?"#ed24202c":"#24202c") : (prefs.transparency?"#f0f6f1fc":"#f6f1fc")
    Solium.keyboard.wants: activeFocus
    Solium.keyboard.claims: ["Escape","Return","Left","Right","Up","Down","Delete"]

    function send(action,data) { Solium.send("desktop."+action,data||{}) }
    function commit(next) { if(next)send("commit",{base:revision,state:next}) }
    function refreshFiles() {
        var result=[]
        for(var i=0;i<fileIndex.count;i++){var obj=fileIndex.objectAt(i);if(obj&&obj.uri&&!obj.hidden)result.push({uri:obj.uri,name:obj.name,displayName:obj.displayName,icon:obj.icon,isDir:obj.isDir,isLauncher:obj.isLauncher,trusted:obj.trusted})}
        files=result
        reconcile()
        refresh()
    }
    // Only the primary's scene admits new files; each scene pins its own
    // output's items. Run again whenever Lua republishes, so a commit that
    // lost the revision race to another scene is retried against the winner.
    function reconcile() {
        if(!primary||dragId||area.width<=0)return
        var next=output===primary?Layout.reconcile(state,files,primary,area.width,area.height)
            :Layout.pin(state,files,output,area.width,area.height)
        if(JSON.stringify(next)!==JSON.stringify(state))commit(next)
    }
    function refreshApps() {
        var result=[];for(var i=0;i<appIndex.count;i++){var obj=appIndex.objectAt(i);if(obj&&obj.present)result.push({id:obj.appId,name:obj.appName,icon:obj.icon})}
        applications=result.sort(function(a,b){return a.name.localeCompare(b.name)})
    }
    function refresh() {
        if(!state||!state.settings||area.width<=0||!primary)return
        var p=Layout.project(preview||state,files,output,outputs,area.width,area.height)
        canvasHeight=p.height;canvasWidth=p.width
        var wanted={};p.items.forEach(function(i){wanted[i.id]=true})
        for(var n=tiles.count-1;n>=0;n--)if(!wanted[tiles.get(n).key])tiles.remove(n)
        for(var k=0;k<p.items.length;k++){
            var item=p.items[k],found=-1;for(var j=0;j<tiles.count;j++)if(tiles.get(j).key===item.id){found=j;break}
            if(found<0)tiles.append({key:item.id,payload:item})
            else tiles.setProperty(found,"payload",item)
        }
    }
    function choose(id,modifiers) {
        root.forceActiveFocus();focusedId=id
        var next=(modifiers&Qt.ControlModifier)?Layout.copy(selected):{}
        if((modifiers&Qt.ControlModifier)&&next[id])delete next[id];else next[id]=true
        selected=next
    }
    function showPanel(which,x,y,id) {root.forceActiveFocus();menuId=id||"";panelX=x===undefined?area.x+area.width-360:x;panelY=y===undefined?area.y+32:y;panel=which}
    function updateSetting(key,value) {commit(Layout.setting(state,key,value))}
    function hideIcon(id) {var next=Layout.copy(state);if(next.items[id]){next.items[id].hidden=true;commit(next)}panel=""}
    function restoreIcon(id) {var next=Layout.copy(state);if(next.items[id]){next.items[id].hidden=false;commit(next)}}
    function openItem(id,origin) {
        var entry=files.find(function(f){return f.uri===id});if(!entry)return
        if(!origin){for(var i=0;i<tiles.count;i++)if(tiles.get(i).key===id){var v=tiles.get(i).payload,size=Math.min(prefs.icon,v.w-12);origin={x:view.x+v.x+((v.w-size)/2),y:view.y+v.y-view.contentY+5,size:size};break}}
        var data={uri:id,directory:entry.isDir,launcher:entry.isLauncher,generation:revision,origin:Object.assign({monitor:output,icon:entry.icon},origin||{})}
        panel=""
        if(entry.isLauncher&&!entry.trusted){trustUri=id;trustData=data;return}
        send("open",data)
    }
    function beginDrag(id,px,py) {
        if(panel||trustUri)return
        dragBase=Layout.copy(state);dragId=id;dragStartX=px;dragStartY=py;dragScroll=view.contentY;lastPointerY=py
        for(var i=0;i<tiles.count;i++)if(tiles.get(i).key===id){var p=tiles.get(i).payload;dragOffsetX=p.x;dragOffsetY=p.y;break}
        dragX=dragOffsetX;dragY=dragOffsetY;preview=dragBase
        root.forceActiveFocus()
    }
    function moveDrag(px,py) {
        if(!dragId)return
        lastPointerY=py;dragX=dragOffsetX+px-dragStartX;dragY=dragOffsetY+py-dragStartY+(view.contentY-dragScroll)
        preview=Layout.move(dragBase,dragId,output,dragX,dragY,area.width,area.height);refresh()
    }
    function endDrag(canceled) {
        if(!dragId)return
        var next=preview;dragId="";preview=null;dragBase=null
        if(!canceled)commit(next)
        refresh()
    }
    function addWidget(kind,size) {commit(Layout.addWidget(state,kind,size,output,area.width,area.height));panel="";editing=true}
    function resizeWidget(id,size) {commit(Layout.resizeWidget(state,id,size,area.width,area.height))}
    function removeWidget(id) {commit(Layout.removeWidget(state,id))}
    function navigate(dx,dy) {
        var items=Layout.project(state,files,output,outputs,area.width,area.height).items
        if(!items.length)return
        var current=items.find(function(i){return i.id===focusedId})||items[0],best=null,distance=Infinity
        items.forEach(function(i){var x=i.x-current.x,y=i.y-current.y;if((dx&&x*dx<=0)||(dy&&y*dy<=0))return
            var d=Math.hypot(x,y)+(dx?Math.abs(y):Math.abs(x))*2;if(d<distance){distance=d;best=i}})
        choose((best||current).id,0)
        var item=best||current;if(item.y<view.contentY)view.contentY=item.y;if(item.y+item.h>view.contentY+view.height)view.contentY=item.y+item.h-view.height
    }
    Keys.onPressed:function(event){
        if(event.key===Qt.Key_Escape){if(dragId)endDrag(true);else if(trustUri)trustUri="";else if(panel)panel="";else editing=false;event.accepted=true}
        else if(event.key===Qt.Key_Return||event.key===Qt.Key_Enter){openItem(focusedId);event.accepted=true}
        else if(event.key===Qt.Key_Left){navigate(-1,0);event.accepted=true}
        else if(event.key===Qt.Key_Right){navigate(1,0);event.accepted=true}
        else if(event.key===Qt.Key_Up){navigate(0,-1);event.accepted=true}
        else if(event.key===Qt.Key_Down){navigate(0,1);event.accepted=true}
        else if(event.key===Qt.Key_Delete){var next=Layout.copy(state);Object.keys(selected).forEach(function(id){if(next.items[id]){if(next.items[id].type==='widget')next=Layout.removeWidget(next,id);else next.items[id].hidden=true}});commit(next);event.accepted=true}
    }
    onStateChanged:{Qt.callLater(refresh);Qt.callLater(reconcile)}
    onOutputsChanged:Qt.callLater(refresh)
    onPrimaryChanged:Qt.callLater(refreshFiles)
    onAreaChanged:Qt.callLater(refresh)
    Component.onCompleted:Qt.callLater(refreshFiles)
    ListModel{id:tiles;dynamicRoles:true}
    Instantiator {
        id:fileIndex;model:Folder
        delegate:QtObject {
            required property var model
            readonly property string uri:model.uri
            readonly property string name:model.name
            readonly property string displayName:model.displayName
            readonly property string icon:model.icon
            readonly property bool isDir:model.isDir
            readonly property bool isLauncher:model.isLauncher
            readonly property bool trusted:model.trusted
            readonly property bool hidden:model.hidden
            onUriChanged:Qt.callLater(root.refreshFiles)
            onNameChanged:Qt.callLater(root.refreshFiles)
            onIconChanged:Qt.callLater(root.refreshFiles)
            onTrustedChanged:Qt.callLater(root.refreshFiles)
            onHiddenChanged:Qt.callLater(root.refreshFiles)
        }
        onObjectAdded:Qt.callLater(root.refreshFiles)
        onObjectRemoved:Qt.callLater(root.refreshFiles)
    }
    Instantiator {
        id:appIndex;model:Apps
        delegate:QtObject {
            required property var model
            readonly property string appId:model.id
            readonly property string appName:model.name
            readonly property string icon:model.icon
            readonly property bool present:model.present
            onAppNameChanged:Qt.callLater(root.refreshApps)
            onPresentChanged:Qt.callLater(root.refreshApps)
        }
        onObjectAdded:Qt.callLater(root.refreshApps)
        onObjectRemoved:Qt.callLater(root.refreshApps)
    }
    Flickable {
        // Edit mode lowers the canvas below its toolbar, so the toolbar never
        // covers a widget's handle or remove button.
        id:view;x:root.area.x;y:root.area.y+editInset;width:root.area.width;height:root.area.height-editInset
        readonly property real editInset:root.editing?64:0
        Behavior on y {enabled:!root.prefs.reduceMotion;NumberAnimation{duration:root.prefs.motionMs/2;easing.type:Easing.OutCubic}}
        Behavior on height {enabled:!root.prefs.reduceMotion;NumberAnimation{duration:root.prefs.motionMs/2;easing.type:Easing.OutCubic}}
        contentWidth:root.canvasWidth;contentHeight:root.canvasHeight;clip:true
        interactive:!root.dragId && !root.panel
        boundsBehavior:Flickable.StopAtBounds
        MouseArea {anchors.fill:parent;acceptedButtons:Qt.LeftButton|Qt.RightButton
            onPressed:function(mouse){root.forceActiveFocus();if(mouse.button===Qt.RightButton){var p=mapToItem(root,mouse.x,mouse.y);root.showPanel("menu",p.x,p.y)}else{root.selected={};root.panel=""}}
            onPressAndHold:{root.editing=true;root.panel=""}
            onWheel:function(wheel){view.contentY=Layout.clamp(view.contentY-wheel.angleDelta.y,0,Math.max(0,view.contentHeight-view.height))}
        }
        Rectangle {visible:root.dragId!==""&&root.prefs.grid;z:0;color:"#26936dff";border {width:2;color:"#aaad8aff"}
radius:20
            property var target:{if(!root.preview||!root.dragId)return null;return Layout.project(root.preview,root.files,root.output,root.outputs,root.area.width,root.area.height).items.find(function(i){return i.id===root.dragId})}
            x:target?target.x:0;y:target?target.y:0;width:target?target.w:0;height:target?target.h:0 }
        Repeater {
            model:tiles
            delegate:Item {
                id:carrier
                required property string key
                required property var payload
                readonly property bool dragging:root.dragId===key
                x:dragging?root.dragX:payload.x;y:dragging?root.dragY:payload.y
                width:payload.w;height:payload.h;z:dragging?100000:payload.z+1
                scale:dragging&&!root.prefs.reduceMotion?1.045:1
                Behavior on x {enabled:!carrier.dragging&&!root.prefs.reduceMotion&&root.prefs.motionMs>0;SpringAnimation{spring:3.5/Math.pow(Math.max(0.25,root.prefs.motionMs/420),2);damping:Math.min(1,0.34/Math.max(0.25,root.prefs.motionMs/420));epsilon:0.2}}
                Behavior on y {enabled:!carrier.dragging&&!root.prefs.reduceMotion&&root.prefs.motionMs>0;SpringAnimation{spring:3.5/Math.pow(Math.max(0.25,root.prefs.motionMs/420),2);damping:Math.min(1,0.34/Math.max(0.25,root.prefs.motionMs/420));epsilon:0.2}}
                Behavior on width {NumberAnimation{duration:root.prefs.reduceMotion?0:root.prefs.motionMs/2;easing.type:Easing.OutCubic}}
                Behavior on height {NumberAnimation{duration:root.prefs.reduceMotion?0:root.prefs.motionMs/2;easing.type:Easing.OutCubic}}
                Behavior on scale {NumberAnimation{duration:root.prefs.reduceMotion?0:140;easing.type:Easing.OutCubic}}
                Loader {
                    id:tileLoader;anchors.fill:parent
                    sourceComponent:carrier.payload.type==='file'?iconComponent:widgetComponent
                }
                Component {
                    id:iconComponent
                    IconItem {
                        entry:carrier.payload.file;settings:root.prefs;selected:!!root.selected[carrier.key];editing:root.editing
                        artHidden:!!root.flights[carrier.key]
                        onPicked:function(modifiers){root.choose(carrier.key,modifiers)}
                        onOpened:function(origin){root.openItem(carrier.key,origin)}
                        onMenuRequested:function(px,py){root.choose(carrier.key,0);root.showPanel('icon',px,py,carrier.key)}
                        onEditRequested:root.editing=true
                        onDragStarted:function(px,py){root.beginDrag(carrier.key,px,py)}
                        onDragMoved:function(px,py){root.moveDrag(px,py)}
                        onDragEnded:function(canceled){root.endDrag(canceled)}
                    }
                }
                Component {id:widgetComponent;WidgetItem {desktop:root;entry:carrier.payload;dragging:carrier.dragging}}
            }
        }
    }
    Timer {interval:40;running:root.dragId!=="";repeat:true
        onTriggered:{var delta=root.lastPointerY<view.y+40?-16:root.lastPointerY>view.y+view.height-40?16:0
            if(delta){view.contentY=Layout.clamp(view.contentY+delta,0,Math.max(0,view.contentHeight-view.height));root.moveDrag(root.dragStartX+root.dragX-root.dragOffsetX,root.lastPointerY)}} }
    Timer {interval:1000;running:root.output===root.primary;repeat:true
        onTriggered:{var now=Date.now();Object.keys(root.flights).forEach(function(uri){var f=root.flights[uri];if(f.expires<=now)root.send('flightDone',{uri:uri,serial:f.serial})});
            var needed=Object.keys(root.state.items).some(function(id){var i=root.state.items[id];return i.kind==='stats'&&!i.hidden});if(needed)root.send('poll')}
    }
    Rectangle {visible:root.editing;x:root.area.x+root.area.width-width-24;y:root.area.y+16;width:330;height:48;radius:18;color:root.surface;border{width:1;color:"#406f5d88"}
        Row {anchors.centerIn:parent;spacing:4
            Action {width:108;height:36;text:"Add widget";dark:root.prefs.dark;onTriggered:root.showPanel('widgets')}
            Action {width:120;height:36;text:root.prefs.grid?"Grid: On":"Grid: Off";dark:root.prefs.dark;onTriggered:root.updateSetting('grid',!root.prefs.grid)}
            Action {width:76;height:36;text:"Done";checked:true;dark:root.prefs.dark;onTriggered:{root.editing=false;root.panel=''}} }
    }
    Rectangle {visible:!!root.saveError;x:root.area.x+32;y:root.area.y+root.area.height-height-18;width:Math.min(660,root.area.width-64);height:54;radius:14;color:root.surface
        Text {anchors {left:parent.left;right:retry.left;verticalCenter:parent.verticalCenter;margins:14}
text:root.saveError;textFormat:Text.PlainText;color:root.foreground;font.pixelSize:12;elide:Text.ElideRight}
        Action{id:retry;anchors {right:parent.right;verticalCenter:parent.verticalCenter;rightMargin:8}
width:72;text:"Retry";dark:root.prefs.dark;onTriggered:root.send('retry')}
    }
    Rectangle {visible:!!root.state.undo&&!root.editing;x:root.area.x+32;y:root.area.y+root.area.height-62;width:220;height:44;radius:14;color:root.surface
        Action{anchors.fill:parent;text:"Undo widget removal";dark:root.prefs.dark;onTriggered:root.commit(Layout.undo(root.state))}}
    Controls {desktop:root}
    Rectangle {visible:!!root.trustUri;anchors.fill:parent;color:"#80000000";z:200001
        MouseArea{anchors.fill:parent;onClicked:root.trustUri=''}
        Rectangle{anchors.centerIn:parent;width:420;height:190;radius:22;color:root.surface
            MouseArea{anchors.fill:parent}
            Column{anchors {fill:parent;margins:20}
spacing:12
                Text{text:"Allow this desktop launcher?";font {pixelSize:18;weight:Font.DemiBold}
color:root.foreground}
                Text{width:parent.width;text:"Trust this shortcut to run its application.";wrapMode:Text.WordWrap;font.pixelSize:13;color:root.secondary}
                Row{spacing:8;Action{width:180;text:"Cancel";dark:root.prefs.dark;onTriggered:root.trustUri=''}
                    Action{width:180;text:"Trust and Open";checked:true;dark:root.prefs.dark;onTriggered:{root.send('trust',root.trustData);root.trustUri=''}}}
            }
        }
    }
}
