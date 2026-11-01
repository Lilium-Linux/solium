local json=require('desktop.json')
local store={}
local function finite(v)return type(v)=='number'and v==v and math.abs(v)<1e9 end
function store.validate(s)
    if type(s)~='table'or s.version~=1 or type(s.settings)~='table'or type(s.items)~='table'then return false,'Unsupported desktop state' end
    local ranges={cell={80,180},margin={0,96},icon={32,96},fontScale={0.75,2},motionMs={0,2000}}
    for k,r in pairs(ranges)do local v=s.settings[k];if v~=nil and(not finite(v)or v<r[1]or v>r[2])then return false,'Invalid setting: '..k end end
    for _,k in ipairs({'grid','labels','dark','transparency','reduceMotion'})do if s.settings[k]~=nil and type(s.settings[k])~='boolean'then return false,'Invalid setting: '..k end end
    if s.settings.open~=nil and s.settings.open~='single'and s.settings.open~='double'then return false,'Invalid click behavior'end
    local count=0
    for id,item in pairs(s.items)do
        count=count+1
        if count>10000 or type(id)~='string'or #id>8192 or type(item)~='table'then return false,'Invalid desktop item'end
        if item.type~='file'and item.type~='widget'then return false,'Invalid item type'end
        if type(item.monitor)~='string'or #item.monitor>256 then return false,'Invalid monitor'end
        if item.hidden~=nil and type(item.hidden)~='boolean'then return false,'Invalid hidden state'end
        if item.z~=nil and not finite(item.z)then return false,'Invalid stacking order'end
        if item.grid and(type(item.grid)~='table'or not finite(item.grid.col)or not finite(item.grid.row)or item.grid.col%1~=0 or item.grid.row%1~=0 or item.grid.col< -1 or item.grid.row< -1 or item.grid.row>100000)then return false,'Invalid grid position'end
        if item.free and(type(item.free)~='table'or not finite(item.free.x)or not finite(item.free.y))then return false,'Invalid free position'end
        if item.type=='file'and id:sub(1,7)~='file://'then return false,'Invalid file identity'end
        if item.type=='widget'then
            local sizes={clock={['2x2']=true,['4x2']=true},calendar={['2x2']=true,['4x2']=true},media={['4x2']=true,['4x4']=true},stats={['2x2']=true,['4x2']=true}}
            if not sizes[item.kind]or not sizes[item.kind][item.size]or type(item.options)~='table'then return false,'Invalid widget'end
        end
    end
    if s.next~=nil and(not finite(s.next)or s.next<1 or s.next%1~=0)then return false,'Invalid instance counter'end
    return true
end
local function read(path)
    local f=io.open(path,'r');if not f then return nil end
    local text=f:read(4*1024*1024+1);f:close();local s,err=json.decode(text)
    if not s then return nil,err end
    local valid,why=store.validate(s);if not valid then return nil,why end
    return s
end
function store.load(path)
    local state,err=read(path);if state then return state end
    if err then
        -- Preserve the malformed source once; subsequent retries never replace it.
        local exists=io.open(path..'.invalid','r')
        if exists then exists:close()else os.rename(path,path..'.invalid')end
    end
    local backup=read(path..'.bak')
    return backup,err or (backup and 'Recovered desktop backup' or nil)
end
function store.save(path,state)
    local valid,why=store.validate(state);if not valid then return false,why end
    local ok,text=pcall(json.encode,state);if not ok then return false,text end
    local tmp=path..'.tmp';local f,err=io.open(tmp,'w');if not f then return false,err end
    local wrote,reason=f:write(text);local closed,closeerr=f:close()
    if not wrote or not closed then os.remove(tmp);return false,reason or closeerr end
    if read(path)then
        local old=assert(io.open(path,'r'));local previous=old:read('*a');old:close()
        local b=io.open(path..'.bak.tmp','w');if not b then os.remove(tmp);return false,'Cannot save desktop backup'end
        local saved=b:write(previous);local done=b:close()
        if not saved or not done then os.remove(tmp);os.remove(path..'.bak.tmp');return false,'Cannot write desktop backup'end
        local renamed,renameerr=os.rename(path..'.bak.tmp',path..'.bak');if not renamed then os.remove(tmp);return false,renameerr end
    end
    local replaced,replaceerr=os.rename(tmp,path);if not replaced then os.remove(tmp);return false,replaceerr end
    return true
end
return store
