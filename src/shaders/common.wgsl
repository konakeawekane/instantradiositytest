enable wgpu_ray_query;
const PI: f32 = 3.14159265359;
struct Frame {
    vp: mat4x4<f32>, inv_vp: mat4x4<f32>, previous_vp: mat4x4<f32>, previous_inv_vp: mat4x4<f32>,
    camera: vec4<f32>, size: vec4<u32>, counts: vec4<u32>, restir: vec4<u32>,
    temporal: vec4<f32>, sdf: vec4<f32>, rr: vec4<f32>, flags: vec4<u32>,
};
struct Object {model:mat4x4<f32>, previous:mat4x4<f32>, inverse:mat4x4<f32>, info:vec4<u32>};
struct Mesh {lo:vec4<f32>,hi:vec4<f32>,geometry:vec4<u32>,field:vec4<u32>,grid:vec4<f32>,geometry_lo:vec4<f32>,geometry_hi:vec4<f32>,enclosing_sphere:vec4<f32>};
struct Material {color:vec4<f32>,params:vec4<f32>,textures:vec4<i32>,channels:vec4<u32>};
struct Light {position_strength:vec4<f32>,direction_angle:vec4<f32>,color_type:vec4<f32>};
struct Vertex {position:vec4<f32>,normal:vec4<f32>,tangent:vec4<f32>,uv:vec4<f32>};
struct Node {lo:vec4<f32>,hi:vec4<f32>,info:vec4<u32>};
struct Vpl {position:vec4<f32>,normal:vec4<f32>,flux:vec4<f32>};
struct Reservoir {id:u32,source:u32,count:u32,age:u32,sum:f32,selected_target:f32,pdf:f32,generation:u32};
@group(0) @binding(0) var<uniform> frame:Frame;
@group(0) @binding(1) var<storage,read> objects:array<Object>;
@group(0) @binding(2) var<storage,read> meshes:array<Mesh>;
@group(0) @binding(3) var<storage,read> materials:array<Material>;
@group(0) @binding(4) var<storage,read> lights:array<Light>;
@group(0) @binding(5) var<storage,read> vertices:array<Vertex>;
@group(0) @binding(6) var<storage,read> indices:array<u32>;
@group(0) @binding(7) var fields:texture_3d<f32>;
@group(0) @binding(8) var<storage,read> nodes:array<Node>;
@group(0) @binding(9) var textures:texture_2d_array<f32>;
@group(0) @binding(10) var tex_sampler:sampler;
@group(0) @binding(11) var tlas:acceleration_structure;
@group(0) @binding(12) var field_sampler:sampler;
@group(0) @binding(13) var srgb_textures:texture_2d_array<f32>;
fn sample_field(origin:vec3<u32>,q:vec3<f32>,res:u32)->f32 {
    let cell=min(vec3<u32>(floor(q)),vec3<u32>(res-2u));let f=q-vec3<f32>(cell);var value=0.;
    for(var z=0u;z<2u;z++){for(var y=0u;y<2u;y++){for(var x=0u;x<2u;x++){
        let w=select(1.-f,f,vec3<bool>(x==1u,y==1u,z==1u));
        value+=textureLoad(fields,vec3<i32>(origin+cell+vec3<u32>(x,y,z)),0).x*w.x*w.y*w.z;
    }}}
    return value;
}
fn random(state:ptr<function,u32>)->f32 {
    *state = *state * 747796405u + 2891336453u;
    let w=((*state >> ((*state >> 28u)+4u)) ^ *state)*277803737u;
    return f32((w >> 22u)^w)*(1.0/4294967296.0);
}
fn seed(i:u32,salt:u32)->u32 { return i*1973u+frame.size.z*9277u+salt*26699u+89173u; }
fn luminance(c:vec3<f32>)->f32 {return dot(c,vec3<f32>(.2126,.7152,.0722));}
fn world(uv:vec2<f32>,depth:f32,m:mat4x4<f32>)->vec3<f32> {let h=m*vec4<f32>(uv.x*2.-1.,1.-uv.y*2.,depth,1.);return h.xyz/h.w;}
fn pixel_uv(p:vec2<u32>)->vec2<f32> {return (vec2<f32>(p)+.5)/vec2<f32>(frame.size.xy);}
fn srgb(c:vec3<f32>)->vec3<f32> {return select(c/12.92,pow((c+.055)/1.055,vec3<f32>(2.4)),c>vec3<f32>(.04045));}
struct Surface {p:vec3<f32>,n:vec3<f32>,color:vec3<f32>,rough:f32,metal:f32};
fn material_at(id:u32,uv:vec2<f32>,n:vec3<f32>,tangent:vec4<f32>)->Surface {
    let texture_uv=vec2<f32>(uv.x,1.-uv.y);
    let m=materials[id];var s:Surface;s.color=m.color.rgb;s.rough=m.params.x;s.metal=m.params.y;s.n=normalize(n);s.p=vec3<f32>(0.);
    if m.textures.x>=0 {
        var c:vec3<f32>;
        if m.params.w>.5 {c=textureSampleLevel(srgb_textures,tex_sampler,texture_uv,m.textures.x,0.).rgb;}
        else {c=textureSampleLevel(textures,tex_sampler,texture_uv,m.textures.x,0.).rgb;}
        s.color*=c;
    }
    if m.textures.y>=0 {s.rough*=textureSampleLevel(textures,tex_sampler,texture_uv,m.textures.y,0.)[m.channels.y];}
    if m.textures.z>=0 {s.metal*=textureSampleLevel(textures,tex_sampler,texture_uv,m.textures.z,0.)[m.channels.z];}
    if m.textures.w>=0 {
        var nm=textureSampleLevel(textures,tex_sampler,texture_uv,m.textures.w,0.).xyz*2.-1.;nm=normalize(vec3<f32>(nm.xy*m.params.z,nm.z));
        let t=normalize(tangent.xyz-s.n*dot(s.n,tangent.xyz));let b=cross(s.n,t)*tangent.w;s.n=normalize(t*nm.x+b*nm.y+s.n*nm.z);
    }
    s.rough=clamp(s.rough,.045,1.);s.metal=clamp(s.metal,0.,1.);return s;
}
fn brdf(s:Surface,wo:vec3<f32>,wi:vec3<f32>)->vec3<f32> {
    let nv=max(dot(s.n,wo),.00001);let nl=max(dot(s.n,wi),0.);if nl<=0. {return vec3<f32>(0.);}
    let h=normalize(wo+wi);let nh=max(dot(s.n,h),0.);let vh=max(dot(wo,h),0.);
    let a=s.rough*s.rough;let a2=a*a;let q=nh*nh*(a2-1.)+1.;let d=a2/(PI*q*q);
    let g1v=2.*nv/(nv+sqrt(a2+(1.-a2)*nv*nv));let g1l=2.*nl/(nl+sqrt(a2+(1.-a2)*nl*nl));
    let f0=mix(vec3<f32>(.04),s.color,s.metal);let f=f0+(1.-f0)*pow(1.-vh,5.);
    return (1.-f)*(1.-s.metal)*s.color/PI+f*d*g1v*g1l/max(4.*nv*nl,.00001);
}
fn direct(s:Surface,id:u32)->vec3<f32> {
    let l=lights[id];let delta=l.position_strength.xyz-s.p;let r2=max(dot(delta,delta),.0001);let wi=delta*inverseSqrt(r2);
    if l.color_type.w>0.5 && dot(-wi,normalize(l.direction_angle.xyz))<cos(l.direction_angle.w*.5) {return vec3<f32>(0.);}
    return brdf(s,normalize(frame.camera.xyz-s.p),wi)*max(dot(s.n,wi),0.)*l.color_type.rgb*l.position_strength.w/r2;
}
fn indirect(s:Surface,v:Vpl)->vec3<f32> {
    let delta=v.position.xyz-s.p;let r2=max(dot(delta,delta),.0025);let wi=delta*inverseSqrt(r2);
    return brdf(s,normalize(frame.camera.xyz-s.p),wi)*max(dot(s.n,wi),0.)*max(dot(v.normal.xyz,-wi),0.)*v.flux.rgb/(4.*PI*r2);
}
fn basis(n:vec3<f32>)->mat3x3<f32> {let axis=select(vec3<f32>(0.,0.,1.),vec3<f32>(1.,0.,0.),abs(n.z)>.9);let t=normalize(cross(axis,n));return mat3x3<f32>(t,cross(n,t),n);}
fn cosine_direction(n:vec3<f32>,rng:ptr<function,u32>)->vec3<f32> {let u=random(rng);let phi=2.*PI*random(rng);return basis(n)*vec3<f32>(sqrt(u)*cos(phi),sqrt(u)*sin(phi),sqrt(1.-u));}
struct Scatter {direction:vec3<f32>,weight:vec3<f32>};
fn sample_bsdf(s:Surface,wo:vec3<f32>,rng:ptr<function,u32>)->Scatter {
    let spec_probability=mix(.25,.9,s.metal);var wi:vec3<f32>;
    if random(rng)<spec_probability {
        let a2=pow(s.rough,4.);let u=random(rng);let cos_theta=sqrt((1.-u)/(1.+(a2-1.)*u));let phi=2.*PI*random(rng);
        let h=basis(s.n)*vec3<f32>(sqrt(1.-cos_theta*cos_theta)*cos(phi),sqrt(1.-cos_theta*cos_theta)*sin(phi),cos_theta);wi=reflect(-wo,h);
    }else {wi=cosine_direction(s.n,rng);}
    let nl=max(dot(s.n,wi),0.);if nl<=0. {return Scatter(wi,vec3<f32>(0.));}
    let h=normalize(wo+wi);let nh=max(dot(s.n,h),0.);let a2=pow(s.rough,4.);let q=nh*nh*(a2-1.)+1.;
    let pdf=mix(nl/PI,a2*nh/(PI*q*q*max(4.*abs(dot(wo,h)),.00001)),spec_probability);
    return Scatter(wi,brdf(s,wo,wi)*nl/max(pdf,.00001));
}
fn empty_reservoir()->Reservoir {return Reservoir(0u,frame.size.z,0u,0u,0.,0.,0.,frame.size.w);}
fn normalization(r:Reservoir)->f32 {if r.count==0u||r.selected_target<=0. {return 0.;}return r.sum/(f32(r.count)*r.selected_target);}
fn add(r:ptr<function,Reservoir>,id:u32,source:u32,selected_target:f32,pdf:f32,weight:f32,count:u32,age:u32,rng:ptr<function,u32>)->bool {
    (*r).count+=count;
    if weight<=0.||weight>1e30 {return false;}
    (*r).sum+=weight;
    if random(rng)*(*r).sum<weight {(*r).id=id;(*r).source=source;(*r).selected_target=selected_target;(*r).pdf=pdf;(*r).age=age;return true;}
    return false;
}
