struct Globals {
    viewport: vec4<f32>, // logical size, scale, sRGB target
    scope: vec4<f32>,
    trace: vec4<f32>,
    scope_color: vec4<f32>,
    accent_strong: vec4<f32>,
}
@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) half_size: vec2<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) @interpolate(flat) envelope: vec4<f32>,
}
fn corner(vertex: u32) -> vec2<f32> {
    let corners = array<vec2<f32>, 6>(vec2(-1.,-1.),vec2(1.,-1.),vec2(-1.,1.),vec2(-1.,1.),vec2(1.,-1.),vec2(1.,1.));
    return corners[vertex];
}
fn position(p: vec2<f32>) -> vec4<f32> {
    return vec4(p / globals.viewport.xy * vec2(2.,-2.) + vec2(-1.,1.), 0., 1.);
}
@vertex fn shape_vertex(@builtin(vertex_index) vertex: u32,
    @location(0) rect: vec4<f32>, @location(1) color: vec4<f32>, @location(2) params: vec4<f32>) -> VertexOut {
    var out: VertexOut;
    out.half_size = rect.zw * .5;
    // Rings extend beyond their centerline bounds; retain a pixel for coverage
    // outside the stroke as well, so its outer edge is not clipped by the quad.
    let pad = 1. / globals.viewport.z + select(0., params.x*.5, params.y == 5.);
    out.local = corner(vertex) * (out.half_size + vec2(pad));
    out.position = position(rect.xy + out.half_size + out.local);
    out.color = color;
    out.params = params;
    out.envelope = vec4(0.);
    if params.y == 4. {
        let a = rect.xy;
        let b = rect.zw;
        let c = params.zw;
        let lo = min(a,min(b,c))-vec2(pad);
        let hi = max(a,max(b,c))+vec2(pad);
        let p = mix(lo,hi,(corner(vertex)+vec2(1.))*.5);
        out.position = position(p);
        out.local = p-a;
        out.envelope = vec4(b-a,c-a);
    }
    return out;
}
@vertex fn trace_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32,
    @location(0) samples: vec4<f32>) -> VertexOut {
    let columns = max(1u,u32(globals.trace.x));
    let channel = instance / columns;
    let column = instance % columns;
    let height = globals.scope.w / max(1.,globals.trace.y);
    let center = globals.scope.y + (f32(channel)+.5)*height;
    // Bucket centers leave room for round caps at both plot edges.
    let width = globals.scope.z / f32(columns);
    let bounds = -samples.yxwz*max(0.,height*.5-2.);
    let slope = max(abs(bounds.z-bounds.x),abs(bounds.w-bounds.y))/max(width,.001);
    let pad = 1.5/globals.viewport.z*sqrt(1.+slope*slope);
    let t = (corner(vertex).x+1.)*.5;
    let lower = mix(bounds.x,bounds.z,t)-pad;
    let upper = mix(bounds.y,bounds.w,t)+pad;
    let y = clamp(select(lower,upper,corner(vertex).y>0.),-height*.5,height*.5);
    var out: VertexOut;
    out.half_size = vec2(0.);
    out.local = vec2(t,y);
    out.position = position(vec2(globals.scope.x+(f32(column)+t)*width,center+y));
    out.color = globals.scope_color;
    out.params = vec4(0.,3.,0.,0.);
    out.envelope = bounds;
    return out;
}
fn cross2(a:vec2<f32>,b:vec2<f32>)->f32 { return a.x*b.y-a.y*b.x; }
fn segment(p:vec2<f32>,a:vec2<f32>,b:vec2<f32>)->f32 {
    let delta=b-a;
    return length(p-a-delta*clamp(dot(p-a,delta)/max(dot(delta,delta),.000001),0.,1.));
}
fn cubic_distance(p:vec2<f32>,a:vec2<f32>,b:vec2<f32>,c:vec2<f32>,d:vec2<f32>)->f32 {
    var previous=a;
    var distance=1e10;
    // At the largest UI logo, even 4x display scaling keeps the subdivision
    // error well below one pixel. Coverage is evaluated once for the union.
    for(var i=1u;i<=32u;i+=1u) {
        let t=f32(i)/32.;let u=1.-t;
        let next=u*u*u*a+3.*u*u*t*b+3.*u*t*t*c+t*t*t*d;
        distance=min(distance,segment(p,previous,next));
        previous=next;
    }
    return distance;
}
fn logo_wave_distance(p:vec2<f32>)->f32 {
    // The three cubic Beziers in the canonical Onda SVG, centered at (256,256).
    return min(min(
        cubic_distance(p,vec2(-89.,0.),vec2(-68.,-3.),vec2(-58.,-42.),vec2(-31.,-42.)),
        cubic_distance(p,vec2(-31.,-42.),vec2(1.,-42.),vec2(-1.,42.),vec2(31.,42.))),
        cubic_distance(p,vec2(31.,42.),vec2(58.,42.),vec2(68.,3.),vec2(89.,0.)))-11.;
}
fn linear(color:vec3<f32>)->vec3<f32> {
    return select(pow((color+vec3(.055))/1.055,vec3(2.4)),color/12.92,color<=vec3(.04045));
}
@fragment fn fragment(in: VertexOut) -> @location(0) vec4<f32> {
    var distance: f32;
    var ring_distance = 0.;
    if in.params.y == 3. {
        let low = mix(in.envelope.x,in.envelope.z,in.local.x);
        let high = mix(in.envelope.y,in.envelope.w,in.local.x);
        distance = max(low-in.local.y,in.local.y-high)-.65/globals.viewport.z;
    } else if in.params.y == 1. {
        let radius = in.half_size.x*25./29.;
        let sweep=4.71238898*clamp(in.params.z,0.,1.);
        let angle = (atan2(in.local.y,in.local.x) + 3.926990817) % 6.283185307;
        distance = abs(length(in.local)-radius)-in.params.x*.5;
        if in.params.w>0. {
            if angle>sweep {
                let start=vec2(cos(2.35619449),sin(2.35619449))*radius;
                let end=vec2(cos(2.35619449+sweep),sin(2.35619449+sweep))*radius;
                distance=min(length(in.local-start),length(in.local-end))-in.params.x*.5;
            }
        } else {
            let edge=select(min(angle-sweep,6.283185307-angle),-min(angle,sweep-angle),angle<=sweep)*radius;
            distance=max(distance,edge);
        }
        if in.params.z<=0. {discard;}
    } else if in.params.y == 2. {
        distance=segment(in.local,-in.params.zw*.5,in.params.zw*.5)-in.params.x;
    } else if in.params.y == 4. {
        let b=in.envelope.xy;let c=in.envelope.zw;let p=in.local;
        let orientation=sign(cross2(b,c));
        let inside=min(min(orientation*cross2(b,p),orientation*cross2(c-b,p-b)),orientation*cross2(-c,p-c));
        distance=min(min(segment(p,vec2(0.),b),segment(p,b,c)),segment(p,c,vec2(0.)))*select(1.,-1.,inside>=0.);
    } else if in.params.y == 5. {
        ring_distance=length(in.local)-in.half_size.x;
        distance=abs(ring_distance)-in.params.x*.5;
    } else if in.params.y == 7. {
        distance=logo_wave_distance(in.local/in.params.x)*in.params.x;
    } else {
        let radius = min(in.params.x,min(in.half_size.x,in.half_size.y));
        let q = abs(in.local)-in.half_size+vec2(radius);
        distance = length(max(q,vec2(0.))) + min(max(q.x,q.y),0.)-radius;
    }
    // Neighboring pixels can cross both edges of a thin ring. Use the signed
    // radial distance so its centerline fold cannot cancel the AA gradient.
    let aa_distance = select(distance,ring_distance,in.params.y == 5.);
    let aa = max(fwidth(aa_distance),.001);
    let coverage = 1.-smoothstep(-aa*.5,aa*.5,distance);
    var rgb=in.color.rgb;
    if in.params.y==6. {
        let t=clamp((in.local.x/max(in.half_size.x,.001)+1.)*.5,0.,1.);
        rgb=mix(rgb,globals.accent_strong.rgb,t);
    }
    let color=select(rgb,linear(rgb),globals.viewport.w>0.);
    return vec4(color,in.color.a*coverage);
}
