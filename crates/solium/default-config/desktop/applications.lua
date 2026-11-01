local apps={}
function apps.quote(s)return "'"..s:gsub("'","'\\''").."'"end
local function contents(path)local f=io.open(path,'rb');if not f then return nil end;local s=f:read(1024*1024);f:close();return s end
function apps.roots()
    local home=os.getenv('HOME')or'';local roots={(os.getenv('XDG_DATA_HOME')or home..'/.local/share')..'/applications'}
    for path in (os.getenv('XDG_DATA_DIRS')or'/usr/local/share:/usr/share'):gmatch('[^:]+')do roots[#roots+1]=path..'/applications'end
    return roots
end
function apps.resolve(id,roots)
    if type(id)~='string'or #id>512 or not id:match('^[%w_.%-]+$')or id:find('..',1,true)then return nil,'Invalid application identity'end
    id=id:gsub('%.desktop$','');local filename=id..'.desktop'
    for _,root in ipairs(roots or apps.roots())do
        local path=root..'/'..filename;local text=contents(path)
        if text and text:find('%[Desktop Entry%]')and text:find('Type=Application',1,true)then return path,text,filename end
        -- Desktop IDs flatten nested application directories with '-'.
        if id:find('-',1,true)then
            local pipe=io.popen('find -L '..apps.quote(root).." -type f -name '*.desktop' -print0 2>/dev/null",'r')
            if pipe then local paths=pipe:read('*a');pipe:close()
                for candidate in paths:gmatch('[^%z]+')do
                    if candidate:sub(#root+2):gsub('/','-')==filename then
                        text=contents(candidate);if text then return candidate,text,filename end
                    end
                end
            end
        end
    end
    return nil,'Installed application launcher was not found'
end
function apps.add(id,directory,roots)
    local source,text,filename=apps.resolve(id,roots);if not source then return nil,text end
    for n=0,999 do
        local destination=directory..'/'..(n==0 and filename or filename:gsub('%.desktop$','-'..n..'.desktop'))
        local previous=contents(destination)
        if previous==text then return destination end
        if not previous then
            -- Lua io.open has no exclusive-create mode. cp's no-clobber option
            -- also protects a file created between the scan and the copy.
            local copied=os.execute('cp --no-clobber -- '..apps.quote(source)..' '..apps.quote(destination))
            if not copied then return nil,'Could not copy application shortcut'end
            if contents(destination)==text then
                os.execute('chmod u+x '..apps.quote(destination))
                return destination
            end
        end
    end
    return nil,'Too many shortcuts with the same name'
end
return apps
