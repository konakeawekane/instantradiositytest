@group(1) @binding(0) var<storage,read_write> pool:array<Vpl>;
@compute @workgroup_size(64) fn generate(@builtin(global_invocation_id) gid:vec3<u32>) {
    let id=gid.x;if id>=frame.counts.y{return;}
    var rng=seed(id,7u);var v:Vpl;v.position=vec4<f32>(0.);v.normal=vec4<f32>(0.);v.flux=vec4<f32>(0.);
    if frame.counts.x==0u {pool[id]=v;return;}
    let light_id=min(u32(random(&rng)*f32(frame.counts.x)),frame.counts.x-1u);let l=lights[light_id];
    var origin=l.position_strength.xyz;var direction:vec3<f32>;var omega=4.*PI;
    if l.color_type.w>.5 {
        omega=2.*PI*(1.-cos(l.direction_angle.w*.5));let z=mix(cos(l.direction_angle.w*.5),1.,random(&rng));let phi=2.*PI*random(&rng);
        direction=basis(normalize(l.direction_angle.xyz))*vec3<f32>(sqrt(1.-z*z)*cos(phi),sqrt(1.-z*z)*sin(phi),z);
    }else{let z=2.*random(&rng)-1.;let phi=2.*PI*random(&rng);direction=vec3<f32>(sqrt(1.-z*z)*cos(phi),sqrt(1.-z*z)*sin(phi),z);}
    // Source power and path-budget normalization must not change roulette's
    // path distribution. Track dimensionless scattering throughput separately.
    let source_flux=l.color_type.rgb*l.position_strength.w*omega*f32(frame.counts.x);
    var throughput=vec3<f32>(1.);
    for(var bounce=0u;bounce<frame.counts.z;bounce++) {
        var query:ray_query;
        rayQueryInitialize(&query,tlas,RayDesc(0u,255u,.0005,10000.,origin,direction));
        while rayQueryProceed(&query) {}
        let hit=rayQueryGetCommittedIntersection(&query);
        if hit.kind==0u {v.flux=vec4<f32>(0.);break;}
        let object=objects[hit.instance_custom_data];let mesh=meshes[object.info.x];
        let base=mesh.geometry.z+hit.primitive_index*3u;
        let a=vertices[mesh.geometry.x+indices[base]];let b=vertices[mesh.geometry.x+indices[base+1u]];let c=vertices[mesh.geometry.x+indices[base+2u]];
        let bary=vec3<f32>(1.-hit.barycentrics.x-hit.barycentrics.y,hit.barycentrics.x,hit.barycentrics.y);
        let local_n=a.normal.xyz*bary.x+b.normal.xyz*bary.y+c.normal.xyz*bary.z;
        let tangent=a.tangent*bary.x+b.tangent*bary.y+c.tangent*bary.z;
        let n=normalize((object.model*vec4<f32>(local_n,0.)).xyz);
        var s=material_at(object.info.y,a.uv.xy*bary.x+b.uv.xy*bary.y+c.uv.xy*bary.z,n,vec4<f32>((object.model*vec4<f32>(tangent.xyz,0.)).xyz,select(-1.,1.,tangent.w>=0.)));
        s.p=origin+direction*hit.t;
        let wo=-direction;
        if dot(s.n,wo)<=0. {v.flux=vec4<f32>(0.);break;}
        v.position=vec4<f32>(s.p,1.);v.normal=vec4<f32>(s.n,0.);
        // The point emitter keeps no outgoing direction. A BSDF/pdf estimate of
        // hemispherical reflected flux includes both Lambert and metallic GGX.
        let scatter=sample_bsdf(s,wo,&rng);
        let reflected=throughput*scatter.weight;
        v.flux=vec4<f32>(source_flux*reflected/f32(frame.counts.y),0.);
        if bounce+1u>=frame.counts.z {break;}
        let survive=clamp(max(reflected.x,max(reflected.y,reflected.z)),frame.rr.x,frame.rr.y);
        if random(&rng)>survive {break;}
        if luminance(scatter.weight)<=0. {break;}
        throughput=reflected/survive;
        origin=s.p+s.n*.001;direction=scatter.direction;
    }
    pool[id]=v;
}
