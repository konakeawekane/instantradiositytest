fn signed_box_distance(p:vec3<f32>,lo:vec3<f32>,hi:vec3<f32>)->f32 {
    let delta=max(lo-p,p-hi);
    return length(max(delta,vec3<f32>(0.)))+min(max(delta.x,max(delta.y,delta.z)),0.);
}
fn mesh_distance(p:vec3<f32>,mesh:Mesh)->f32 {
    let geometry_bound=signed_box_distance(p,mesh.geometry_lo.xyz,mesh.geometry_hi.xyz);
    // Verified box topology makes this bound the exact mesh distance, which
    // dominates the sampled field. Skip a texture lookup with the same result.
    if mesh.geometry_lo.w>.5 {return geometry_bound;}
    let res=mesh.field.w;
    let clamped=clamp(p,mesh.lo.xyz,mesh.hi.xyz);let q=(clamped-mesh.lo.xyz)*mesh.grid.xyz;
    let value=sample_field(mesh.field.xyz,q,res);
    // Jensen's inequality bounds trilinear interpolation error by half the
    // voxel diagonal. This is continuous across cell boundaries and remains
    // a signed lower bound; unlike the exact distance it can be negative outside.
    // Extra interpolation allowance also covers fixed-point hardware weights.
    var bound=value-mesh.grid.w*(.5+1./128.);
    let outside=max(max(mesh.lo.xyz-p,p-mesh.hi.xyz),vec3<f32>(0.));
    let outside_squared=dot(outside,outside);
    // Projection onto the padded box gives a nonnegative cross term to every
    // mesh point. Pythagoras extends the bound without a box-boundary dip.
    if outside_squared>0. {let positive=max(bound,0.);bound=sqrt(positive*positive+outside_squared);}
    // The measured maximum of analytic cell-corner gradients bounds every
    // trilinear gradient. Scale positive distances for a 1-Lipschitz penumbra
    // field; preserve signed interior bounds. Extension also requires L >= 1.
    if bound>0. {bound*=mesh.geometry_hi.w;}
    // Containment makes the signed distance to the mesh's enclosing box a
    // lower bound as well. The maximum of conservative, continuous bounds is
    // conservative and continuous, and has the same positive Lipschitz bound.
    // This removes unnecessary underestimation near flat enclosing faces.
    let sphere_bound=length(p-mesh.enclosing_sphere.xyz)-mesh.enclosing_sphere.w;
    return max(bound,max(geometry_bound,sphere_bound));
}
struct Distance {distance:f32,voxel:f32,object_id:u32};
fn object_distance(p:vec3<f32>,id:u32,need_spacing:bool)->vec2<f32> {
    let obj=objects[id];let mesh=meshes[obj.info.x];let local=(obj.inverse*vec4<f32>(p,1.)).xyz;
    var d=mesh_distance(local,mesh)*bitcast<f32>(obj.info.z);
    if d>0. {d*=.9999;}
    var voxel=frame.sdf.y;if need_spacing {voxel=bitcast<f32>(obj.info.w);}
    return vec2<f32>(d,voxel);
}
fn scene_distance(p:vec3<f32>,need_spacing:bool,hint:u32)->Distance {
    var best=1e20;var voxel=frame.sdf.y;var closest=hint;var cursor=0u;
    if hint<frame.counts.w {let d=object_distance(p,hint,need_spacing);best=d.x;voxel=d.y;}
    // Preorder hierarchy with escape links needs no per-thread traversal stack.
    while cursor<frame.counts.w*2u-1u {
        let node=nodes[cursor];
        let delta=max(max(node.lo.xyz-p,p-node.hi.xyz),vec3<f32>(0.));
        // The geometry-box bound in mesh_distance is at least the distance to
        // this enclosing world-space box. Match the object query's numerical
        // safety factor, with a little extra allowance for transformed bounds.
        let lower_bound=delta*.9998;
        if dot(lower_bound,lower_bound)>max(best,0.)*max(best,0.) {cursor=node.info.w;continue;}
        if node.info.z==1u {
            if node.info.x!=hint {let d=object_distance(p,node.info.x,need_spacing);if d.x<best {best=d.x;voxel=d.y;closest=node.info.x;}}
        }
        cursor++;
    }
    return Distance(best,voxel,closest);
}
fn visibility(p:vec3<f32>,n:vec3<f32>,selected_target:vec3<f32>,end_normal:vec3<f32>)->f32 {
    let clearance=scene_distance(p,true,0xffffffffu);let epsilon=max(frame.sdf.y,clearance.voxel*.65);
    var closest=clearance.object_id;
    let origin=p+n*epsilon;
    var endpoint=selected_target;
    if dot(end_normal,end_normal)>.5 {endpoint+=end_normal*max(frame.sdf.y,scene_distance(selected_target,true,0xffffffffu).voxel*.65);}
    let delta=endpoint-origin;let limit=length(delta)-frame.sdf.y;
    if limit<=0. {return 1.;}let direction=normalize(delta);
    var t=frame.sdf.y;var result=1.;var previous=1e20;
    for(var i=0u;i<u32(frame.sdf.x);i++){
        if t>=limit {return clamp(result,0.,1.);}
        let query=scene_distance(origin+direction*t,false,closest);let h=query.distance;closest=query.object_id;
        if h<frame.sdf.y {return 0.;}
        let y=h*h/(2.*previous);let d=sqrt(max(h*h-y*y,0.));
        if t>y {result=min(result,frame.sdf.z*d/(t-y));}
        if result<.001 {return 0.;}
        previous=h;t+=h;
    }
    // A budget-exhausted ray is unresolved; avoid leaking through geometry.
    return 0.;
}
