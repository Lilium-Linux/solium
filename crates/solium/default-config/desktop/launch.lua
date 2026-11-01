-- Keep a launch's origin with its loading pane, not in a global last-icon slot.
local desktop=require('desktop')
local launch={}
local queue,byPane={},{}
local issuing,departed
local serial=0
local function window(id)for _,w in ipairs(sol.windows())do if w.id==id then return w end end end
local function finite(v)return type(v)=='number'and v==v and math.abs(v)<1e8 end
local function monitor(name)for _,m in ipairs(sol.monitors())do if m.name==name then return m end end end
local function normalize(s)return (s or''):lower():gsub('%.desktop$',''):gsub('[^%w]','')end
local function valid(origin,generation)
    if type(origin)~='table'or generation~=desktop.generation()then return nil end
    local m=monitor(origin.monitor);if not m then return nil end
    local whole=m.whole or m
    if not finite(origin.x)or not finite(origin.y)or not finite(origin.size)or origin.size<16 or origin.size>160 or origin.x<0 or origin.y<0 or origin.x+origin.size>whole.w+1 or origin.y+origin.size>whole.h+1 then return nil end
    return {x=whole.x+origin.x,y=whole.y+origin.y,w=origin.size,h=origin.size,monitor=m.name,scale=m.scale or 1,
        whole={x=whole.x,y=whole.y,w=whole.w,h=whole.h},icon=type(origin.icon)=='string'and origin.icon or''}
end
local function still_valid(record)
    local m=record.origin and monitor(record.origin.monitor);if not m then return false end
    local a,b=m.whole or m,record.origin.whole
    return a.x==b.x and a.y==b.y and a.w==b.w and a.h==b.h
end
local function aliases(uri)
    local found={};local filename=uri:match('([^/]+)$')or'';found[normalize(filename)]=true
    local f=io.open(uri:sub(8),'r')
    if f then
        local section=false
        for line in f:lines()do
            if line:sub(1,1)=='['then section=line=='[Desktop Entry]'end
            if section then local key,value=line:match('^([%w]+)=(.*)');if key=='Name'or key=='StartupWMClass'then found[normalize(value)]=true end end
        end
        f:close()
    end
    return found
end
local function animate(record,id)
    local w=window(id);if not w or not record.origin or not still_valid(record)then return false end
    local prefs=desktop.settings();if prefs.reduceMotion or prefs.motionMs==0 then return false end
    local o=record.origin;local pace=math.max(0.2,(prefs.motionMs or 420)/420)
    local function spring(k,d)return{easing={spring={stiffness=k/(pace*pace),damping=d/pace}}}end
    record.id=id;record.expires=os.time()*1000+math.ceil(1400*pace);byPane[id]=record
    local decorated=w.style~='none'
    sol.frame(id,{desktopFlight=decorated and{serial=record.serial,icon=o.icon,pace=pace}or false})
    sol.animate(spring(324,36))
    local from={x=o.x,y=o.y,w=o.w,h=o.h,opacity=1,
        motion={x=spring(625,50),y=spring(625,50),size=spring(324,36)}}
    if decorated then
        from.client_opacity=0;from.motion.client_opacity={duration=math.floor(100*pace),easing='linear'}
        from.deform={effect='desktop-flight',to={x=o.x,y=o.y,w=o.w,h=o.h},icon_x=math.min(1,o.w/math.max(1,w.w)),icon_y=math.min(1,o.h/math.max(1,w.h)),corner=o.w/4*o.scale,rest=12*o.scale,peak=24*o.scale}
    else from.opacity=0;from.motion.opacity={duration=math.floor(140*pace),easing='outCubic'}end
    sol.present_from(id,from)
    if decorated then desktop.flight(record.uri,{serial=record.serial,expires=record.expires})end
    return true
end
function launch.finish(uri,token)
    for id,record in pairs(byPane)do
        if record.uri==uri and record.serial==token then sol.frame(id,{desktopFlight=false});byPane[id]=nil end
    end
    -- A late completion from an earlier flight must not reveal a newer source.
    local remaining
    for _,record in pairs(byPane)do if record.uri==uri and record.expires and(not remaining or record.expires>remaining.expires)then remaining=record end end
    desktop.flight(uri,remaining and{serial=remaining.serial,expires=remaining.expires}or nil)
end
function launch.capture(id)
    departed=nil
    if not issuing or issuing.id or not issuing.origin then return false end
    issuing.id=id;byPane[id]=issuing
    -- The loading pane's first frame already has its artwork.
    sol.frame(id,{desktopFlight={serial=issuing.serial,icon=issuing.origin.icon,pace=1}})
    return true
end
local issue
issue=function()
    if issuing or #queue==0 then return end
    issuing=table.remove(queue,1);local record=issuing
    sol.act('folder.open',{uri=record.uri},function(ok)
        if ok and record.id then animate(record,record.id)
        elseif record.id then launch.finish(record.uri,record.serial)end
        issuing=nil;issue()
    end)
end
function launch.open(uri,origin,generation)
    serial=serial+1
    queue[#queue+1]={uri=uri,serial=serial,origin=valid(origin,generation),aliases=aliases(uri)}
    issue()
end
sol.on('close',function(id)
    local record=byPane[id];departed=record
    if record then launch.finish(record.uri,record.serial)end
end)
sol.on('activate',function(id,why)
    local record=departed;departed=nil
    if why~='launch'or not record then return end
    local w=window(id)
    if w and record.aliases[normalize(w.app_id)]then animate(record,id)end
end)
sol.on('minimize',function(id)local record=byPane[id];if record then launch.finish(record.uri,record.serial)end end)
sol.on('monitors',function()
    local remove={};for _,record in pairs(byPane)do if not still_valid(record)then remove[#remove+1]=record end end
    for _,record in ipairs(remove)do launch.finish(record.uri,record.serial)end
end)
return launch
