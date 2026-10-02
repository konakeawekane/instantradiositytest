struct Out {@builtin(position) clip:vec4<f32>,@location(0) normal:vec3<f32>,@location(1) tangent:vec4<f32>,@location(2) uv:vec2<f32>,@location(3) current:vec4<f32>,@location(4) previous:vec4<f32>,@location(5) @interpolate(flat) material:u32};
@vertex fn vs(@location(0) p:vec4<f32>,@location(1) n:vec4<f32>,@location(2) tangent:vec4<f32>,@location(3) uv:vec4<f32>,@builtin(instance_index) id:u32)->Out {
    let o=objects[id];var v:Out;v.clip=frame.vp*o.model*p;v.current=v.clip;v.previous=frame.previous_vp*o.previous*p;
    v.normal=normalize((o.model*vec4<f32>(n.xyz,0.)).xyz);v.tangent=vec4<f32>(normalize((o.model*vec4<f32>(tangent.xyz,0.)).xyz),tangent.w);
    v.uv=uv.xy;v.material=o.info.y;return v;
}
struct Gbuffer {@location(0) normal:vec4<f32>,@location(1) color:vec4<f32>,@location(2) params:vec2<f32>,@location(3) motion:vec2<f32>};
@fragment fn fs(v:Out)->Gbuffer {
    let s=material_at(v.material,v.uv,v.normal,v.tangent);var g:Gbuffer;
    g.normal=vec4<f32>(s.n,1.);g.color=vec4<f32>(s.color,1.);g.params=vec2<f32>(s.rough,s.metal);
    let current=v.current.xy/v.current.w;let previous=v.previous.xy/v.previous.w;g.motion=(current-previous)*vec2<f32>(.5,-.5);return g;
}
