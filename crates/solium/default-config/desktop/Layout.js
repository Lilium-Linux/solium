.pragma library

// The canvas projects persistent home positions; previewing a move never edits them.
var families = {clock:['2x2','4x2'],calendar:['2x2','4x2'],media:['4x2','4x4'],stats:['2x2','4x2']}
function copy(value) { return JSON.parse(JSON.stringify(value)) }
function defaults() {
    return {version:1,settings:{grid:true,cell:112,margin:32,icon:64,labels:true,open:'double',
        fontScale:1,dark:true,transparency:true,reduceMotion:false,motionMs:420},items:{},next:2}
}
function reconcile(state,files,primary,width,height) {
    var s=copy(state), valid={}
    if(!s.initialized) {
        s.items['widget:1']={type:'widget',kind:'clock',size:'2x2',monitor:primary,grid:{col:0,row:0},free:null,z:0,options:{}}
        s.initialized=true
    }
    files.forEach(function(f){valid[f.uri]=true;if(!s.items[f.uri])
        s.items[f.uri]={type:'file',monitor:primary,grid:{col:-1,row:-1},free:null,z:0}})
    // Missing files stay dormant: a temporary empty model cannot erase their
    // placement, and a file brought back to Desktop returns to its old spot.
    if(width>0&&height>0)materialize(s,place(s,files,primary,null,width,height,null),primary)
    return s
}
// Pin this output's unplaced items to the cells they are shown in, so they
// do not drift as other items come and go.
function pin(state,files,output,width,height) {
    var s=copy(state);if(width>0&&height>0)materialize(s,place(s,files,output,null,width,height,null),output);return s
}
function span(item) { var s=(item.type==='widget'?item.size:'1x1').split('x');return {cols:+s[0],rows:+s[1]} }
function clamp(x,a,b) {return Math.max(a,Math.min(b,x))}
function place(state,files,output,outputs,width,height,first) {
    var s=state.settings,cell=s.cell,margin=s.margin,columns=Math.max(4,Math.floor((width-margin*2)/cell))
    var occupied={},list=[], lookup={}
    files.forEach(function(f){lookup[f.uri]=f})
    // Without outputs, only this output's own items: an edit there must not
    // make room for, or be pushed by, another output's items. With them, a
    // disconnected output's items borrow the primary after its own items, so
    // the guests are the ones that give way.
    function shown(id){var i=state.items[id];return !i.hidden&&(i.type!=='file'||lookup[id])}
    var ids=Object.keys(state.items).filter(function(id){return shown(id)&&state.items[id].monitor===output})
    if(outputs&&output===outputs[0])ids=ids.concat(Object.keys(state.items).filter(function(id){
        return shown(id)&&outputs.indexOf(state.items[id].monitor)<0}))
    if(first && ids.indexOf(first)>=0){ids.splice(ids.indexOf(first),1);ids.unshift(first)}
    function fits(c,r,w,h){if(c<0||r<0||c+w>columns)return false
        for(var y=r;y<r+h;y++)for(var x=c;x<c+w;x++)if(occupied[x+','+y])return false
        return true}
    var bottom=height
    ids.forEach(function(id){
        var i=state.items[id],sp=span(i),g=i.grid||{col:-1,row:-1},c=g.col,r=g.row
        if(!fits(c,r,sp.cols,sp.rows)) {c=0;r=0;while(!fits(c,r,sp.cols,sp.rows)){c++;if(c+sp.cols>columns){c=0;r++}}}
        for(var yy=r;yy<r+sp.rows;yy++)for(var xx=c;xx<c+sp.cols;xx++)occupied[xx+','+yy]=true
        var x=margin+c*cell,y=margin+r*cell,w=sp.cols*cell-16,h=sp.rows*cell-16
        if(!s.grid && i.free) {x=clamp(i.free.x,0,Math.max(0,width-48));y=clamp(i.free.y,0,Math.max(0,height-48))}
        bottom=Math.max(bottom,y+h+margin)
        list.push({id:id,type:i.type,kind:i.kind||'',size:i.size||'1x1',options:i.options||{},
            col:c,row:r,cols:sp.cols,rows:sp.rows,x:x,y:y,w:w,h:h,z:i.z||0,
            monitor:i.monitor,fallback:i.monitor!==output,file:lookup[id]||{}})
    })
    return {items:list,height:bottom,width:Math.max(width,columns*cell+margin*2)}
}
function project(state,files,output,outputs,width,height) {return place(state,files,output,outputs,width,height,null)}
function fileRows(state){return Object.keys(state.items).filter(function(id){return state.items[id].type==='file'}).map(function(id){return {uri:id}})}
function materialize(state,p,output) {
    p.items.forEach(function(v){if(state.items[v.id].monitor!==output)return
        state.items[v.id].grid={col:v.col,row:v.row}
        if(!state.items[v.id].free)state.items[v.id].free={x:v.x,y:v.y}})
}
function move(state,id,output,x,y,width,height) {
    if(!state.items[id]||!isFinite(x)||!isFinite(y))return null
    var s=copy(state),i=s.items[id],settings=s.settings
    var before=place(s,fileRows(s),output,null,width,height,null);materialize(s,before,output)
    i.monitor=output;i.z=s.next++
    if(settings.grid){
        var cols=Math.max(4,Math.floor((width-settings.margin*2)/settings.cell)),sp=span(i)
        i.grid={col:clamp(Math.round((x-settings.margin)/settings.cell),0,cols-sp.cols),row:Math.max(0,Math.round((y-settings.margin)/settings.cell))}
        materialize(s,place(s,fileRows(s),output,null,width,height,id),output)
    }else i.free={x:clamp(x,0,Math.max(0,width-48)),y:clamp(y,0,Math.max(0,height-48))}
    return s
}
function arrange(state,output,width,height) {
    var s=copy(state)
    Object.keys(s.items).forEach(function(id){if(s.items[id].monitor===output){s.items[id].grid={col:-1,row:-1};s.items[id].free=null}})
    materialize(s,place(s,fileRows(s),output,null,width,height,null),output);return s
}
function addWidget(state,kind,size,output,width,height) {
    if(!families[kind]||families[kind].indexOf(size)<0)return null
    var s=copy(state),id='widget:'+s.next++
    s.items[id]={type:'widget',kind:kind,size:size,monitor:output,grid:{col:-1,row:-1},free:null,z:s.next,options:{}}
    materialize(s,place(s,fileRows(s),output,null,width,height,null),output);return s
}
function resizeWidget(state,id,size,width,height) {
    var i=state.items[id];if(!i||!families[i.kind]||families[i.kind].indexOf(size)<0)return null
    var s=copy(state);s.items[id].size=size
    materialize(s,place(s,fileRows(s),i.monitor,null,width,height,id),i.monitor);return s
}
function removeWidget(state,id) {if(!state.items[id]||state.items[id].type!=='widget')return null
    var s=copy(state);s.undo={id:id,item:s.items[id]};delete s.items[id];return s}
function undo(state) {if(!state.undo)return null;var s=copy(state);s.items[s.undo.id]=s.undo.item;delete s.undo;return s}
function setting(state,key,value) {
    var ranges={cell:[80,180],margin:[0,96],icon:[32,96],fontScale:[0.75,2],motionMs:[0,2000]}
    var bools=['grid','labels','dark','transparency','reduceMotion']
    if(ranges[key]){if(typeof value!=='number'||!isFinite(value)||value<ranges[key][0]||value>ranges[key][1])return null}
    else if(bools.indexOf(key)>=0){if(typeof value!=='boolean')return null}
    else if(key==='open'){if(value!=='single'&&value!=='double')return null}
    else return null
    var s=copy(state);s.settings[key]=value;return s
}
