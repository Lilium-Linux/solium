-- Shape a real desktop pane during its flight from a launcher square.
return {
    api=1,grid={4,16},
    params={icon_x={1,min=0.0001,max=1},icon_y={1,min=0.0001,max=1},corner={16,min=0,max=512},
        rest={12,min=0,max=512},peak={24,min=0,max=256}},
    frag='flight.frag',
    mesh=function(t,cols,rows,out)
        local f=t.from;local p=math.max(0,math.min(1,t.progress));local target=t.to
        local aspect=f.w/math.max(1,f.h);local iconAspect=target.w/math.max(1,target.h)
        local blend=1-(1-p)^3;local ratio=aspect+(iconAspect-aspect)*blend
        local w,h=f.w,f.h
        if ratio<aspect then w=h*ratio else h=w/math.max(0.0001,ratio)end
        local left,top=f.x+(f.w-w)/2,f.y+(f.h-h)/2
        local n=0
        for r=0,rows do for c=0,cols do
            local u,v=sol_grid(t,c,r)
            out[n+1]=left+u*w;out[n+2]=top+v*h;n=n+2
        end end
    end,
}
