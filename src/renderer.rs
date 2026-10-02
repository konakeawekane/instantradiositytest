use crate::{scene::Scene, settings::Settings};
use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use std::{borrow::Cow, collections::HashMap, path::Path};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuObject {
    model: Mat4,
    previous: Mat4,
    inverse: Mat4,
    meta: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMesh {
    lo: [f32; 4],
    hi: [f32; 4],
    geometry: [u32; 4],
    field: [u32; 4],
    // XYZ convert local positions to grid coordinates; W is the voxel diagonal.
    grid: [f32; 4],
    geometry_lo: [f32; 4],
    geometry_hi: [f32; 4],
    enclosing_sphere: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMaterial {
    color: [f32; 4],
    params: [f32; 4],
    textures: [i32; 4],
    channels: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuLight {
    position_strength: [f32; 4],
    direction_angle: [f32; 4],
    color_type: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Node {
    lo: [f32; 4],
    hi: [f32; 4],
    meta: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Frame {
    vp: Mat4,
    inv_vp: Mat4,
    previous_vp: Mat4,
    previous_inv_vp: Mat4,
    camera: [f32; 4],
    size: [u32; 4],
    counts: [u32; 4],
    restir: [u32; 4],
    temporal: [f32; 4],
    sdf: [f32; 4],
    rr: [f32; 4],
    flags: [u32; 4],
}
pub struct Target {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}
impl Target {
    pub(crate) fn new(
        d: &wgpu::Device,
        label: &str,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
        storage: bool,
    ) -> Self {
        let texture = d.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | if storage {
                    wgpu::TextureUsages::STORAGE_BINDING
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }
}
pub struct Renderer {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub settings: Settings,
    common: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    object_buffer: wgpu::Buffer,
    material_buffer: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    node_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    mesh_meta: Vec<GpuMesh>,
    blas: Vec<wgpu::Blas>,
    tlas: wgpu::Tlas,
    depth: Target,
    normal: Target,
    color: Target,
    params: Target,
    motion: Target,
    previous_depth: Target,
    previous_normal: Target,
    lighting: Target,
    direct_lighting: Target,
    unused_lighting_output: Target,
    histories: [Target; 2],
    svgf: crate::svgf::Svgf,
    pub display: Target,
    gbuffer_group: wgpu::BindGroup,
    direct: [wgpu::Buffer; 3],
    indirect: [wgpu::Buffer; 3],
    vpls: [wgpu::Buffer; 2],
    archived_vpls: wgpu::Buffer,
    depth_pipeline: wgpu::RenderPipeline,
    gbuffer_pipeline: wgpu::RenderPipeline,
    vpl_pipeline: wgpu::ComputePipeline,
    vpl_groups: [wgpu::BindGroup; 2],
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
    pass_layout: wgpu::BindGroupLayout,
    present_pipeline: wgpu::RenderPipeline,
    present_groups: [wgpu::BindGroup; 2],
    present_layout: wgpu::BindGroupLayout,
    previous_models: Vec<Mat4>,
    previous_vp: Mat4,
    frame_index: u32,
    previous_generation: u32,
    previous_debug: u32,
    pub debug_mode: u32,
    pub last_gpu_ms: Option<f64>,
    pub last_pass_ms: HashMap<&'static str, f64>,
    query: Option<wgpu::QuerySet>,
    query_resolve: Option<wgpu::Buffer>,
    query_read: Option<wgpu::Buffer>,
    timestamp_period: f32,
}
fn buffer<T: Pod>(
    d: &wgpu::Device,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let zero = [0u8; 16];
    let bytes = if data.is_empty() {
        &zero[..]
    } else {
        bytemuck::cast_slice(data)
    };
    d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: usage | wgpu::BufferUsages::COPY_DST,
    })
}
fn storage(d: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    d.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
fn entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}
fn sb(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}
fn tex(depth: bool) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: if depth {
            wgpu::TextureSampleType::Depth
        } else {
            wgpu::TextureSampleType::Float { filterable: false }
        },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}
fn be(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}
fn shader(d: &wgpu::Device, label: &str, source: String) -> wgpu::ShaderModule {
    d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
    })
}
fn resize_material_image(img: &image::RgbaImage, size: u32, srgb: bool) -> image::RgbaImage {
    if !srgb {
        return image::imageops::resize(img, size, size, image::imageops::FilterType::Triangle);
    }
    let linear = image::Rgba32FImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        image::Rgba(std::array::from_fn(|i| {
            let c = p[i] as f32 / 255.;
            if i == 3 || c <= 0.04045 {
                if i == 3 { c } else { c / 12.92 }
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }))
    });
    let resized =
        image::imageops::resize(&linear, size, size, image::imageops::FilterType::Triangle);
    image::RgbaImage::from_fn(size, size, |x, y| {
        let p = resized.get_pixel(x, y);
        image::Rgba(std::array::from_fn(|i| {
            let c = p[i].clamp(0., 1.);
            let encoded = if i == 3 {
                c
            } else if c <= 0.0031308 {
                c * 12.92
            } else {
                1.055 * c.powf(1. / 2.4) - 0.055
            };
            (encoded * 255.).round() as u8
        }))
    })
}
pub fn shader_source(stage: &str) -> String {
    let common = include_str!("shaders/common.wgsl");
    match stage {
        "gbuffer" => format!("{common}\n{}", include_str!("shaders/gbuffer.wgsl")),
        "vpl" => format!("{common}\n{}", include_str!("shaders/vpl.wgsl")),
        "lighting" => format!(
            "{common}\n{}\n{}",
            include_str!("shaders/sdf.wgsl"),
            include_str!("shaders/lighting.wgsl")
        ),
        "present" => include_str!("shaders/present.wgsl").to_string(),
        "svgf" => format!("{common}\n{}", include_str!("shaders/svgf.wgsl")),
        _ => unreachable!(),
    }
}
fn filtered_shader_source(stage: &str) -> String {
    let source = shader_source(stage);
    let start = source.find("fn sample_field(").unwrap();
    let end = source[start..].find("\nfn random(").unwrap() + start;
    format!(
        "{}fn sample_field(origin:vec3<u32>,q:vec3<f32>,res:u32)->f32 {{return textureSampleLevel(fields,field_sampler,(vec3<f32>(origin)+q+.5)/vec3<f32>(textureDimensions(fields)),0.).x;}}\n{}",
        &source[..start],
        &source[end..]
    )
}
fn is_box_geometry(vertices: &[crate::scene::Vertex], indices: &[u32], lo: Vec3, hi: Vec3) -> bool {
    if indices.len() != 36 || (hi - lo).min_element() <= 0. {
        return false;
    }
    let mut corners = Vec::with_capacity(vertices.len());
    for vertex in vertices {
        let p = Vec3::from_slice(&vertex.position[..3]);
        let mut corner = 0u8;
        for axis in 0..3 {
            if p[axis] == hi[axis] {
                corner |= 1 << axis;
            } else if p[axis] != lo[axis] {
                return false;
            }
        }
        corners.push(corner);
    }
    let mut faces: [Vec<[u8; 3]>; 6] = Default::default();
    for triangle in indices.as_chunks::<3>().0 {
        let mut corners = triangle.map(|i| corners[i as usize]);
        corners.sort_unstable();
        if corners[0] == corners[1] || corners[1] == corners[2] {
            return false;
        }
        let Some(axis) = (0..3).find(|axis| {
            let mask = 1 << axis;
            corners.iter().all(|c| c & mask == corners[0] & mask)
        }) else {
            return false;
        };
        let side = ((corners[0] >> axis) & 1) as usize;
        faces[axis * 2 + side].push(corners);
    }
    faces.iter().all(|face| {
        if face.len() != 2 {
            return false;
        }
        let shared: Vec<_> = face[0].iter().filter(|c| face[1].contains(c)).collect();
        // Two distinct triangles must share an opposite-corner diagonal.
        // This proves that they tile the whole face without overlap or a hole.
        shared.len() == 2 && (*shared[0] ^ *shared[1]).count_ones() == 2
    })
}
fn field_lipschitz_bound(samples: &[f32], resolution: u32, spacing: Vec3) -> f32 {
    let n = resolution as usize;
    let positive_threshold = spacing.length() * (0.5 + 1. / 128.);
    let mut maximum_squared = 1_f32;
    for z in 0..n - 1 {
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let values: [f32; 8] = std::array::from_fn(|i| {
                    samples[(x + (i & 1)) + n * ((y + ((i >> 1) & 1)) + n * (z + (i >> 2)))]
                });
                // A cell whose corner maximum is below the interpolation
                // allowance cannot yield a positive marching distance.
                if values.iter().all(|v| *v <= positive_threshold) {
                    continue;
                }
                for corner in 0..8 {
                    let dx = (values[(corner & 6) | 1] - values[corner & 6]) / spacing.x;
                    let dy = (values[(corner & 5) | 2] - values[corner & 5]) / spacing.y;
                    let dz = (values[(corner & 3) | 4] - values[corner & 3]) / spacing.z;
                    maximum_squared = maximum_squared.max(Vec3::new(dx, dy, dz).length_squared());
                }
            }
        }
    }
    // The trilinear gradient is a convex combination of its eight corner
    // gradients. This maximum bounds the whole positive field. The orthogonal
    // extension outside the grid additionally requires a bound of at least 1.
    maximum_squared.sqrt() * 1.0001
}
fn objects(scene: &Scene, previous: &[Mat4]) -> Vec<GpuObject> {
    scene
        .manifest
        .objects
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let model = o.transform.matrix();
            let mesh = &scene.manifest.meshes[scene.mesh_ids[&o.mesh]];
            let spacing =
                (mesh.sdf.bounds_max - mesh.sdf.bounds_min) / (mesh.sdf.resolution - 1) as f32;
            let scale = model.x_axis.truncate().length();
            GpuObject {
                model,
                previous: previous[i],
                inverse: model.inverse(),
                meta: [
                    scene.mesh_ids[&o.mesh] as u32,
                    scene.material_ids[&o.material] as u32,
                    scale.to_bits(),
                    (spacing.length() * scale).to_bits(),
                ],
            }
        })
        .collect()
}
fn hierarchy(scene: &Scene) -> Vec<Node> {
    let mut bounds = vec![];
    for (i, o) in scene.manifest.objects.iter().enumerate() {
        let mesh = &scene.manifest.meshes[scene.mesh_ids[&o.mesh]];
        let m = o.transform.matrix();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let p = Vec3::new(
                        if x == 0 {
                            mesh.sdf.bounds_min.x
                        } else {
                            mesh.sdf.bounds_max.x
                        },
                        if y == 0 {
                            mesh.sdf.bounds_min.y
                        } else {
                            mesh.sdf.bounds_max.y
                        },
                        if z == 0 {
                            mesh.sdf.bounds_min.z
                        } else {
                            mesh.sdf.bounds_max.z
                        },
                    );
                    let p = m.transform_point3(p);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        bounds.push((i as u32, lo, hi));
    }
    fn build(b: &mut [(u32, Vec3, Vec3)], nodes: &mut Vec<Node>) -> u32 {
        let index = nodes.len() as u32;
        nodes.push(Node::zeroed());
        let lo = b.iter().fold(Vec3::splat(f32::INFINITY), |a, x| a.min(x.1));
        let hi = b
            .iter()
            .fold(Vec3::splat(f32::NEG_INFINITY), |a, x| a.max(x.2));
        let meta = if b.len() == 1 {
            [b[0].0, 0, 1, 0]
        } else {
            let extent = hi - lo;
            let axis = if extent.x > extent.y && extent.x > extent.z {
                0
            } else if extent.y > extent.z {
                1
            } else {
                2
            };
            b.sort_by(|a, b| ((a.1 + a.2)[axis]).total_cmp(&((b.1 + b.2)[axis])));
            let n = b.len() / 2;
            let (a, b) = b.split_at_mut(n);
            [build(a, nodes), build(b, nodes), 0, 0]
        };
        let mut meta = meta;
        meta[3] = nodes.len() as u32;
        nodes[index as usize] = Node {
            lo: lo.extend(0.).to_array(),
            hi: hi.extend(0.).to_array(),
            meta,
        };
        index
    }
    let mut result = vec![];
    build(&mut bounds, &mut result);
    result
}
impl Renderer {
    #[cfg(test)]
    pub(crate) fn test_transport_resources(&self) {
        let radiance =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.lighting.texture, 8));
        assert!(
            radiance.iter().all(|v| v.is_finite() && *v >= 0.),
            "resolved user-scene HDR must be finite and nonnegative"
        );
        let current = ((self.frame_index - 1) % 2) as usize;
        let bytes = crate::gpu_tests::buffer(self, &self.vpls[current]);
        assert_eq!(bytes.len(), self.settings.vpl_count as usize * 48);
        let archive = crate::gpu_tests::buffer(self, &self.archived_vpls);
        let offset = ((self.frame_index - 1) % (self.settings.restir_max_history + 1)) as usize
            * bytes.len();
        assert_eq!(
            bytes,
            &archive[offset..offset + bytes.len()],
            "archive must retain the exact generated pool across ring wrap"
        );
        let values: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert!(values.iter().all(|x| x.is_finite()));
        assert!(
            values
                .as_chunks::<12>()
                .0
                .iter()
                .any(|v| v[3] > 0. && v[8..11].iter().any(|f| *f > 0.)),
            "ray queries must generate contributing terminal VPLs"
        );
        let bytes = crate::gpu_tests::buffer(self, &self.direct[2]);
        assert!(
            bytes
                .as_chunks::<32>()
                .0
                .iter()
                .any(|r| u32::from_le_bytes(r[8..12].try_into().unwrap())
                    > self.settings.restir_direct_candidates),
            "temporal/spatial reuse must accumulate represented candidates"
        );
    }
    #[cfg(test)]
    pub(crate) fn test_indirect_reuse(&self) {
        let indirect = crate::gpu_tests::buffer(self, &self.indirect[2]);
        assert!(
            indirect
                .as_chunks::<32>()
                .0
                .iter()
                .any(|r| u32::from_le_bytes(r[8..12].try_into().unwrap())
                    > self.settings.restir_vpl_candidates),
            "indirect temporal/spatial reuse must accumulate represented candidates"
        );
    }
    #[cfg(test)]
    pub(crate) fn test_vpl_records(&self) -> Vec<[f32; 12]> {
        let bytes =
            crate::gpu_tests::buffer(self, &self.vpls[((self.frame_index - 1) % 2) as usize]);
        bytes
            .as_chunks::<48>()
            .0
            .iter()
            .map(|v| {
                std::array::from_fn(|i| f32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap()))
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn test_center_base_color(&self) -> [f32; 3] {
        let values =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.color.texture, 8));
        let i = ((self.settings.render_height / 2 * self.settings.render_width
            + self.settings.render_width / 2)
            * 4) as usize;
        [values[i], values[i + 1], values[i + 2]]
    }
    #[cfg(test)]
    pub(crate) fn test_center_material_params(&self) -> [f32; 2] {
        let values =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.params.texture, 4));
        let i = ((self.settings.render_height / 2 * self.settings.render_width
            + self.settings.render_width / 2)
            * 2) as usize;
        [values[i], values[i + 1]]
    }
    #[cfg(test)]
    pub(crate) fn test_history_max_age(&self) -> f32 {
        let current = ((self.frame_index - 1) % 2) as usize;
        crate::gpu_tests::texture(self, &self.svgf.moments[current].texture, 16)
            .as_chunks::<16>()
            .0
            .iter()
            .map(|pixel| f32::from_le_bytes(pixel[8..12].try_into().unwrap()))
            .fold(0., f32::max)
    }
    #[cfg(test)]
    pub(crate) fn test_svgf_signal(
        &self,
        signal: &[[f32; 4]],
        material_edge: bool,
        disoccluded: bool,
    ) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let write_half = |target: &Target, values: &[f32], channels: u32| {
            let data: Vec<u16> = values
                .iter()
                .map(|v| half::f16::from_f32(*v).to_bits())
                .collect();
            self.queue.write_texture(
                target.texture.as_image_copy(),
                bytemuck::cast_slice(&data),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.settings.render_width * channels * 2),
                    rows_per_image: None,
                },
                target.texture.size(),
            );
        };
        write_half(&self.lighting, bytemuck::cast_slice(signal), 4);
        if material_edge {
            let colors: Vec<f32> = (0..signal.len())
                .flat_map(|i| {
                    if i as u32 % self.settings.render_width < self.settings.render_width / 2 {
                        [1., 0., 0., 1.]
                    } else {
                        [0., 1., 0., 1.]
                    }
                })
                .collect();
            write_half(&self.color, &colors, 4);
        }
        if disoccluded {
            write_half(&self.motion, &vec![2.; signal.len() * 2], 2);
        }
        let current = ((self.frame_index - 1) % 2) as usize;
        let mut e = self.device.create_command_encoder(&Default::default());
        self.svgf.encode(
            &self.device,
            &mut e,
            &self.common,
            &self.gbuffer_group,
            &self.lighting,
            &self.histories[current],
            &self.previous_depth,
            &self.previous_normal,
            current,
            self.settings.svgf_iterations,
            0,
        );
        self.queue.submit([e.finish()]);
        self.wait().unwrap();
        let rgba = crate::gpu_tests::floats(&crate::gpu_tests::texture(
            self,
            &self.histories[current].texture,
            8,
        ));
        let read_float = |target: &Target, bpp| {
            crate::gpu_tests::texture(self, &target.texture, bpp)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect()
        };
        (
            rgba,
            read_float(&self.svgf.moments[current], 16),
            read_float(&self.depth, 4),
        )
    }
    #[cfg(test)]
    pub(crate) fn test_history_mature_fraction(&self, minimum_age: f32) -> f32 {
        let current = ((self.frame_index - 1) % 2) as usize;
        let ages = crate::gpu_tests::texture(self, &self.svgf.moments[current].texture, 16);
        let mature = ages
            .as_chunks::<16>()
            .0
            .iter()
            .filter(|pixel| f32::from_le_bytes(pixel[8..12].try_into().unwrap()) >= minimum_age)
            .count();
        mature as f32 / (self.settings.render_width * self.settings.render_height) as f32
    }
    #[cfg(test)]
    pub(crate) fn test_reservoir_maturity(&self, indirect: bool) -> (u32, u32) {
        let source = if indirect {
            &self.indirect[2]
        } else {
            &self.direct[2]
        };
        crate::gpu_tests::buffer(self, source)
            .as_chunks::<32>()
            .0
            .iter()
            .fold((0, 0), |(max_count, max_age), record| {
                (
                    max_count.max(u32::from_le_bytes(record[8..12].try_into().unwrap())),
                    max_age.max(u32::from_le_bytes(record[12..16].try_into().unwrap())),
                )
            })
    }
    #[cfg(test)]
    pub(crate) fn test_motion_nonzero(&self) {
        let motion =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.motion.texture, 4));
        assert!(motion.iter().all(|x| x.is_finite()));
        assert!(motion.iter().any(|x| x.abs() > 0.001));
    }
    #[cfg(test)]
    pub(crate) fn test_texture_channels(&self) {
        let center = ((self.settings.render_height / 2) * self.settings.render_width
            + self.settings.render_width / 2) as usize;
        let params =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.params.texture, 4));
        assert!(
            (params[center * 2] - 0.5).abs() < 0.01,
            "roughness uses packed green channel: {}",
            params[center * 2]
        );
        assert!(
            (params[center * 2 + 1] - 0.125).abs() < 0.01,
            "metallic uses packed blue channel: {}",
            params[center * 2 + 1]
        );
        let color =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.color.texture, 8));
        for (i, v) in [128., 64., 191.].into_iter().enumerate() {
            let expected = ((v / 255. + 0.055) / 1.055_f32).powf(2.4);
            assert!(
                (color[center * 4 + i] - expected).abs() < 0.003,
                "base color channel {i} must decode sRGB into linear: {} vs {expected}",
                color[center * 4 + i]
            );
        }
        let normals =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.normal.texture, 8));
        assert!(
            normals[center * 4 + 2] > 0.99,
            "neutral tangent normal map must face +Z"
        );
    }
    #[cfg(test)]
    pub(crate) fn test_center_radiance(&self) -> f32 {
        let values =
            crate::gpu_tests::floats(&crate::gpu_tests::texture(self, &self.lighting.texture, 8));
        assert!(
            values.iter().all(|v| v.is_finite()),
            "resolved HDR must be finite"
        );
        values[((self.settings.render_height / 2 * self.settings.render_width
            + self.settings.render_width / 2)
            * 4) as usize]
    }
    pub async fn new(scene: &Scene, settings: Settings) -> Result<Self> {
        settings.validate()?;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let mut adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN).await;
        adapters.sort_by_key(|a| {
            if a.get_info().device_type == wgpu::DeviceType::DiscreteGpu {
                0
            } else {
                1
            }
        });
        let adapter=adapters.into_iter().find(|a|a.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)).context("Vulkan hardware ray queries unavailable; an RT-capable GPU and current Vulkan driver are required")?;
        let info = adapter.get_info();
        eprintln!(
            "GPU: {} ({:?}); {}x{}",
            info.name, info.backend, settings.render_width, settings.render_height
        );
        let available = adapter.features();
        let timestamps =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let filtered_fields = available.contains(wgpu::Features::FLOAT32_FILTERABLE);
        let features = wgpu::Features::EXPERIMENTAL_RAY_QUERY
            | if filtered_fields {
                wgpu::Features::FLOAT32_FILTERABLE
            } else {
                wgpu::Features::empty()
            }
            | if available.contains(timestamps) {
                timestamps
            } else {
                wgpu::Features::empty()
            };
        let limits = adapter.limits();
        let pixels = settings.render_width as u64 * settings.render_height as u64;
        ensure!(
            limits.max_storage_buffers_per_shader_stage >= 16,
            "adapter needs at least 16 storage buffers per compute stage"
        );
        ensure!(
            settings.render_width.max(settings.render_height) <= limits.max_texture_dimension_2d,
            "render size exceeds adapter texture limit"
        );
        ensure!(
            (settings.restir_max_history as u64 + 1) * settings.vpl_count as u64 * 48
                <= limits.max_storage_buffer_binding_size as u64,
            "VPL history budget exceeds adapter storage binding limit"
        );
        ensure!(
            pixels * 32 <= limits.max_storage_buffer_binding_size as u64,
            "render size exceeds adapter reservoir binding limit"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Instant Radiosity"),
                required_features: features,
                required_limits: limits,
                experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
                ..Default::default()
            })
            .await?;
        let mut all_vertices = vec![];
        let mut all_indices = vec![];
        let tile = scene
            .manifest
            .meshes
            .iter()
            .map(|m| m.sdf.resolution)
            .max()
            .unwrap_or(4);
        let count = scene.data.len() as u32;
        let mut gx = 1u32;
        while gx.pow(3) < count {
            gx += 1;
        }
        let mut gy = 1u32;
        while gx * gy * gy < count {
            gy += 1;
        }
        let gz = count.div_ceil(gx * gy);
        let extent = wgpu::Extent3d {
            width: tile * gx,
            height: tile * gy,
            depth_or_array_layers: tile * gz,
        };
        ensure!(
            extent
                .width
                .max(extent.height)
                .max(extent.depth_or_array_layers)
                <= device.limits().max_texture_dimension_3d,
            "SDF atlas exceeds adapter 3D texture limit"
        );
        let sdf_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("immutable float32 SDF atlas"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut mesh_meta = vec![];
        for (mesh, data) in scene.manifest.meshes.iter().zip(&scene.data) {
            let id = mesh_meta.len() as u32;
            let origin = wgpu::Origin3d {
                x: (id % gx) * tile,
                y: ((id / gx) % gy) * tile,
                z: (id / (gx * gy)) * tile,
            };
            let res = mesh.sdf.resolution;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &sdf_texture,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&data.sdf),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(res * 4),
                    rows_per_image: Some(res),
                },
                wgpu::Extent3d {
                    width: res,
                    height: res,
                    depth_or_array_layers: res,
                },
            );
            let spacing = (mesh.sdf.bounds_max - mesh.sdf.bounds_min) / (res - 1) as f32;
            let mut geometry_lo = Vec3::splat(f32::INFINITY);
            let mut geometry_hi = Vec3::splat(f32::NEG_INFINITY);
            for vertex in &data.vertices {
                let p = Vec3::from_slice(&vertex.position[..3]);
                geometry_lo = geometry_lo.min(p);
                geometry_hi = geometry_hi.max(p);
            }
            let sphere_center = (geometry_lo + geometry_hi) * 0.5;
            let sphere_radius = data
                .vertices
                .iter()
                .map(|v| Vec3::from_slice(&v.position[..3]).distance(sphere_center))
                .fold(0_f32, f32::max)
                * 1.000001
                + 0.000001;
            mesh_meta.push(GpuMesh {
                lo: mesh.sdf.bounds_min.extend(0.).to_array(),
                hi: mesh.sdf.bounds_max.extend(0.).to_array(),
                geometry: [
                    all_vertices.len() as u32,
                    data.vertices.len() as u32,
                    all_indices.len() as u32,
                    data.indices.len() as u32,
                ],
                field: [origin.x, origin.y, origin.z, res],
                grid: spacing.recip().extend(spacing.length()).to_array(),
                geometry_lo: geometry_lo
                    .extend(u32::from(is_box_geometry(
                        &data.vertices,
                        &data.indices,
                        geometry_lo,
                        geometry_hi,
                    )) as f32)
                    .to_array(),
                geometry_hi: geometry_hi
                    .extend(field_lipschitz_bound(&data.sdf, res, spacing).recip())
                    .to_array(),
                enclosing_sphere: sphere_center.extend(sphere_radius).to_array(),
            });
            all_vertices.extend_from_slice(&data.vertices);
            all_indices.extend_from_slice(&data.indices);
        }
        let vertex_buffer = buffer(
            &device,
            "vertices",
            &all_vertices,
            wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::BLAS_INPUT,
        );
        let index_buffer = buffer(
            &device,
            "indices",
            &all_indices,
            wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::BLAS_INPUT,
        );
        let mesh_buffer = buffer(
            &device,
            "mesh metadata",
            &mesh_meta,
            wgpu::BufferUsages::STORAGE,
        );
        let sdf_view = sdf_texture.create_view(&Default::default());
        let sdf_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SDF interpolation"),
            mag_filter: if filtered_fields {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            },
            min_filter: if filtered_fields {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            },
            ..Default::default()
        });
        let previous_models = scene
            .manifest
            .objects
            .iter()
            .map(|o| o.transform.matrix())
            .collect::<Vec<_>>();
        let object_buffer = buffer(
            &device,
            "objects",
            &objects(scene, &previous_models),
            wgpu::BufferUsages::STORAGE,
        );
        let node_buffer = buffer(
            &device,
            "SDF hierarchy",
            &hierarchy(scene),
            wgpu::BufferUsages::STORAGE,
        );
        let uniform = buffer(
            &device,
            "frame",
            &[Frame::zeroed()],
            wgpu::BufferUsages::UNIFORM,
        );
        let mut material_data = vec![];
        let mut images = vec![];
        for mat in &scene.manifest.materials {
            let mut layers = [-1; 4];
            for (i, path) in [
                &mat.base_color_texture,
                &mat.roughness_texture,
                &mat.metallic_texture,
                &mat.normal_texture,
            ]
            .iter()
            .enumerate()
            {
                if let Some(path) = path {
                    layers[i] = images.len() as i32;
                    let image = image::open(scene.root.join(path))
                        .with_context(|| format!("material {}: texture {path}", mat.id))?
                        .to_rgba8();
                    images.push((image, i == 0 && mat.base_color_srgb));
                }
            }
            material_data.push(GpuMaterial {
                color: mat.base_color,
                params: [
                    mat.roughness,
                    mat.metallic,
                    mat.normal_strength,
                    if mat.base_color_srgb { 1. } else { 0. },
                ],
                textures: layers,
                channels: mat.texture_channels,
            });
        }
        let texture_size = images
            .iter()
            .map(|(i, _)| i.width().max(i.height()))
            .max()
            .unwrap_or(1)
            .min(2048);
        ensure!(
            images.len().max(1) <= device.limits().max_texture_array_layers as usize,
            "material textures exceed adapter array layer limit"
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material textures"),
            size: wgpu::Extent3d {
                width: texture_size,
                height: texture_size,
                depth_or_array_layers: images.len().max(1) as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        for (i, (img, srgb)) in images.iter().enumerate() {
            let resized = resize_material_image(img, texture_size, *srgb);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                resized.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(texture_size * 4),
                    rows_per_image: Some(texture_size),
                },
                wgpu::Extent3d {
                    width: texture_size,
                    height: texture_size,
                    depth_or_array_layers: 1,
                },
            );
        }
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let srgb_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("repeat linear"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let material_buffer = buffer(
            &device,
            "materials",
            &material_data,
            wgpu::BufferUsages::STORAGE,
        );
        let light_buffer = storage(
            &device,
            "lights",
            scene.manifest.lights.len().max(1) as u64 * 48,
        );
        let sizes = scene
            .data
            .iter()
            .map(|m| wgpu::BlasTriangleGeometrySizeDescriptor {
                vertex_format: wgpu::VertexFormat::Float32x3,
                vertex_count: m.vertices.len() as u32,
                index_format: Some(wgpu::IndexFormat::Uint32),
                index_count: Some(m.indices.len() as u32),
                flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
            })
            .collect::<Vec<_>>();
        let blas = sizes
            .iter()
            .map(|s| {
                device.create_blas(
                    &wgpu::CreateBlasDescriptor {
                        label: Some("static mesh BLAS"),
                        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                    },
                    wgpu::BlasGeometrySizeDescriptors::Triangles {
                        descriptors: vec![s.clone()],
                    },
                )
            })
            .collect::<Vec<_>>();
        let mut encoder = device.create_command_encoder(&Default::default());
        let entries = blas
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BlasBuildEntry {
                blas: b,
                geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                    wgpu::BlasTriangleGeometry {
                        size: &sizes[i],
                        vertex_buffer: &vertex_buffer,
                        first_vertex: mesh_meta[i].geometry[0],
                        vertex_stride: 64,
                        index_buffer: Some(&index_buffer),
                        first_index: Some(mesh_meta[i].geometry[2]),
                        transform_buffer: None,
                        transform_buffer_offset: None,
                    },
                ]),
            })
            .collect::<Vec<_>>();
        encoder.build_acceleration_structures(entries.iter(), std::iter::empty());
        queue.submit([encoder.finish()]);
        let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("dynamic TLAS"),
            max_instances: scene.manifest.objects.len() as u32,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        let all = wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE;
        let compute = wgpu::ShaderStages::COMPUTE;
        let mut layout_entries = vec![entry(
            0,
            all,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        )];
        for b in 1..=8 {
            if b != 7 {
                layout_entries.push(entry(b, all, sb(true)));
            }
        }
        layout_entries.push(entry(
            7,
            all,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float {
                    filterable: filtered_fields,
                },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
        ));
        layout_entries.push(entry(
            12,
            all,
            wgpu::BindingType::Sampler(if filtered_fields {
                wgpu::SamplerBindingType::Filtering
            } else {
                wgpu::SamplerBindingType::NonFiltering
            }),
        ));
        layout_entries.push(entry(
            9,
            all,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
        ));
        layout_entries.push(entry(
            13,
            all,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
        ));
        layout_entries.push(entry(
            10,
            all,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        ));
        layout_entries.push(entry(
            11,
            compute,
            wgpu::BindingType::AccelerationStructure {
                vertex_return: false,
            },
        ));
        let common_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene layout"),
            entries: &layout_entries,
        });
        let common = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &common_layout,
            entries: &[
                be(0, uniform.as_entire_binding()),
                be(1, object_buffer.as_entire_binding()),
                be(2, mesh_buffer.as_entire_binding()),
                be(3, material_buffer.as_entire_binding()),
                be(4, light_buffer.as_entire_binding()),
                be(5, vertex_buffer.as_entire_binding()),
                be(6, index_buffer.as_entire_binding()),
                be(7, wgpu::BindingResource::TextureView(&sdf_view)),
                be(8, node_buffer.as_entire_binding()),
                be(9, wgpu::BindingResource::TextureView(&texture_view)),
                be(10, wgpu::BindingResource::Sampler(&sampler)),
                be(11, tlas.as_binding()),
                be(12, wgpu::BindingResource::Sampler(&sdf_sampler)),
                be(13, wgpu::BindingResource::TextureView(&srgb_view)),
            ],
        });
        let w = settings.render_width;
        let h = settings.render_height;
        let depth = Target::new(
            &device,
            "depth",
            w,
            h,
            wgpu::TextureFormat::Depth32Float,
            false,
        );
        let normal = Target::new(
            &device,
            "normal",
            w,
            h,
            wgpu::TextureFormat::Rgba16Float,
            false,
        );
        let color = Target::new(
            &device,
            "base color",
            w,
            h,
            wgpu::TextureFormat::Rgba16Float,
            false,
        );
        let params = Target::new(
            &device,
            "roughness metallic",
            w,
            h,
            wgpu::TextureFormat::Rg16Float,
            false,
        );
        let motion = Target::new(
            &device,
            "motion",
            w,
            h,
            wgpu::TextureFormat::Rg16Float,
            false,
        );
        let previous_depth = Target::new(
            &device,
            "depth history",
            w,
            h,
            wgpu::TextureFormat::Depth32Float,
            false,
        );
        let previous_normal = Target::new(
            &device,
            "normal history",
            w,
            h,
            wgpu::TextureFormat::Rgba16Float,
            false,
        );
        let lighting = Target::new(
            &device,
            "resolved noisy HDR",
            w,
            h,
            wgpu::TextureFormat::Rgba16Float,
            true,
        );
        // Resolve the two visibility domains in separate kernels. Preserve
        // float32 direct lighting until it is added to indirect lighting, so
        // splitting the kernels does not introduce an intermediate HDR rounding.
        let direct_lighting = Target::new(
            &device,
            "direct HDR scratch",
            w,
            h,
            wgpu::TextureFormat::Rgba32Float,
            true,
        );
        let unused_lighting_output = Target::new(
            &device,
            "inactive direct HDR output",
            1,
            1,
            wgpu::TextureFormat::Rgba32Float,
            true,
        );
        let histories = std::array::from_fn(|_| {
            Target::new(
                &device,
                "temporal HDR",
                w,
                h,
                wgpu::TextureFormat::Rgba16Float,
                true,
            )
        });
        let display = Target::new(
            &device,
            "capture display",
            w,
            h,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            false,
        );
        let gl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("G-buffer layout"),
            entries: &(0..5)
                .map(|b| entry(b, compute, tex(b == 0)))
                .collect::<Vec<_>>(),
        });
        let gbuffer_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("G-buffer"),
            layout: &gl,
            entries: &[
                be(0, wgpu::BindingResource::TextureView(&depth.view)),
                be(1, wgpu::BindingResource::TextureView(&normal.view)),
                be(2, wgpu::BindingResource::TextureView(&color.view)),
                be(3, wgpu::BindingResource::TextureView(&params.view)),
                be(4, wgpu::BindingResource::TextureView(&motion.view)),
            ],
        });
        let direct = std::array::from_fn(|_| storage(&device, "direct reservoir", pixels * 32));
        let indirect = std::array::from_fn(|_| storage(&device, "indirect reservoir", pixels * 32));
        let vpls =
            std::array::from_fn(|_| storage(&device, "VPL pool", settings.vpl_count as u64 * 48));
        let archived_vpls = storage(
            &device,
            "VPL sample archive",
            (settings.restir_max_history as u64 + 1) * settings.vpl_count as u64 * 48,
        );
        let vpl_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VPL output layout"),
            entries: &[entry(0, compute, sb(false))],
        });
        let vpl_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("VPL output"),
                layout: &vpl_layout,
                entries: &[be(0, vpls[i].as_entire_binding())],
            })
        });
        let raster_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("raster"),
            bind_group_layouts: &[Some(&common_layout)],
            immediate_size: 0,
        });
        let gshader = shader(&device, "G-buffer", shader_source("gbuffer"));
        let attrs = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4];
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: 64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attrs,
        };
        let targets = [
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureFormat::Rg16Float,
            wgpu::TextureFormat::Rg16Float,
        ]
        .map(|format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        });
        let raster = |depth_only: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if depth_only {
                    "depth prepass"
                } else {
                    "G-buffer"
                }),
                layout: Some(&raster_layout),
                vertex: wgpu::VertexState {
                    module: &gshader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(depth_only),
                    depth_compare: Some(if depth_only {
                        wgpu::CompareFunction::Less
                    } else {
                        wgpu::CompareFunction::Equal
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: if depth_only {
                    None
                } else {
                    Some(wgpu::FragmentState {
                        module: &gshader,
                        entry_point: Some("fs"),
                        compilation_options: Default::default(),
                        targets: &targets,
                    })
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let depth_pipeline = raster(true);
        let gbuffer_pipeline = raster(false);
        let vp_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VPL"),
            bind_group_layouts: &[Some(&common_layout), Some(&vpl_layout)],
            immediate_size: 0,
        });
        let vshader = shader(&device, "VPL generation", shader_source("vpl"));
        let vpl_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("VPL generation"),
            layout: Some(&vp_layout),
            module: &vshader,
            entry_point: Some("generate"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut pe = vec![];
        for b in [0, 1, 4, 5, 12, 13, 14] {
            pe.push(entry(b, compute, sb(true)));
        }
        for b in [2, 3] {
            pe.push(entry(b, compute, sb(false)));
        }
        for b in [6, 7, 8, 10, 16] {
            pe.push(entry(b, compute, tex(b == 6)));
        }
        pe.push(entry(
            9,
            compute,
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
        ));
        pe.push(entry(
            15,
            compute,
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba32Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
        ));
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lighting resources"),
            entries: &pe,
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lighting"),
            bind_group_layouts: &[Some(&common_layout), Some(&gl), Some(&pass_layout)],
            immediate_size: 0,
        });
        let lshader = shader(
            &device,
            "ReSTIR / selected visibility",
            if filtered_fields {
                filtered_shader_source("lighting")
            } else {
                shader_source("lighting")
            },
        );
        let mut pipelines = HashMap::new();
        for name in [
            "candidates_direct",
            "candidates_indirect",
            "temporal_direct",
            "temporal_indirect",
            "spatial_direct",
            "spatial_indirect",
            "resolve_direct",
            "resolve_indirect",
        ] {
            pipelines.insert(
                name,
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(name),
                    layout: Some(&pl),
                    module: &lshader,
                    entry_point: Some(name),
                    compilation_options: Default::default(),
                    cache: None,
                }),
            );
        }
        let svgf = crate::svgf::Svgf::new(&device, &common_layout, &gl, w, h);
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("presentation"),
            entries: &[entry(0, wgpu::ShaderStages::FRAGMENT, tex(false))],
        });
        let present_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("HDR presentation"),
                layout: &present_layout,
                entries: &[be(
                    0,
                    wgpu::BindingResource::TextureView(&histories[i].view),
                )],
            })
        });
        let present_pipeline = Self::presentation_pipeline(
            &device,
            &present_layout,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        );
        let timestamp_period = queue.get_timestamp_period();
        let query = features.contains(timestamps).then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("frame timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 15,
            })
        });
        let query_resolve = query.as_ref().map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp resolve"),
                size: 120,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let query_read = query.as_ref().map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp read"),
                size: 120,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        let previous_vp = scene.manifest.camera.matrix(w as f32 / h as f32);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            settings,
            common,
            uniform,
            object_buffer,
            material_buffer,
            light_buffer,
            node_buffer,
            vertex_buffer,
            index_buffer,
            mesh_meta,
            blas,
            tlas,
            depth,
            normal,
            color,
            params,
            motion,
            previous_depth,
            previous_normal,
            lighting,
            direct_lighting,
            unused_lighting_output,
            histories,
            svgf,
            display,
            gbuffer_group,
            direct,
            indirect,
            vpls,
            archived_vpls,
            depth_pipeline,
            gbuffer_pipeline,
            vpl_pipeline,
            vpl_groups,
            pipelines,
            pass_layout,
            present_pipeline,
            present_groups,
            present_layout,
            previous_models,
            previous_vp,
            frame_index: 0,
            previous_generation: 0,
            previous_debug: 0,
            debug_mode: 0,
            last_gpu_ms: None,
            last_pass_ms: HashMap::new(),
            query,
            query_resolve,
            query_read,
            timestamp_period,
        })
    }
    #[cfg(test)]
    pub(crate) fn test_reservoir_mature_fraction(&self, indirect: bool, minimum_age: u32) -> f32 {
        let source = if indirect {
            &self.indirect[2]
        } else {
            &self.direct[2]
        };
        let records = crate::gpu_tests::buffer(self, source);
        let reservoirs = records.as_chunks::<32>().0;
        let mature = reservoirs
            .iter()
            .filter(|record| u32::from_le_bytes(record[12..16].try_into().unwrap()) >= minimum_age)
            .count();
        mature as f32 / reservoirs.len() as f32
    }
    fn pass_group(
        &self,
        input: usize,
        output: usize,
        current: usize,
        stage: &str,
    ) -> wgpu::BindGroup {
        let old = 1 - current;
        let history = if output == 2 { 0 } else { 2 };
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lighting pass"),
            layout: &self.pass_layout,
            entries: &[
                be(0, self.direct[input].as_entire_binding()),
                be(1, self.indirect[input].as_entire_binding()),
                be(2, self.direct[output].as_entire_binding()),
                be(3, self.indirect[output].as_entire_binding()),
                be(4, self.vpls[current].as_entire_binding()),
                be(5, self.vpls[old].as_entire_binding()),
                be(
                    6,
                    wgpu::BindingResource::TextureView(&self.previous_depth.view),
                ),
                be(
                    7,
                    wgpu::BindingResource::TextureView(&self.previous_normal.view),
                ),
                be(
                    8,
                    wgpu::BindingResource::TextureView(&self.histories[old].view),
                ),
                be(9, wgpu::BindingResource::TextureView(&self.lighting.view)),
                be(
                    10,
                    wgpu::BindingResource::TextureView(&self.histories[old].view),
                ),
                be(12, self.direct[history].as_entire_binding()),
                be(13, self.indirect[history].as_entire_binding()),
                be(14, self.archived_vpls.as_entire_binding()),
                be(
                    15,
                    wgpu::BindingResource::TextureView(if stage == "resolve_direct" {
                        &self.direct_lighting.view
                    } else {
                        &self.unused_lighting_output.view
                    }),
                ),
                be(
                    16,
                    wgpu::BindingResource::TextureView(if stage == "resolve_indirect" {
                        &self.direct_lighting.view
                    } else {
                        &self.histories[old].view
                    }),
                ),
            ],
        })
    }
    pub fn encode(
        &mut self,
        scene: &Scene,
        encoder: &mut wgpu::CommandEncoder,
        profile: bool,
    ) -> Result<()> {
        let current = (self.frame_index % 2) as usize;
        let s = &self.settings;
        let vp = scene
            .manifest
            .camera
            .matrix(s.render_width as f32 / s.render_height as f32);
        let f = Frame {
            vp,
            inv_vp: vp.inverse(),
            previous_vp: self.previous_vp,
            previous_inv_vp: self.previous_vp.inverse(),
            camera: scene.manifest.camera.position.extend(0.).to_array(),
            size: [
                s.render_width,
                s.render_height,
                self.frame_index,
                scene.generation,
            ],
            counts: [
                scene.manifest.lights.len() as u32,
                s.vpl_count,
                s.vpl_max_bounces,
                scene.manifest.objects.len() as u32,
            ],
            restir: [
                s.restir_direct_candidates,
                s.restir_vpl_candidates,
                s.restir_spatial_neighbors,
                s.restir_spatial_radius,
            ],
            temporal: [
                s.temporal_history_weight,
                s.depth_threshold,
                s.normal_threshold,
                s.temporal_max_history as f32,
            ],
            sdf: [
                s.sdf_max_steps as f32,
                s.sdf_hit_epsilon,
                s.sdf_soft_shadow_k,
                0.,
            ],
            rr: [
                s.vpl_rr_min_survival,
                s.vpl_rr_max_survival,
                s.restir_max_history as f32,
                0.,
            ],
            flags: [
                u32::from(self.frame_index > 0 && self.previous_debug == self.debug_mode),
                self.previous_generation,
                self.debug_mode,
                0,
            ],
        };
        self.queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(&f));
        self.queue.write_buffer(
            &self.object_buffer,
            0,
            bytemuck::cast_slice(&objects(scene, &self.previous_models)),
        );
        let lights = scene
            .manifest
            .lights
            .iter()
            .map(|l| GpuLight {
                position_strength: l.position.extend(l.strength).to_array(),
                direction_angle: l.direction.extend(l.angle_radians).to_array(),
                color_type: l
                    .color
                    .extend(if l.kind == "spot" { 1. } else { 0. })
                    .to_array(),
            })
            .collect::<Vec<_>>();
        if !lights.is_empty() {
            self.queue
                .write_buffer(&self.light_buffer, 0, bytemuck::cast_slice(&lights));
        }
        // Texture handles stay immutable; edit only numeric scattering values.
        for (i, m) in scene.manifest.materials.iter().enumerate() {
            self.queue.write_buffer(
                &self.material_buffer,
                i as u64 * 64,
                bytemuck::cast_slice(&m.base_color),
            );
            self.queue.write_buffer(
                &self.material_buffer,
                i as u64 * 64 + 16,
                bytemuck::cast_slice(&[
                    m.roughness,
                    m.metallic,
                    m.normal_strength,
                    if m.base_color_srgb { 1. } else { 0. },
                ]),
            );
        }
        self.queue.write_buffer(
            &self.node_buffer,
            0,
            bytemuck::cast_slice(&hierarchy(scene)),
        );
        if profile && let Some(q) = &self.query {
            encoder.write_timestamp(q, 0);
        }
        for (i, o) in scene.manifest.objects.iter().enumerate() {
            let matrix = o.transform.matrix().transpose().to_cols_array();
            self.tlas[i] = Some(wgpu::TlasInstance::new(
                &self.blas[scene.mesh_ids[&o.mesh]],
                matrix[..12].try_into().unwrap(),
                i as u32,
                255,
            ));
        }
        encoder.build_acceleration_structures(std::iter::empty(), [&self.tlas]);
        let mut stamp = 1u32;
        if profile && let Some(q) = &self.query {
            encoder.write_timestamp(q, stamp);
        }
        stamp += 1;
        for depth_only in [true, false] {
            let clear = wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            };
            let attachments = [
                &self.normal.view,
                &self.color.view,
                &self.params.view,
                &self.motion.view,
            ]
            .map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: clear,
                })
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if depth_only {
                    "depth prepass"
                } else {
                    "G-buffer"
                }),
                color_attachments: if depth_only { &[] } else { &attachments },
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: if depth_only {
                            wgpu::LoadOp::Clear(1.)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(if depth_only {
                &self.depth_pipeline
            } else {
                &self.gbuffer_pipeline
            });
            pass.set_bind_group(0, &self.common, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (i, o) in scene.manifest.objects.iter().enumerate() {
                let m = self.mesh_meta[scene.mesh_ids[&o.mesh]];
                pass.draw_indexed(
                    m.geometry[2]..m.geometry[2] + m.geometry[3],
                    m.geometry[0] as i32,
                    i as u32..i as u32 + 1,
                );
            }
            drop(pass);
            if profile && let Some(q) = &self.query {
                encoder.write_timestamp(q, stamp);
            }
            stamp += 1;
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("VPL hardware ray queries"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.vpl_pipeline);
            pass.set_bind_group(0, &self.common, &[]);
            pass.set_bind_group(1, &self.vpl_groups[current], &[]);
            pass.dispatch_workgroups(s.vpl_count.div_ceil(64), 1, 1);
        }
        // Reservoirs keep a frame stamp and VPL index. Retain every referenced
        // pool for the finite history window rather than duplicating its 48-byte
        // sample inside each pixel. Reject expired stamps before ring overwrite.
        let pool_bytes = s.vpl_count as u64 * 48;
        let slot = self.frame_index % (s.restir_max_history + 1);
        encoder.copy_buffer_to_buffer(
            &self.vpls[current],
            0,
            &self.archived_vpls,
            slot as u64 * pool_bytes,
            pool_bytes,
        );
        if profile && let Some(q) = &self.query {
            encoder.write_timestamp(q, stamp);
        }
        stamp += 1;
        for (names, input, output) in [
            (["candidates_direct", "candidates_indirect"], 2, 0),
            (["temporal_direct", "temporal_indirect"], 0, 1),
            (["spatial_direct", "spatial_indirect"], 1, 2),
        ] {
            let group = self.pass_group(input, output, current, "reservoir");
            for name in names {
                let mut p = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(name),
                    timestamp_writes: None,
                });
                p.set_pipeline(&self.pipelines[name]);
                p.set_bind_group(0, &self.common, &[]);
                p.set_bind_group(1, &self.gbuffer_group, &[]);
                p.set_bind_group(2, &group, &[]);
                p.dispatch_workgroups(s.render_width.div_ceil(8), s.render_height.div_ceil(8), 1);
                drop(p);
                if profile && let Some(q) = &self.query {
                    encoder.write_timestamp(q, stamp);
                }
                stamp += 1;
            }
        }
        for name in ["resolve_direct", "resolve_indirect"] {
            let group = self.pass_group(2, 0, current, name);
            let mut p = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(name),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.pipelines[name]);
            p.set_bind_group(0, &self.common, &[]);
            p.set_bind_group(1, &self.gbuffer_group, &[]);
            p.set_bind_group(2, &group, &[]);
            p.dispatch_workgroups(s.render_width.div_ceil(8), s.render_height.div_ceil(8), 1);
            drop(p);
            if profile && let Some(q) = &self.query {
                encoder.write_timestamp(q, stamp);
            }
            stamp += 1;
        }
        self.svgf.encode(
            &self.device,
            encoder,
            &self.common,
            &self.gbuffer_group,
            &self.lighting,
            &self.histories[current],
            &self.previous_depth,
            &self.previous_normal,
            current,
            s.svgf_iterations,
            self.debug_mode,
        );
        if profile && let Some(q) = &self.query {
            encoder.write_timestamp(q, stamp);
        }
        stamp += 1;
        let extent = wgpu::Extent3d {
            width: s.render_width,
            height: s.render_height,
            depth_or_array_layers: 1,
        };
        encoder.copy_texture_to_texture(
            self.depth.texture.as_image_copy(),
            self.previous_depth.texture.as_image_copy(),
            extent,
        );
        encoder.copy_texture_to_texture(
            self.normal.texture.as_image_copy(),
            self.previous_normal.texture.as_image_copy(),
            extent,
        );
        self.present(encoder, &self.display.view, &self.present_pipeline, current);
        if profile && let Some(q) = &self.query {
            encoder.write_timestamp(q, stamp);
            encoder.resolve_query_set(q, 0..15, self.query_resolve.as_ref().unwrap(), 0);
            encoder.copy_buffer_to_buffer(
                self.query_resolve.as_ref().unwrap(),
                0,
                self.query_read.as_ref().unwrap(),
                0,
                120,
            );
        }
        self.previous_vp = vp;
        self.previous_generation = scene.generation;
        self.previous_debug = self.debug_mode;
        self.previous_models = scene
            .manifest
            .objects
            .iter()
            .map(|o| o.transform.matrix())
            .collect();
        self.frame_index += 1;
        Ok(())
    }
    fn presentation_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let m = shader(device, "presentation", shader_source("present"));
        let l = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("presentation"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("presentation"),
            layout: Some(&l),
            vertex: wgpu::VertexState {
                module: &m,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &m,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
    fn present(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        current: usize,
    ) {
        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("presentation"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        p.set_pipeline(pipeline);
        p.set_bind_group(0, &self.present_groups[current], &[]);
        p.draw(0..3, 0..1);
    }
    pub fn surface_pipeline(&self, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        Self::presentation_pipeline(&self.device, &self.present_layout, format)
    }
    pub fn present_surface(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
    ) {
        self.present(
            encoder,
            view,
            pipeline,
            ((self.frame_index - 1) % 2) as usize,
        );
    }
    pub fn wait(&self) -> Result<()> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }
    pub fn read_gpu_time(&mut self) -> Result<Option<f64>> {
        if let Some(b) = &self.query_read {
            let (tx, rx) = std::sync::mpsc::channel();
            b.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.wait()?;
            rx.recv()??;
            let bytes = b.slice(..).get_mapped_range()?;
            let values: &[u64] = bytemuck::cast_slice(&bytes);
            let ms = (values[14] - values[0]) as f64 * self.timestamp_period as f64 / 1e6;
            for (i, name) in [
                "tlas",
                "depth",
                "gbuffer",
                "vpl",
                "candidates_direct",
                "candidates_indirect",
                "temporal_direct",
                "temporal_indirect",
                "spatial_direct",
                "spatial_indirect",
                "visibility_direct",
                "visibility_indirect",
                "svgf",
                "history_present",
            ]
            .iter()
            .enumerate()
            {
                self.last_pass_ms.insert(
                    name,
                    (values[i + 1] - values[i]) as f64 * self.timestamp_period as f64 / 1e6,
                );
            }
            drop(bytes);
            b.unmap();
            self.last_gpu_ms = Some(ms);
            Ok(Some(ms))
        } else {
            Ok(None)
        }
    }
    pub fn capture(&self, path: &Path) -> Result<()> {
        self.capture_texture(&self.display.texture, path, false)
    }
    pub fn capture_texture(&self, texture: &wgpu::Texture, path: &Path, bgra: bool) -> Result<()> {
        let w = texture.width();
        let h = texture.height();
        let stride = (w * 4).div_ceil(256) * 256;
        let b = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PNG capture"),
            size: stride as u64 * h as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = self.device.create_command_encoder(&Default::default());
        e.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &b,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([e.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        b.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.wait()?;
        rx.recv()??;
        let mapped = b.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in mapped.chunks_exact(stride as usize) {
            pixels.extend_from_slice(&row[..w as usize * 4]);
        }
        drop(mapped);
        b.unmap();
        if bgra {
            for p in pixels.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        image::save_buffer(path, &pixels, w, h, image::ColorType::Rgba8)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trilinear_gradient_bound_covers_simultaneous_axis_slopes() {
        let spacing = Vec3::new(0.25, 0.5, 1.);
        let samples = std::array::from_fn::<_, 8, _>(|i| {
            let p = Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, (i >> 2) as f32) * spacing;
            10. + p.dot(Vec3::new(1., 2., 3.))
        });
        let bound = field_lipschitz_bound(&samples, 2, spacing);
        assert!(bound >= 14_f32.sqrt());
        assert!(bound < 14_f32.sqrt() * 1.001);
        // Flat and wholly negative cells still need unit slope outside the grid.
        assert!(field_lipschitz_bound(&[0.; 8], 2, spacing) >= 1.);
        assert!(field_lipschitz_bound(&[-100.; 8], 2, spacing) >= 1.);
        let n = 16usize;
        let sphere: Vec<_> = (0..n * n * n)
            .map(|i| {
                let p = Vec3::new((i % n) as f32, ((i / n) % n) as f32, (i / (n * n)) as f32) * 0.2
                    - Vec3::splat(1.5);
                p.length() - 1.
            })
            .collect();
        let sphere_bound = field_lipschitz_bound(&sphere, n as u32, Vec3::splat(0.2));
        assert!(
            (1. ..1.2).contains(&sphere_bound),
            "curved sphere field: {sphere_bound}"
        );
    }
    #[test]
    fn box_certification_rejects_holes_overlap_and_other_geometry() {
        let lo = Vec3::splat(-1.);
        let hi = Vec3::ONE;
        let mut vertices: Vec<crate::scene::Vertex> = (0..8)
            .map(|i| {
                let mut v = crate::scene::Vertex::zeroed();
                v.position = Vec3::new(
                    if i & 1 == 0 { -1. } else { 1. },
                    if i & 2 == 0 { -1. } else { 1. },
                    if i & 4 == 0 { -1. } else { 1. },
                )
                .extend(1.)
                .to_array();
                v
            })
            .collect();
        let indices = [
            0, 2, 6, 0, 6, 4, 1, 5, 7, 1, 7, 3, 0, 4, 5, 0, 5, 1, 2, 3, 7, 2, 7, 6, 0, 1, 3, 0, 3,
            2, 4, 6, 7, 4, 7, 5,
        ];
        assert!(is_box_geometry(&vertices, &indices, lo, hi));
        assert!(!is_box_geometry(&vertices, &indices[..33], lo, hi));
        let mut overlap = indices;
        overlap[3..6].copy_from_slice(&indices[..3]);
        assert!(!is_box_geometry(&vertices, &overlap, lo, hi));
        let mut hole = indices;
        hole[3..6].copy_from_slice(&[0, 2, 4]);
        assert!(!is_box_geometry(&vertices, &hole, lo, hi));
        let mut nonface = indices;
        nonface[..3].copy_from_slice(&[0, 3, 7]);
        assert!(!is_box_geometry(&vertices, &nonface, lo, hi));
        vertices[0].position[0] += 0.001;
        assert!(!is_box_geometry(&vertices, &indices, lo, hi));
    }
    #[test]
    fn color_texture_resampling_is_linear() {
        let image = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let color = resize_material_image(&image, 1, true);
        let data = resize_material_image(&image, 1, false);
        assert_eq!(
            color.get_pixel(0, 0).0,
            [188, 188, 188, 255],
            "half linear light must encode near sRGB 0.735"
        );
        assert_eq!(
            data.get_pixel(0, 0).0,
            [128, 128, 128, 255],
            "non-color data must interpolate directly"
        );
    }
    #[test]
    fn gpu_layout_sizes_match_wgsl() {
        assert_eq!(size_of::<crate::scene::Vertex>(), 64);
        assert_eq!(size_of::<GpuMesh>(), 128);
        assert_eq!(size_of::<GpuObject>(), 208);
        assert_eq!(size_of::<GpuLight>(), 48);
        assert_eq!(size_of::<GpuMaterial>(), 64);
        assert_eq!(size_of::<Frame>(), 384);
    }
    #[test]
    fn all_shaders_parse_and_validate() {
        for stage in [
            "gbuffer",
            "vpl",
            "lighting",
            "present",
            "lighting_filtered",
            "svgf",
        ] {
            let s = if stage == "lighting_filtered" {
                filtered_shader_source("lighting")
            } else {
                shader_source(stage)
            };
            let m = naga::front::wgsl::parse_str(&s)
                .unwrap_or_else(|e| panic!("{stage}: {}", e.emit_to_string(&s)));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&m)
            .unwrap_or_else(|e| panic!("{stage}: {}", e.emit_to_string(&s)));
            for e in &m.entry_points {
                eprintln!("Compiling {stage}/{}", e.name);
                naga::back::spv::write_vec(
                    &m,
                    &info,
                    &Default::default(),
                    Some(&naga::back::spv::PipelineOptions {
                        shader_stage: e.stage,
                        entry_point: e.name.clone(),
                    }),
                )
                .unwrap();
            }
        }
    }
}
