-- Desktop icon launches own their origin; all other windows keep the normal open.
local config=require('config')
local launch=require('desktop.launch')
-- load is a public effect API option, but is not a user.lua schema key.
local effects={}
for key,value in pairs(config.effects or{})do effects[key]=value end
effects.load={"desktop-flight"}
sol.effects(effects)
sol.on('open',function(id)
    if launch.capture(id)then return end
    for _,w in ipairs(sol.windows())do if w.id==id then
        local scale=config.open.scale
        sol.animate(config.open.motion)
        sol.present_from(id,{x=w.x+w.w*(1-scale)/2,y=w.y+w.h*(1-scale)/2,w=w.w*scale,h=w.h*scale,opacity=0})
        return
    end end
end)
