@group(1) @binding(0) var depth:texture_depth_2d;
@group(1) @binding(1) var normal:texture_2d<f32>;
@group(1) @binding(2) var color:texture_2d<f32>;
@group(1) @binding(3) var params:texture_2d<f32>;
@group(1) @binding(4) var motion:texture_2d<f32>;
@group(2) @binding(0) var<storage,read> direct_in:array<Reservoir>;
@group(2) @binding(1) var<storage,read> indirect_in:array<Reservoir>;
@group(2) @binding(2) var<storage,read_write> direct_out:array<Reservoir>;
@group(2) @binding(3) var<storage,read_write> indirect_out:array<Reservoir>;
@group(2) @binding(4) var<storage,read> current_vpls:array<Vpl>;
@group(2) @binding(5) var<storage,read> previous_vpls:array<Vpl>;
@group(2) @binding(6) var previous_depth:texture_depth_2d;
@group(2) @binding(7) var previous_normal:texture_2d<f32>;
@group(2) @binding(8) var previous_lighting:texture_2d<f32>;
@group(2) @binding(9) var output:texture_storage_2d<rgba16float,write>;
@group(2) @binding(10) var current_lighting:texture_2d<f32>;
@group(2) @binding(12) var<storage,read> direct_history:array<Reservoir>;
@group(2) @binding(13) var<storage,read> indirect_history:array<Reservoir>;
@group(2) @binding(14) var<storage,read> archived_vpls:array<Vpl>;
@group(2) @binding(15) var direct_output:texture_storage_2d<rgba32float,write>;
@group(2) @binding(16) var direct_lighting:texture_2d<f32>;
fn vpl_sample_valid(r:Reservoir)->bool {
    return r.generation==frame.size.w&&r.selected_target>0.&&r.id<frame.counts.y&&frame.size.z-r.source<=u32(frame.rr.z);
}
fn selected_vpl(r:Reservoir)->Vpl {
    if r.source==frame.size.z {return current_vpls[r.id];}
    if r.source+1u==frame.size.z {return previous_vpls[r.id];}
    return archived_vpls[(r.source%(u32(frame.rr.z)+1u))*frame.counts.y+r.id];
}
fn surface(p:vec2<u32>)->Surface {
    let coord=vec2<i32>(p);let rm=textureLoad(params,coord,0).xy;var s:Surface;
    s.p=world(pixel_uv(p),textureLoad(depth,coord,0),frame.inv_vp);s.n=normalize(textureLoad(normal,coord,0).xyz);
    s.color=textureLoad(color,coord,0).rgb;s.rough=rm.x;s.metal=rm.y;return s;
}
fn valid(p:vec2<u32>)->bool {return textureLoad(depth,vec2<i32>(p),0)<1.;}
fn compatible(a:Surface,q:vec2<u32>)->bool {
    let coord=vec2<i32>(q);let d=textureLoad(depth,coord,0);if d>=1. {return false;}
    let p=world(pixel_uv(q),d,frame.inv_vp);let n=normalize(textureLoad(normal,coord,0).xyz);
    return dot(a.n,n)>frame.temporal.z && distance(a.p,p)<frame.temporal.y*max(distance(a.p,frame.camera.xyz),1.);
}
fn reproject(p:vec2<u32>)->vec2<i32> {
    let uv=pixel_uv(p)-textureLoad(motion,vec2<i32>(p),0).xy;
    return vec2<i32>(floor(uv*vec2<f32>(frame.size.xy)));
}
fn history_valid(p:vec2<u32>,q:vec2<i32>)->bool {
    if frame.flags.x==0u||frame.flags.y!=frame.size.w||any(q<vec2<i32>(0))||any(q>=vec2<i32>(frame.size.xy)) {return false;}
    let d=textureLoad(previous_depth,q,0);if d>=1. {return false;}
    let a=surface(p);let oldp=world((vec2<f32>(q)+.5)/vec2<f32>(frame.size.xy),d,frame.previous_inv_vp);
    return dot(a.n,textureLoad(previous_normal,q,0).xyz)>frame.temporal.z && distance(a.p,oldp)<frame.temporal.y*max(distance(a.p,frame.camera.xyz),1.);
}
fn cap(r:Reservoir,limit:u32)->Reservoir {var a=r;if a.count>limit {a.sum*=f32(limit)/f32(a.count);a.count=limit;}return a;}
@compute @workgroup_size(8,8) fn candidates_direct(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var r=empty_reservoir();var rng=seed(id,11u);
    if valid(g.xy)&&frame.counts.x>0u {
        let s=surface(g.xy);let pdf=1./f32(frame.counts.x);
        for(var i=0u;i<frame.restir.x;i++){let light_id=min(u32(random(&rng)*f32(frame.counts.x)),frame.counts.x-1u);let selected_target=luminance(direct(s,light_id));add(&r,light_id,frame.size.z,selected_target,pdf,selected_target/pdf,1u,0u,&rng);}
    }direct_out[id]=r;
}
@compute @workgroup_size(8,8) fn candidates_indirect(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var r=empty_reservoir();var rng=seed(id,13u);
    if valid(g.xy) {
        let s=surface(g.xy);let pdf=1./f32(frame.counts.y);
        for(var i=0u;i<frame.restir.y;i++){let vpl_id=min(u32(random(&rng)*f32(frame.counts.y)),frame.counts.y-1u);let v=current_vpls[vpl_id];let selected_target=luminance(indirect(s,v));add(&r,vpl_id,frame.size.z,selected_target,pdf,selected_target/pdf,1u,0u,&rng);}
    }indirect_out[id]=r;
}
@compute @workgroup_size(8,8) fn temporal_direct(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var r=direct_in[id];let q=reproject(g.xy);var rng=seed(id,17u);
    if valid(g.xy)&&history_valid(g.xy,q) {
        let previous=cap(direct_history[u32(q.y)*frame.size.x+u32(q.x)],u32(frame.rr.z)*frame.restir.x);
        if previous.generation==frame.size.w && previous.selected_target>0. && previous.id<frame.counts.x && previous.age<u32(frame.rr.z) {
            let selected_target=luminance(direct(surface(g.xy),previous.id));add(&r,previous.id,previous.source,selected_target,previous.pdf,selected_target*normalization(previous)*f32(previous.count),previous.count,previous.age+1u,&rng);
        }
    }direct_out[id]=r;
}
@compute @workgroup_size(8,8) fn temporal_indirect(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var r=indirect_in[id];let q=reproject(g.xy);var rng=seed(id,19u);
    if valid(g.xy)&&history_valid(g.xy,q) {
        let previous=cap(indirect_history[u32(q.y)*frame.size.x+u32(q.x)],u32(frame.rr.z)*frame.restir.y);
        if vpl_sample_valid(previous) {
            let v=selected_vpl(previous);
            let selected_target=luminance(indirect(surface(g.xy),v));
            add(&r,previous.id,previous.source,selected_target,previous.pdf,selected_target*normalization(previous)*f32(previous.count),previous.count,previous.age+1u,&rng);
        }
    }indirect_out[id]=r;
}
fn neighbor(p:vec2<u32>,rng:ptr<function,u32>)->vec2<i32> {
    let angle=2.*PI*random(rng);let radius=sqrt(random(rng))*f32(frame.restir.w);
    return vec2<i32>(p)+vec2<i32>(round(vec2<f32>(cos(angle),sin(angle))*radius));
}
@compute @workgroup_size(8,8) fn spatial_direct(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var r=direct_in[id];var rng=seed(id,23u);
    if valid(g.xy) {let s=surface(g.xy);for(var i=0u;i<frame.restir.z;i++) {
        let q=neighbor(g.xy,&rng);if any(q<vec2<i32>(0))||any(q>=vec2<i32>(frame.size.xy)){continue;}
        if !compatible(s,vec2<u32>(q)){continue;}let previous=direct_in[u32(q.y)*frame.size.x+u32(q.x)];
        if previous.selected_target>0. {let selected_target=luminance(direct(s,previous.id));add(&r,previous.id,previous.source,selected_target,previous.pdf,selected_target*normalization(previous)*f32(previous.count),previous.count,previous.age,&rng);}
    }}direct_out[id]=cap(r,u32(frame.rr.z)*frame.restir.x*8u);
}
@compute @workgroup_size(8,8) fn spatial_indirect(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var r=indirect_in[id];var rng=seed(id,29u);
    if valid(g.xy) {let s=surface(g.xy);for(var i=0u;i<frame.restir.z;i++) {
        let q=neighbor(g.xy,&rng);if any(q<vec2<i32>(0))||any(q>=vec2<i32>(frame.size.xy)){continue;}
        if !compatible(s,vec2<u32>(q)){continue;}let previous=indirect_in[u32(q.y)*frame.size.x+u32(q.x)];
        if vpl_sample_valid(previous) {let selected_target=luminance(indirect(s,selected_vpl(previous)));add(&r,previous.id,previous.source,selected_target,previous.pdf,selected_target*normalization(previous)*f32(previous.count),previous.count,previous.age,&rng);}
    }}indirect_out[id]=cap(r,u32(frame.rr.z)*frame.restir.y*8u);
}
@compute @workgroup_size(8,8) fn resolve_direct(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;var result=vec3<f32>(0.);
    if valid(g.xy) {
        let s=surface(g.xy);let d=direct_in[id];
        if d.selected_target>0. && frame.flags.z!=2u && frame.flags.z<3u {let value=direct(s,d.id);if luminance(value)>0. {result+=value*normalization(d)*visibility(s.p,s.n,lights[d.id].position_strength.xyz,vec3<f32>(0.));}}
        if frame.flags.z==3u {result=s.color;}
        if frame.flags.z==4u {result=s.n*.5+.5;}
        // Motion is a signed screen-space vector.  Encode x/y independently so
        // the debug view can reveal direction as well as magnitude: red is x,
        // green is y, and neutral grey means zero motion.
        if frame.flags.z==5u {
            let m=textureLoad(motion,vec2<i32>(g.xy),0).xy;
            result=vec3<f32>(clamp(.5+m.x*50.,0.,1.),clamp(.5+m.y*50.,0.,1.),.5);
        }
    }
    textureStore(direct_output,vec2<i32>(g.xy),vec4<f32>(result,1.));
}
@compute @workgroup_size(8,8) fn resolve_indirect(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let id=g.y*frame.size.x+g.x;
    var result=textureLoad(direct_lighting,vec2<i32>(g.xy),0).rgb;
    if valid(g.xy) && frame.flags.z!=1u && frame.flags.z<3u {
        let v=indirect_in[id];
        if vpl_sample_valid(v) {
            let s=surface(g.xy);let sample=selected_vpl(v);let value=indirect(s,sample);
            if luminance(value)>0. {result+=value*normalization(v)*visibility(s.p,s.n,sample.position.xyz,sample.normal.xyz);}
        }
    }
    textureStore(output,vec2<i32>(g.xy),vec4<f32>(min(result,vec3<f32>(65000.)),1.));
}
