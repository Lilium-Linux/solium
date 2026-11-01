-- Bounded data-only JSON for the configuration's state file.
local json={}
local escapes={['"']='\\"',['\\']='\\\\',['\b']='\\b',['\f']='\\f',['\n']='\\n',['\r']='\\r',['\t']='\\t'}
local function quote(s) return '"'..s:gsub('[%z\1-\31\\"]',function(c)return escapes[c] or string.format('\\u%04x',c:byte())end)..'"' end
function json.encode(value)
    local seen={}
    local function encode(v,depth)
        assert(depth<64,'JSON nesting too deep')
        local t=type(v)
        if t=='nil' then return 'null' end
        if t=='boolean' then return tostring(v) end
        if t=='number' then assert(v==v and v~=math.huge and v~=-math.huge,'non-finite number');return tostring(v) end
        if t=='string' then return quote(v) end
        assert(t=='table' and not seen[v],'invalid JSON value');seen[v]=true
        local out={};local array=#v>0
        if array then for k in pairs(v)do if type(k)~='number' or k<1 or k>#v or k%1~=0 then array=false;break end end end
        if array then for _,item in ipairs(v)do out[#out+1]=encode(item,depth+1)end
        else for k,item in pairs(v)do assert(type(k)=='string','object key must be a string');out[#out+1]=quote(k)..':'..encode(item,depth+1)end;table.sort(out)end
        seen[v]=nil;return (array and '[' or '{')..table.concat(out,',')..(array and ']' or '}')
    end
    return encode(value,0)
end
function json.decode(text)
    if type(text)~='string' or #text>4*1024*1024 then return nil,'state too large' end
    local pos=1
    local function skip()local _,e=text:find('^[ \t\r\n]*',pos);pos=(e or pos-1)+1 end
    local function str()
        assert(text:sub(pos,pos)=='"');pos=pos+1;local out={}
        while pos<=#text do
            local c=text:sub(pos,pos);pos=pos+1
            if c=='"' then return table.concat(out) end
            if c=='\\' then
                c=text:sub(pos,pos);pos=pos+1
                local map={['"']='"',['\\']='\\',['/']='/',b='\b',f='\f',n='\n',r='\r',t='\t'}
                if c=='u' then
                    local h=text:sub(pos,pos+3);assert(h:match('^%x%x%x%x$'),'bad unicode escape');pos=pos+4
                    local n=tonumber(h,16)
                    if n>=0xd800 and n<=0xdbff then
                        assert(text:sub(pos,pos+1)=='\\u','missing unicode pair');pos=pos+2
                        local lo=text:sub(pos,pos+3);assert(lo:match('^%x%x%x%x$'));pos=pos+4;lo=tonumber(lo,16)
                        assert(lo>=0xdc00 and lo<=0xdfff);n=0x10000+(n-0xd800)*0x400+lo-0xdc00
                    else assert(n<0xdc00 or n>0xdfff,'invalid unicode pair') end
                    out[#out+1]=utf8.char(n)
                else assert(map[c],'bad escape');out[#out+1]=map[c] end
            else assert(c:byte()>=32,'control character');out[#out+1]=c end
        end
        error('unterminated string')
    end
    local parse
    parse=function(depth)
        assert(depth<64,'JSON nesting too deep');skip();local c=text:sub(pos,pos)
        if c=='"'then return str()end
        if c=='{'or c=='['then
            pos=pos+1;local result={};skip();local close=c=='{'and'}'or']'
            if text:sub(pos,pos)==close then pos=pos+1;return result end
            repeat
                skip();local k;if c=='{'then k=str();skip();assert(text:sub(pos,pos)==':','missing colon');pos=pos+1 end
                local v=parse(depth+1);if c=='{'then assert(result[k]==nil,'duplicate key');result[k]=v else result[#result+1]=v end
                skip();local separator=text:sub(pos,pos);pos=pos+1
                if separator==close then return result end
                assert(separator==',','missing separator')
            until false
        end
        for literal,v in pairs({['true']=true,['false']=false})do if text:sub(pos,pos+#literal-1)==literal then pos=pos+#literal;return v end end
        if text:sub(pos,pos+3)=='null'then pos=pos+4;return nil end
        local token=text:sub(pos):match('^-?%d+%.?%d*[eE]?[+-]?%d*')
        assert(token and not token:match('^-?0%d'),'invalid number')
        assert(not token:match('%.$') and not token:match('[eE][+-]?$'),'invalid number')
        local n=tonumber(token);assert(n and n==n and n~=math.huge and n~=-math.huge,'invalid number');pos=pos+#token;return n
    end
    local ok,result=pcall(function()local v=parse(0);skip();assert(pos>#text,'trailing JSON');return v end)
    if ok then return result end
    return nil,tostring(result)
end
return json
