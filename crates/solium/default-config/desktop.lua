local store=require('desktop.store')
local apps=require('desktop.applications')
local desktop={}
local configRoot=(os.getenv('XDG_CONFIG_HOME')or(os.getenv('HOME')..'/.config'))..'/solium'
local stateDir=(os.getenv('XDG_STATE_HOME')or(os.getenv('HOME')..'/.local/state'))..'/solium/desktop'
os.execute('mkdir -p '..apps.quote(stateDir))
local path=stateDir..'/layout.json'
local loaded,loadError=store.load(path)
local defaults={version=1,settings={grid=true,cell=112,margin=32,icon=64,labels=true,open='double',fontScale=1,dark=true,
    transparency=true,reduceMotion=false,motionMs=420},items={},next=2}
if loaded then for k,v in pairs(defaults.settings)do if loaded.settings[k]==nil then loaded.settings[k]=v end end;loaded.next=loaded.next or 2 end
local kept=sol.keep('desktop.homescreen.v1',{state=loaded or defaults,revision=0,error=loadError or'',shown={},stats={}})
local outputs,primary={},''
local flights={}
local desktopDir=os.getenv('XDG_DESKTOP_DIR')
if not desktopDir then
    local f=io.open((os.getenv('XDG_CONFIG_HOME')or(os.getenv('HOME')..'/.config'))..'/user-dirs.dirs')
    if f then for line in f:lines()do local value=line:match('^XDG_DESKTOP_DIR="(.-)"');if value then desktopDir=value:gsub('%$HOME',os.getenv('HOME'))end end;f:close()end
end
desktopDir=desktopDir or(os.getenv('HOME')..'/Desktop')
function desktop.apply()
    local current={};outputs={};primary=''
    local monitors=sol.monitors()
    for _,m in ipairs(monitors)do if m.primary then primary=m.name end end
    primary=primary~=''and primary or(monitors[1]and monitors[1].name or'')
    if primary~=''then outputs[1]=primary end
    for _,m in ipairs(monitors)do if m.name~=primary then outputs[#outputs+1]=m.name end end
    for _,m in ipairs(monitors)do
        local name='homescreen-'..m.name;current[name]=true
        sol.surface(name,{scene=configRoot..'/desktop/Desktop.qml',layer='bottom',on=m.name,interactive=true,
            keyboard={bindings='except_claimed'},properties={state=kept.state,revision=kept.revision,outputs=outputs,primary=primary,
            saveError=kept.error,stats=kept.stats,flights=flights,desktopPath=desktopDir}})
    end
    for name in pairs(kept.shown)do if not current[name]then sol.surface(name,false)end end
    kept.shown=current
end
function desktop.commit(base,state)
    if base~=kept.revision then desktop.apply();return false,'Desktop changed in another view'end
    local valid,err=store.validate(state)
    if not valid then kept.error=err;desktop.apply();return false,err end
    kept.state=state;kept.revision=kept.revision+1
    local ok,why=store.save(path,state);kept.error=ok and''or('Could not save desktop: '..tostring(why))
    desktop.apply();return true
end
function desktop.flight(uri,record)flights[uri]=record;desktop.apply()end
function desktop.settings()return kept.state.settings end
function desktop.generation()return kept.revision end
local previous={}
local function sample()
    local f=io.open('/proc/stat');local total,idle=0,0
    if f then local line=f:read('*l')or'';local values={};for n in line:gmatch('%d+')do values[#values+1]=tonumber(n);total=total+tonumber(n)end;idle=(values[4]or 0)+(values[5]or 0);f:close()end
    local stats={cpu=-1,memory=-1,down=-1,up=-1}
    if previous.total and total>previous.total then stats.cpu=math.floor(100*(1-(idle-previous.idle)/(total-previous.total))+0.5)end
    previous.total=total;previous.idle=idle
    f=io.open('/proc/meminfo');if f then local text=f:read('*a');f:close();local all=tonumber(text:match('MemTotal:%s+(%d+)'));local available=tonumber(text:match('MemAvailable:%s+(%d+)'));if all and available then stats.memory=100*(all-available)/all;stats.used=(all-available)/1048576;stats.total=all/1048576 end end
    f=io.open('/proc/net/dev');local rx,tx=0,0
    if f then for line in f:lines()do local name,fields=line:match('^%s*([^:]+):%s*(.+)');if name and name~='lo'then local v={};for n in fields:gmatch('%d+')do v[#v+1]=tonumber(n)end;rx=rx+(v[1]or 0);tx=tx+(v[9]or 0)end end;f:close()end
    local now=os.time();if previous.time and now>previous.time then stats.down=math.max(0,(rx-previous.rx)/(now-previous.time));stats.up=math.max(0,(tx-previous.tx)/(now-previous.time))end
    previous.time=now;previous.rx=rx;previous.tx=tx;kept.stats=stats;desktop.apply()
end
local function open_file(data)
    local uri=data.uri
    if type(uri)~='string'or uri:sub(1,#('file://'..desktopDir..'/'))~='file://'..desktopDir..'/' then return end
    if data.directory then sol.spawn('xdg-open',uri);return end
    if data.launcher then require('desktop.launch').open(uri,data.origin or{},data.generation)else sol.act('folder.open',{uri=uri})end
end
sol.on('surface',function(_,action,data)
    data=type(data)=='table'and data or{}
    if action=='desktop.commit'then desktop.commit(data.base,data.state)
    elseif action=='desktop.open'then open_file(data)
    elseif action=='desktop.trust'then sol.act('folder.trust',{uri=data.uri},function(ok)if ok then open_file(data)end end)
    elseif action=='desktop.openFolder'then sol.spawn('xdg-open',desktopDir)
    elseif action=='desktop.addApplication'then
        local destination,why=apps.add(data.id,desktopDir)
        if not destination then kept.error=why else local item=kept.state.items['file://'..destination];if item then item.hidden=false;kept.revision=kept.revision+1;store.save(path,kept.state)end;kept.error=''end
        desktop.apply()
    elseif action=='desktop.retry'then local ok,why=store.save(path,kept.state);kept.error=ok and''or tostring(why);desktop.apply()
    elseif action=='desktop.poll'then sample()
    elseif action=='desktop.flightDone'then local launch=package.loaded['desktop.launch'];if launch then launch.finish(data.uri,data.serial)end end
end)
sol.on('monitors',desktop.apply);sol.on('restore',desktop.apply)
local lastGeometry=''
sol.on('layout',function()
    local parts={};for _,m in ipairs(sol.monitors())do local w=m.whole or m;parts[#parts+1]=table.concat({m.name,w.x,w.y,w.w,w.h,m.w,m.h},',')end
    local signature=table.concat(parts,';');if signature~=lastGeometry then lastGeometry=signature;desktop.apply()end
end)
return desktop
