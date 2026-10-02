//! Explicit hardware integration checks: cargo test gpu_integration -- --ignored.
use crate::{renderer::Renderer, scene::Scene, settings::Settings};
use glam::Vec3;
use std::path::Path;

fn map(renderer: &Renderer, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    renderer.wait().unwrap();
    rx.recv().unwrap().unwrap();
    let view = buffer.slice(..).get_mapped_range().unwrap();
    let result = view.to_vec();
    drop(view);
    buffer.unmap();
    result
}
pub(super) fn buffer(renderer: &Renderer, source: &wgpu::Buffer) -> Vec<u8> {
    let staging = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test readback"),
        size: source.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, source.size());
    renderer.queue.submit([encoder.finish()]);
    map(renderer, &staging)
}
pub(super) fn texture(renderer: &Renderer, source: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let w = source.width();
    let h = source.height();
    let stride = (w * bpp).div_ceil(256) * 256;
    let staging = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test texture readback"),
        size: stride as u64 * h as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        source.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
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
    renderer.queue.submit([encoder.finish()]);
    map(renderer, &staging)
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..(w * bpp) as usize].iter().copied())
        .collect()
}
pub(super) fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_bits(u16::from_le_bytes(*b)).to_f32())
        .collect()
}
pub(super) fn frame(renderer: &mut Renderer, scene: &Scene) {
    let mut e = renderer.device.create_command_encoder(&Default::default());
    renderer.encode(scene, &mut e, false).unwrap();
    renderer.queue.submit([e.finish()]);
    renderer.wait().unwrap();
}

#[test]
#[ignore = "requires native Vulkan hardware ray queries and exported test scene"]
fn gpu_integration_svgf_noise_edges_and_disocclusion() {
    let mut scene = Scene::load(Path::new("assets/test_scene/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 160;
    settings.render_height = 100;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    let mut rng = 123456789u32;
    let mut last = None;
    for _ in 0..12 {
        frame(&mut renderer, &scene);
        let signal: Vec<_> = (0..16000)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let v = if rng & 1 == 0 { 0. } else { 2. };
                [v, v, v, 1.]
            })
            .collect();
        last = Some(renderer.test_svgf_signal(&signal, false, false));
    }
    let (filtered, moments, depth) = last.unwrap();
    let mut error = 0.;
    let mut mean = 0.;
    let mut count = 0.;
    for (i, d) in depth.iter().enumerate() {
        if *d >= 1. {
            continue;
        }
        let v = filtered[i * 4];
        assert!(v.is_finite() && filtered[i * 4 + 3].is_finite());
        assert!(moments[i * 4 + 1] >= 0. && moments[i * 4 + 1].is_finite());
        error += (v - 1.).powi(2);
        mean += v;
        count += 1.;
    }
    println!(
        "SVGF binary-noise MSE: {}, mean: {}",
        error / count,
        mean / count
    );
    assert!(
        error / count < 0.1,
        "SVGF must remove at least 90% of the known unit input MSE"
    );
    assert!(
        (mean / count - 1.).abs() < 0.04,
        "SVGF must preserve mean radiance"
    );
    // Force a fresh lighting generation and a material boundary.  Spatial
    // filtering must preserve it even though neighbouring geometry is planar.
    scene.changed();
    frame(&mut renderer, &scene);
    let signal: Vec<_> = (0..16000)
        .map(|i| {
            let v = if i % 160 < 80 { 0.5 } else { 2. };
            [v, v, v, 1.]
        })
        .collect();
    let (filtered, _, depth) = renderer.test_svgf_signal(&signal, true, false);
    let mut edge_count = 0;
    for (i, d) in depth.iter().enumerate() {
        if *d < 1. && (78..=81).contains(&(i % 160)) {
            assert!(
                (filtered[i * 4] - signal[i][0]).abs() < 0.02,
                "material edge bled across pixel {i}"
            );
            edge_count += 1;
        }
    }
    assert!(edge_count > 20);
    // An out-of-bounds reprojection must start moments/history at one frame.
    frame(&mut renderer, &scene);
    let (_, moments, depth) = renderer.test_svgf_signal(&signal, false, true);
    for (i, d) in depth.iter().enumerate() {
        if *d < 1. {
            assert_eq!(moments[i * 4 + 2], 1.);
        }
    }
    scene.changed();
    frame(&mut renderer, &scene);
    let hdr: Vec<_> = (0..16000)
        .map(|i| {
            let v = if i % 2 == 0 { 0. } else { 60000. };
            [v, v, v, 1.]
        })
        .collect();
    let (filtered, moments, _) = renderer.test_svgf_signal(&hdr, false, false);
    assert!(
        filtered.iter().all(|v| v.is_finite()),
        "HDR wavelet radiance/variance must stay finite"
    );
    assert!(
        moments.iter().all(|v| v.is_finite()),
        "HDR squared luminance must not overflow"
    );
    assert!(
        moments.as_chunks::<4>().0.iter().any(|m| m[1] > 65504.),
        "moments require float32 range rather than float16"
    );
}

#[test]
#[ignore = "requires native Vulkan hardware ray queries and exported test scene"]
fn gpu_integration_lighting_history_and_motion() {
    let mut scene = Scene::load(Path::new("assets/test_scene/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 160;
    settings.render_height = 100;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    renderer.debug_mode = 1;
    for _ in 0..40 {
        frame(&mut renderer, &scene);
    }
    assert!(
        renderer.test_history_max_age() >= 16.,
        "static surfaces must accumulate temporal HDR history rather than restarting every frame"
    );
    assert!(
        renderer.test_history_mature_fraction(16.) > 0.25,
        "temporal reconstruction must retain history across the visible static surfaces"
    );
    for indirect in [false, true] {
        let (count, age) = renderer.test_reservoir_maturity(indirect);
        assert!(
            count
                >= renderer.settings.restir_max_history
                    * renderer.settings.restir_direct_candidates,
            "final reservoir must retain temporal candidates: {count}"
        );
        assert!(
            age >= 1 && age <= renderer.settings.restir_max_history,
            "ReSTIR selection must reuse samples within its short lifetime: {age}"
        );
        assert!(
            renderer.test_reservoir_mature_fraction(indirect, 1) > 0.1,
            "ReSTIR reservoirs must reuse temporal samples across static surfaces"
        );
    }
    renderer.test_transport_resources();
    renderer.test_indirect_reuse();
    let before = texture(&renderer, &renderer.display.texture, 4);
    assert!(before.as_chunks::<4>().0.iter().any(|p| p[0] > 40));
    for light in &mut scene.manifest.lights {
        light.strength = 0.;
    }
    scene.changed();
    frame(&mut renderer, &scene);
    let dark = texture(&renderer, &renderer.display.texture, 4);
    assert!(
        dark.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0]),
        "changed lighting must invalidate resolved history immediately"
    );
    let generation = scene.generation;
    scene.manifest.camera.position += Vec3::new(0.15, 0., 0.);
    frame(&mut renderer, &scene);
    assert_eq!(generation, scene.generation);
    renderer.test_motion_nonzero();
    renderer.debug_mode = 2;
    frame(&mut renderer, &scene);
    assert_eq!(
        renderer.test_history_max_age(),
        1.,
        "debug lighting changes must invalidate SVGF history"
    );
    scene.manifest.objects[0].transform.position += Vec3::X * 0.2;
    scene.changed();
    frame(&mut renderer, &scene);
    // Mesh-local samples are immutable and remain identical across transforms.
    let reloaded = Scene::load(Path::new("assets/test_scene/scene.json")).unwrap();
    assert_eq!(scene.data[0].sdf, reloaded.data[0].sdf);
}

#[test]
#[ignore = "requires native Vulkan and tools/test_export.py fixtures"]
fn gpu_integration_texture_channels() {
    let mut scene = Scene::load(Path::new("artifacts/export_tests/textures/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 64;
    settings.render_height = 64;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    frame(&mut renderer, &scene);
    renderer.test_texture_channels();
    for m in &mut scene.manifest.materials {
        m.base_color_srgb = false;
    }
    scene.changed();
    frame(&mut renderer, &scene);
    for (actual, byte) in renderer
        .test_center_base_color()
        .into_iter()
        .zip([128., 64., 191.])
    {
        assert!(
            (actual - byte / 255.).abs() < 0.001,
            "Non-Color base texture must remain linear"
        );
    }
}

#[test]
#[ignore = "requires native Vulkan and exported analytic cube fixture"]
fn gpu_integration_inverse_square_and_spot_cone() {
    let mut scene = Scene::load(Path::new("artifacts/export_tests/cube/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 64;
    settings.render_height = 64;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    renderer.debug_mode = 1;
    frame(&mut renderer, &scene);
    let near = renderer.test_center_radiance();
    assert!(near > 0.);
    scene.manifest.lights[0].position = Vec3::new(0., 0., 6.);
    scene.changed();
    frame(&mut renderer, &scene);
    let far = renderer.test_center_radiance();
    assert!(
        (near / far - 6.25).abs() < 0.05,
        "distance 2 versus 5 must attenuate by 25/4; got {}",
        near / far
    );
    let l = &mut scene.manifest.lights[0];
    l.kind = "spot".into();
    l.angle_radians = 0.4;
    l.direction = -Vec3::Z;
    scene.changed();
    frame(&mut renderer, &scene);
    assert!((renderer.test_center_radiance() - far).abs() < 0.002);
    scene.manifest.lights[0].direction = Vec3::Z;
    scene.changed();
    frame(&mut renderer, &scene);
    assert_eq!(
        renderer.test_center_radiance(),
        0.,
        "outside the full spotlight cone must be zero"
    );
}

#[test]
#[ignore = "requires native Vulkan and exported analytic cube fixture"]
fn gpu_integration_moving_occluder_and_metallic_vpls() {
    let mut scene = Scene::load(Path::new("artifacts/export_tests/cube/scene.json")).unwrap();
    let mut occluder = scene.manifest.objects[0].clone();
    occluder.id = "moving_occluder".into();
    occluder.transform.position = Vec3::new(20., 0., 2.);
    occluder.transform.scale = 0.25;
    scene.manifest.objects.push(occluder);
    scene.manifest.lights[0].position = Vec3::new(3., 0., 3.);
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 64;
    settings.render_height = 64;
    settings.vpl_max_bounces = 1;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    renderer.debug_mode = 1;
    frame(&mut renderer, &scene);
    assert!(renderer.test_center_radiance() > 0.001);
    scene.manifest.objects[1].transform.position = Vec3::new(1.5, 0., 2.);
    scene.changed();
    frame(&mut renderer, &scene);
    assert_eq!(
        renderer.test_center_radiance(),
        0.,
        "moving a uniformly scaled occluder into the light ray must update the SDF hierarchy"
    );
    scene.manifest.objects[1].transform.position.x = 20.;
    scene.manifest.materials[0].metallic = 1.;
    scene.changed();
    for _ in 0..4 {
        frame(&mut renderer, &scene);
    }
    renderer.test_transport_resources();
}

#[test]
#[ignore = "requires native Vulkan hardware ray queries and exported test scene"]
fn gpu_integration_vpl_budget_and_power_normalization() {
    let mut scene = Scene::load(Path::new("assets/test_scene/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 16;
    settings.render_height = 16;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings.clone())).unwrap();
    frame(&mut renderer, &scene);
    let baseline = renderer.test_vpl_records();
    drop(renderer);
    settings.vpl_count *= 2;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings.clone())).unwrap();
    frame(&mut renderer, &scene);
    let doubled_budget = renderer.test_vpl_records();
    drop(renderer);
    for (a, b) in baseline.iter().zip(&doubled_budget) {
        assert_eq!(
            &a[..8],
            &b[..8],
            "path budget must preserve emission and roulette decisions"
        );
        for i in 8..11 {
            assert!(
                (a[i] - 2. * b[i]).abs() < 1e-6,
                "doubling path budget must halve each shared path's flux"
            );
        }
    }
    settings.vpl_count /= 2;
    for l in &mut scene.manifest.lights {
        l.strength *= 2.;
    }
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    frame(&mut renderer, &scene);
    let doubled_power = renderer.test_vpl_records();
    for (a, b) in baseline.iter().zip(doubled_power) {
        assert_eq!(
            &a[..8],
            &b[..8],
            "source power must preserve the path distribution"
        );
        for i in 8..11 {
            assert!(
                (2. * a[i] - b[i]).abs() < 1e-6,
                "doubling source power must double terminal flux"
            );
        }
    }
    assert!(
        baseline.iter().any(|v| v[8..11].iter().any(|f| *f > 0.)),
        "normalization checks need contributing VPLs"
    );
}

#[test]
#[ignore = "requires native Vulkan and exported alpha-channel texture fixture"]
fn gpu_integration_alpha_scalar_texture() {
    let scene = Scene::load(Path::new("artifacts/export_tests/alpha_scalar/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 64;
    settings.render_height = 64;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    frame(&mut renderer, &scene);
    let params = renderer.test_center_material_params();
    assert!(
        (params[0] - 0.375).abs() < 0.005,
        "roughness must use alpha, not the distinct red channel: {params:?}"
    );
    assert!(params[1].abs() < 0.001);
}

#[test]
#[ignore = "requires native Vulkan and exported checker texture fixture"]
fn gpu_integration_srgb_interpolation_is_linear() {
    let scene = Scene::load(Path::new("artifacts/export_tests/filtering/scene.json")).unwrap();
    let mut settings = Settings::load(Path::new("settings.json")).unwrap();
    settings.render_width = 64;
    settings.render_height = 64;
    let mut renderer = pollster::block_on(Renderer::new(&scene, settings)).unwrap();
    frame(&mut renderer, &scene);
    let uv = (glam::Vec2::splat(32.) + 0.5) / 64.;
    let clip = glam::Vec4::new(uv.x * 2. - 1., 1. - uv.y * 2., 1., 1.);
    let h = scene.manifest.camera.matrix(1.).inverse() * clip;
    let origin = scene.manifest.camera.position;
    let delta = h.truncate() / h.w - origin;
    let p = origin + delta * ((1. - origin.z) / delta.z);
    // Fixture pixels were authored in Blender's bottom-up UV coordinates.
    // The PNG rows flip on export; the shader flips V when reading them.
    let texture_uv = glam::Vec2::new((p.x + 1.) * 0.5, (p.y + 1.) * 0.5);
    let f = (texture_uv * 8. - 0.5).fract();
    let expected = f.x + f.y - 2. * f.x * f.y;
    for actual in renderer.test_center_base_color() {
        assert!(
            (actual - expected).abs() < 0.004,
            "black/white sRGB interpolation must be linear: {actual} vs {expected}"
        );
    }
}
