// SVGF (Schied et al., 2017): temporal luminance moments, short-history
// spatial variance bootstrap, variance-guided edge-avoiding a-trous wavelets.
@group(1) @binding(0) var depth:texture_depth_2d;
@group(1) @binding(1) var normal:texture_2d<f32>;
@group(1) @binding(2) var color:texture_2d<f32>;
@group(1) @binding(3) var params:texture_2d<f32>;
@group(1) @binding(4) var motion:texture_2d<f32>;
@group(2) @binding(0) var noisy:texture_2d<f32>;
@group(2) @binding(1) var old_radiance:texture_2d<f32>;
@group(2) @binding(2) var old_moments:texture_2d<f32>;
@group(2) @binding(3) var input_radiance:texture_2d<f32>;
@group(2) @binding(4) var moments:texture_2d<f32>;
@group(2) @binding(5) var radiance_out:texture_storage_2d<rgba16float,write>;
@group(2) @binding(6) var moments_out:texture_storage_2d<rgba32float,write>;
@group(2) @binding(7) var<uniform> wavelet:vec4<u32>;
@group(2) @binding(8) var old_depth:texture_depth_2d;
@group(2) @binding(9) var old_normal:texture_2d<f32>;
fn inside(p:vec2<i32>)->bool {return all(p>=vec2<i32>(0))&&all(p<vec2<i32>(frame.size.xy));}
fn position(p:vec2<i32>)->vec3<f32> {return world(pixel_uv(vec2<u32>(p)),textureLoad(depth,p,0),frame.inv_vp);}
fn history_ok(p:vec2<i32>,q:vec2<i32>)->bool {
    if frame.flags.x==0u||frame.flags.y!=frame.size.w||!inside(q){return false;}
    let d=textureLoad(old_depth,q,0);if d>=1. {return false;}
    let a=position(p);let b=world(pixel_uv(vec2<u32>(q)),d,frame.previous_inv_vp);
    return distance(a,b)<frame.temporal.y*max(distance(a,frame.camera.xyz),1.)
        &&dot(textureLoad(normal,p,0).xyz,textureLoad(old_normal,q,0).xyz)>frame.temporal.z;
}
struct Guide {position:vec3<f32>,normal:vec3<f32>,color:vec3<f32>,material:vec2<f32>,pixel_scale:f32};
fn guide(p:vec2<i32>)->Guide {
    let wp=position(p);
    return Guide(wp,textureLoad(normal,p,0).xyz,textureLoad(color,p,0).rgb,textureLoad(params,p,0).xy,distance(wp,frame.camera.xyz)/f32(frame.size.y));
}
fn geometry_weight(a:Guide,q:vec2<i32>,stride:f32)->f32 {
    if !inside(q)||textureLoad(depth,q,0)>=1. {return 0.;}
    let nq=textureLoad(normal,q,0).xyz;
    let wn=pow(max(dot(a.normal,nq),0.),64.);
    // Plane distance preserves sloped planar surfaces and rejects silhouettes.
    let wq=position(q);
    let tolerance=max(.0001,a.pixel_scale*stride*.5);
    let wz=exp(-abs(dot(wq-a.position,a.normal))/tolerance);
    // Filtering full BRDF radiance (also glossy/metallic) must preserve material
    // boundaries; do not demodulate total radiance by diffuse albedo.
    let wc=exp(-length(a.color-textureLoad(color,q,0).rgb)*8.-length(a.material-textureLoad(params,q,0).xy)*8.);
    return wn*wz*wc;
}
@compute @workgroup_size(8,8) fn svgf_temporal(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let p=vec2<i32>(g.xy);
    let c=textureLoad(noisy,p,0).rgb;let l=luminance(c);
    var history=vec3<f32>(0.);var hm=vec2<f32>(0.);var length_history=0.;var total=0.;
    if textureLoad(depth,p,0)<1. {
        let previous=(pixel_uv(g.xy)-textureLoad(motion,p,0).xy)*vec2<f32>(frame.size.xy)-.5;
        let base=vec2<i32>(floor(previous));let f=fract(previous);
        for(var y=0;y<2;y++){for(var x=0;x<2;x++){
            let q=base+vec2<i32>(x,y);if !history_ok(p,q){continue;}
            let m=textureLoad(old_moments,q,0);if m.z<=0. {continue;}
            let wx=select(1.-f.x,f.x,x==1);let wy=select(1.-f.y,f.y,y==1);let w=wx*wy;
            history+=textureLoad(old_radiance,q,0).rgb*w;hm+=m.xy*w;length_history+=m.z*w;total+=w;
        }}
    }
    var age=1.;var result=c;var m=vec2<f32>(l,l*l);
    if total>0.001 {
        history/=total;hm/=total;age=min(length_history/total+1.,frame.temporal.w);
        // Broad neighbourhood clamp suppresses stale/outlier history while
        // allowing stochastic visibility zeros and bright samples to accumulate.
        var lo=1e20;var hi=0.;let a=guide(p);
        for(var y=-1;y<=1;y++){for(var x=-1;x<=1;x++){
            let q=clamp(p+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(frame.size.xy)-1);
            if geometry_weight(a,q,1.)>.01 {let v=luminance(textureLoad(noisy,q,0).rgb);lo=min(lo,v);hi=max(hi,v);}
        }}
        let lh=luminance(history);history*=clamp(lh,lo*.5,hi*1.5+.001)/max(lh,.000001);
        let alpha=max(1.-frame.temporal.x,1./age);
        result=mix(history,c,alpha);m=mix(hm,m,max(.2,1./age));
    }
    let variance=max(m.y-m.x*m.x,0.);
    textureStore(radiance_out,p,vec4<f32>(result,min(sqrt(variance),65000.)));
    textureStore(moments_out,p,vec4<f32>(m,age,0.));
}
@compute @workgroup_size(8,8) fn svgf_variance(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let p=vec2<i32>(g.xy);let c=textureLoad(input_radiance,p,0);let age=textureLoad(moments,p,0).z;
    var sigma=c.a;
    if age<4.&&textureLoad(depth,p,0)<1. {
        var sums=vec2<f32>(0.);var total=0.;let a=guide(p);
        for(var y=-3;y<=3;y++){for(var x=-3;x<=3;x++){
            let q=p+vec2<i32>(x,y);let w=geometry_weight(a,q,1.);if w<=0. {continue;}
            sums+=textureLoad(moments,q,0).xy*w;total+=w;
        }}
        sums/=max(total,.00001);sigma=sqrt(max(sums.y-sums.x*sums.x,0.)*4./max(age,1.));
    }
    textureStore(radiance_out,p,vec4<f32>(c.rgb,min(sigma,65000.)));
}
fn kernel(x:i32)->f32 {if x==0{return 1.;}if abs(x)==1{return 2./3.;}return 1./6.;}
@compute @workgroup_size(8,8) fn svgf_atrous(@builtin(global_invocation_id) g:vec3<u32>) {
    if any(g.xy>=frame.size.xy){return;}let p=vec2<i32>(g.xy);let c=textureLoad(input_radiance,p,0);
    if textureLoad(depth,p,0)>=1. {textureStore(radiance_out,p,c);return;}
    // Smooth the variance guide before computing the luminance stopping term.
    var variance=0.;var guide_weight=0.;let a=guide(p);
    for(var y=-1;y<=1;y++){for(var x=-1;x<=1;x++){
        let q=clamp(p+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(frame.size.xy)-1);
        let w=select(1.,2.,x==0)*select(1.,2.,y==0);
        let s=textureLoad(input_radiance,q,0).a;variance+=s*s*w;guide_weight+=w;
    }}
    let phi=4.*sqrt(variance/max(guide_weight,.00001))+.0001;
    var sum=vec3<f32>(0.);var total=0.;var v=0.;let step=i32(wavelet.x);
    for(var y=-2;y<=2;y++){for(var x=-2;x<=2;x++){
        let q=p+vec2<i32>(x,y)*step;let geom=geometry_weight(a,q,f32(step));if geom<=0. {continue;}
        let sample=textureLoad(input_radiance,q,0);
        let w=kernel(x)*kernel(y)*geom*exp(-abs(luminance(sample.rgb)-luminance(c.rgb))/phi);
        sum+=sample.rgb*w;v+=sample.a*sample.a*w*w;total+=w;
    }}
    textureStore(radiance_out,p,vec4<f32>(sum/max(total,.00001),min(sqrt(v)/max(total,.00001),65000.)));
}
