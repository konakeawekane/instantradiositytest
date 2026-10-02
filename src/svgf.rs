//! SVGF reconstruction with independent moments and first-wavelet feedback.
use crate::renderer::Target;
use wgpu::util::DeviceExt;

pub struct Svgf {
    pub moments: [Target; 2],
    feedback: [Target; 2],
    temporal: Target,
    scratch: [Target; 2],
    dummy: Target,
    steps: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 3],
}
impl Svgf {
    pub fn new(
        d: &wgpu::Device,
        common: &wgpu::BindGroupLayout,
        gbuffer: &wgpu::BindGroupLayout,
        w: u32,
        h: u32,
    ) -> Self {
        let rgba = |label| Target::new(d, label, w, h, wgpu::TextureFormat::Rgba16Float, true);
        let moments = std::array::from_fn(|_| {
            Target::new(
                d,
                "SVGF moments/history length",
                w,
                h,
                wgpu::TextureFormat::Rgba32Float,
                true,
            )
        });
        let mut entries: Vec<_> = (0..5)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        for (binding, format) in [
            (5, wgpu::TextureFormat::Rgba16Float),
            (6, wgpu::TextureFormat::Rgba32Float),
        ] {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            });
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        for binding in [8, 9] {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: if binding == 8 {
                        wgpu::TextureSampleType::Depth
                    } else {
                        wgpu::TextureSampleType::Float { filterable: false }
                    },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SVGF resources"),
            entries: &entries,
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SVGF"),
            bind_group_layouts: &[Some(common), Some(gbuffer), Some(&layout)],
            immediate_size: 0,
        });
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SVGF"),
            source: wgpu::ShaderSource::Wgsl(crate::renderer::shader_source("svgf").into()),
        });
        let pipelines = ["svgf_temporal", "svgf_variance", "svgf_atrous"].map(|name| {
            d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module: &shader,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let alignment = d.limits().min_uniform_buffer_offset_alignment as usize;
        let mut step_bytes = vec![0u8; alignment * 5];
        for i in 0..5 {
            step_bytes[i * alignment..i * alignment + 4]
                .copy_from_slice(&(1u32 << i).to_le_bytes());
        }
        let steps = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SVGF wavelet strides"),
            contents: &step_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self {
            moments,
            feedback: std::array::from_fn(|_| rgba("SVGF first wavelet feedback")),
            temporal: rgba("SVGF temporal radiance"),
            scratch: std::array::from_fn(|_| rgba("SVGF radiance and standard deviation")),
            dummy: Target::new(
                d,
                "inactive SVGF moments",
                1,
                1,
                wgpu::TextureFormat::Rgba32Float,
                true,
            ),
            steps,
            layout,
            pipelines,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        d: &wgpu::Device,
        e: &mut wgpu::CommandEncoder,
        common: &wgpu::BindGroup,
        gbuffer: &wgpu::BindGroup,
        noisy: &Target,
        output: &Target,
        old_depth: &Target,
        old_normal: &Target,
        current: usize,
        iterations: u32,
        debug: u32,
    ) {
        if debug >= 3 {
            e.copy_texture_to_texture(
                noisy.texture.as_image_copy(),
                output.texture.as_image_copy(),
                noisy.texture.size(),
            );
            return;
        }
        let old = 1 - current;
        let pass = |e: &mut wgpu::CommandEncoder,
                    stage: usize,
                    input: &Target,
                    target: &Target,
                    moment_target: &Target,
                    stride_index: u32| {
            let views = [
                &noisy.view,
                &self.feedback[old].view,
                &self.moments[old].view,
                &input.view,
                if stage == 0 {
                    &self.moments[old].view
                } else {
                    &self.moments[current].view
                },
                &target.view,
                &moment_target.view,
            ];
            let mut entries: Vec<_> = views
                .iter()
                .enumerate()
                .map(|(i, v)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(v),
                })
                .collect();
            entries.push(wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.steps,
                    offset: stride_index as u64
                        * d.limits().min_uniform_buffer_offset_alignment as u64,
                    size: std::num::NonZeroU64::new(16),
                }),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&old_depth.view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&old_normal.view),
            });
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("SVGF pass"),
                layout: &self.layout,
                entries: &entries,
            });
            let mut p = e.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(["SVGF temporal", "SVGF variance", "SVGF atrous"][stage]),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.pipelines[stage]);
            p.set_bind_group(0, common, &[]);
            p.set_bind_group(1, gbuffer, &[]);
            p.set_bind_group(2, &group, &[]);
            p.dispatch_workgroups(
                noisy.texture.width().div_ceil(8),
                noisy.texture.height().div_ceil(8),
                1,
            );
        };
        // Temporal writes only current moments; its sampled moments slot refers
        // to old moments to prevent sampling/storage aliasing.
        pass(
            e,
            0,
            &self.feedback[old],
            &self.temporal,
            &self.moments[current],
            0,
        );
        pass(e, 1, &self.temporal, &self.scratch[0], &self.dummy, 0);
        for i in 0..iterations {
            pass(
                e,
                2,
                &self.scratch[(i % 2) as usize],
                &self.scratch[(1 - i % 2) as usize],
                &self.dummy,
                i,
            );
            if i == 0 {
                e.copy_texture_to_texture(
                    self.scratch[1].texture.as_image_copy(),
                    self.feedback[current].texture.as_image_copy(),
                    noisy.texture.size(),
                );
            }
        }
        e.copy_texture_to_texture(
            self.scratch[(iterations % 2) as usize]
                .texture
                .as_image_copy(),
            output.texture.as_image_copy(),
            noisy.texture.size(),
        );
    }
}
