@group(0) @binding(0) var hdr:texture_2d<f32>;
struct Out {@builtin(position) p:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex fn vs(@builtin(vertex_index) id:u32)->Out {
    let p=vec2<f32>(f32((id<<1u)&2u),f32(id&2u));var o:Out;o.p=vec4<f32>(p*2.-1.,0.,1.);o.uv=vec2<f32>(p.x,1.-p.y);return o;
}
@fragment fn fs(v:Out)->@location(0) vec4<f32> {
    return vec4<f32>(textureLoad(hdr,vec2<i32>(v.uv*vec2<f32>(textureDimensions(hdr))),0).rgb,1.);
}
