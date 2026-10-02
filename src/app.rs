use crate::{Args, renderer::Renderer, scene::Scene, settings::Settings};
use anyhow::Result;
use glam::{EulerRot, Quat, Vec3};
use std::{collections::HashSet, sync::Arc, time::Instant};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};
#[derive(Clone, Copy, Debug, PartialEq)]
enum Selection {
    None,
    Object(usize),
    Light(usize),
}
struct App {
    scene: Scene,
    settings: Settings,
    args: Args,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    surface_pipeline: Option<wgpu::RenderPipeline>,
    ctx: egui::Context,
    gui_state: Option<egui_winit::State>,
    gui_renderer: Option<egui_wgpu::Renderer>,
    selection: Selection,
    keys: HashSet<KeyCode>,
    freelook: bool,
    cursor: (f64, f64),
    speed: f32,
    last: Instant,
    error: Option<anyhow::Error>,
    frames: u32,
}
pub fn run(scene: Scene, settings: Settings, args: Args) -> Result<()> {
    let events = EventLoop::new()?;
    let mut app = App {
        scene,
        settings,
        args,
        window: None,
        renderer: None,
        surface: None,
        surface_config: None,
        surface_pipeline: None,
        ctx: Default::default(),
        gui_state: None,
        gui_renderer: None,
        selection: Selection::None,
        keys: HashSet::new(),
        freelook: false,
        cursor: (0., 0.),
        speed: 3.,
        last: Instant::now(),
        error: None,
        frames: 0,
    };
    events.run_app(&mut app)?;
    if let Some(e) = app.error {
        Err(e)
    } else {
        Ok(())
    }
}
impl App {
    fn initialize(&mut self, events: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            events.create_window(
                Window::default_attributes()
                    .with_title("Instant Radiosity Lab — Vulkan")
                    .with_inner_size(winit::dpi::PhysicalSize::new(
                        self.settings.render_width,
                        self.settings.render_height,
                    )),
            )?,
        );
        let mut renderer = pollster::block_on(Renderer::new(&self.scene, self.settings.clone()))?;
        renderer.debug_mode = self.args.debug;
        let surface = renderer.instance.create_surface(window.clone())?;
        let caps = surface.get_capabilities(&renderer.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .ok_or_else(|| {
                anyhow::anyhow!("surface has no sRGB format for linear HDR presentation")
            })?;
        anyhow::ensure!(
            !self.args.editor_smoke || caps.usages.contains(wgpu::TextureUsages::COPY_SRC),
            "editor screenshot requires a COPY_SRC-capable surface"
        );
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if self.args.editor_smoke {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&renderer.device, &config);
        let state = egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(renderer.device.limits().max_texture_dimension_2d as usize),
        );
        let gui = egui_wgpu::Renderer::new(
            &renderer.device,
            format,
            egui_wgpu::RendererOptions::default(),
        );
        self.surface_pipeline = Some(renderer.surface_pipeline(format));
        self.window = Some(window);
        self.gui_state = Some(state);
        self.gui_renderer = Some(gui);
        self.surface = Some(surface);
        self.surface_config = Some(config);
        self.renderer = Some(renderer);
        if self.args.editor_smoke {
            self.selection = Selection::Object(0);
        }
        Ok(())
    }
    fn select(&mut self) {
        let window = self.window.as_ref().unwrap();
        let size = window.inner_size();
        let uv = glam::Vec2::new(
            self.cursor.0 as f32 / size.width as f32,
            self.cursor.1 as f32 / size.height as f32,
        );
        let inv = self
            .scene
            .manifest
            .camera
            .matrix(self.settings.render_width as f32 / self.settings.render_height as f32)
            .inverse();
        let h = inv * glam::Vec4::new(uv.x * 2. - 1., 1. - uv.y * 2., 1., 1.);
        let origin = self.scene.manifest.camera.position;
        let direction = (h.truncate() / h.w - origin).normalize();
        let mut best = f32::INFINITY;
        self.selection = Selection::None;
        for (i, o) in self.scene.manifest.objects.iter().enumerate() {
            let inv = o.transform.matrix().inverse();
            let ro = inv.transform_point3(origin);
            let rd = inv.transform_vector3(direction);
            let data = &self.scene.data[self.scene.mesh_ids[&o.mesh]];
            for tri in data.indices.as_chunks::<3>().0 {
                let a = Vec3::from_slice(&data.vertices[tri[0] as usize].position);
                let b = Vec3::from_slice(&data.vertices[tri[1] as usize].position);
                let c = Vec3::from_slice(&data.vertices[tri[2] as usize].position);
                if let Some(t) = triangle(ro, rd, a, b, c)
                    && t < best
                {
                    best = t;
                    self.selection = Selection::Object(i);
                }
            }
        }
        for (i, l) in self.scene.manifest.lights.iter().enumerate() {
            let d = l.position - origin;
            let t = d.dot(direction);
            let radius = (d.length() * 0.012).max(0.06);
            if t > 0. && d.distance(direction * t) < radius && t < best {
                best = t;
                self.selection = Selection::Light(i);
            }
        }
    }
    fn redraw(&mut self) -> Result<()> {
        if self.args.editor_smoke {
            let third = self.args.frames / 3;
            self.selection = if self.frames < third {
                Selection::Object(0)
            } else if self.frames < third * 2 {
                self.scene
                    .manifest
                    .lights
                    .iter()
                    .position(|l| l.kind == "spot")
                    .or_else(|| (!self.scene.manifest.lights.is_empty()).then_some(0))
                    .map_or(Selection::None, Selection::Light)
            } else {
                Selection::None
            };
        }
        let dt = self.last.elapsed().as_secs_f32().min(0.1);
        self.last = Instant::now();
        if self.freelook && !self.ctx.egui_wants_keyboard_input() {
            let mut motion = Vec3::ZERO;
            for (key, vector) in [
                (KeyCode::KeyW, -Vec3::Z),
                (KeyCode::KeyS, Vec3::Z),
                (KeyCode::KeyA, -Vec3::X),
                (KeyCode::KeyD, Vec3::X),
            ] {
                if self.keys.contains(&key) {
                    motion += self.scene.manifest.camera.rotation * vector;
                }
            }
            if self.keys.contains(&KeyCode::KeyQ) {
                motion -= Vec3::Z;
            }
            if self.keys.contains(&KeyCode::KeyE) {
                motion += Vec3::Z;
            }
            let boost = if self.keys.contains(&KeyCode::ShiftLeft)
                || self.keys.contains(&KeyCode::ShiftRight)
            {
                4.
            } else {
                1.
            };
            self.scene.manifest.camera.position +=
                motion.normalize_or_zero() * dt * self.speed * boost;
        }
        let window = self.window.as_ref().unwrap();
        let renderer = self.renderer.as_mut().unwrap();
        // Acquire (or recover) the presentation surface before beginning an
        // egui frame. Once run_ui returns, every texture delta it produced must
        // be consumed, even when presentation is temporarily unavailable.
        let frame = match self.surface.as_ref().unwrap().get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface
                    .as_ref()
                    .unwrap()
                    .configure(&renderer.device, self.surface_config.as_ref().unwrap());
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                let surface = renderer.instance.create_surface(window.clone())?;
                surface.configure(&renderer.device, self.surface_config.as_ref().unwrap());
                self.surface = Some(surface);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => anyhow::bail!("surface validation error"),
        };
        let state = self.gui_state.as_mut().unwrap();
        let input = state.take_egui_input(window);
        let mut changed = false;
        let scene = &mut self.scene;
        let selection = &mut self.selection;
        let args = &self.args;
        let mut output = self.ctx.run_ui(input, |root_ui| {
            let ctx = root_ui.ctx().clone();
            egui::Panel::top("toolbar").show(root_ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Instant Radiosity Lab");
                    ui.separator();
                    if ui.button("Scene").clicked() {
                        *selection = Selection::None;
                    }
                    egui::ComboBox::from_id_salt("entities")
                        .selected_text(match *selection {
                            Selection::None => "Select entity".to_string(),
                            Selection::Object(i) => scene.manifest.objects[i].id.clone(),
                            Selection::Light(i) => scene.manifest.lights[i].id.clone(),
                        })
                        .show_ui(ui, |ui| {
                            for (i, o) in scene.manifest.objects.iter().enumerate() {
                                ui.selectable_value(selection, Selection::Object(i), &o.id);
                            }
                            for (i, l) in scene.manifest.lights.iter().enumerate() {
                                ui.selectable_value(selection, Selection::Light(i), &l.id);
                            }
                        });
                    let before = renderer.debug_mode;
                    egui::ComboBox::from_id_salt("debug")
                        .selected_text(
                            [
                                "Combined",
                                "Direct",
                                "Indirect",
                                "Base color",
                                "Normals",
                                "Motion",
                            ][renderer.debug_mode as usize],
                        )
                        .show_ui(ui, |ui| {
                            for (i, name) in [
                                "Combined",
                                "Direct",
                                "Indirect",
                                "Base color",
                                "Normals",
                                "Motion",
                            ]
                            .iter()
                            .enumerate()
                            {
                                ui.selectable_value(&mut renderer.debug_mode, i as u32, *name);
                            }
                        });
                    changed |= before != renderer.debug_mode;
                    if ui.button("Save edits").clicked()
                        && let Err(e) = scene.save(&args.scene)
                    {
                        eprintln!("Save: {e:#}");
                    }
                    ui.label("RMB + WASDQE · Shift boost · Wheel speed · F12 capture");
                });
            });
            if *selection != Selection::None {
                egui::Window::new("Inspector")
                    .fade_in(!args.editor_smoke)
                    .default_width(260.)
                    .show(&ctx, |ui| match *selection {
                        Selection::Object(i) => {
                            let o = &mut scene.manifest.objects[i];
                            ui.heading(&o.id);
                            changed |= vector(ui, "Position", &mut o.transform.position);
                            let (mut x, mut y, mut z) =
                                o.transform.rotation.to_euler(EulerRot::XYZ);
                            let mut angles =
                                Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees());
                            if vector(ui, "Rotation °", &mut angles) {
                                x = angles.x.to_radians();
                                y = angles.y.to_radians();
                                z = angles.z.to_radians();
                                o.transform.rotation = Quat::from_euler(EulerRot::XYZ, x, y, z);
                                changed = true;
                            }
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut o.transform.scale)
                                        .speed(0.01)
                                        .range(0.001..=1000.)
                                        .prefix("Uniform scale "),
                                )
                                .changed();
                            let mat =
                                &mut scene.manifest.materials[scene.material_ids[&o.material]];
                            ui.separator();
                            ui.label(format!("Material: {}", mat.id));
                            changed |= ui
                                .color_edit_button_rgba_unmultiplied(&mut mat.base_color)
                                .changed();
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut mat.roughness, 0.045..=1.)
                                        .text("Roughness"),
                                )
                                .changed();
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut mat.metallic, 0.0..=1.).text("Metallic"),
                                )
                                .changed();
                        }
                        Selection::Light(i) => {
                            let l = &mut scene.manifest.lights[i];
                            ui.heading(&l.id);
                            changed |= vector(ui, "Position", &mut l.position);
                            let mut color = l.color.to_array();
                            if ui.color_edit_button_rgb(&mut color).changed() {
                                l.color = Vec3::from_array(color);
                                changed = true;
                            }
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut l.strength)
                                        .speed(0.1)
                                        .range(0.0..=1e6)
                                        .prefix("Strength "),
                                )
                                .changed();
                            if l.kind == "spot" {
                                let old = l.direction;
                                if vector(ui, "Direction", &mut l.direction) {
                                    l.direction = if l.direction.length_squared() > 1e-8 {
                                        l.direction.normalize()
                                    } else {
                                        old
                                    };
                                    changed = true;
                                }
                                let mut angle = l.angle_radians.to_degrees();
                                if ui
                                    .add(
                                        egui::Slider::new(&mut angle, 1.0..=179.)
                                            .text("Full cone °"),
                                    )
                                    .changed()
                                {
                                    l.angle_radians = angle.to_radians();
                                    changed = true;
                                }
                            }
                        }
                        Selection::None => {}
                    });
            }
        });
        if changed {
            scene.changed();
        }
        self.gui_state
            .as_mut()
            .unwrap()
            .handle_platform_output(window, output.platform_output);
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let size = window.inner_size();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: output.pixels_per_point,
        };
        let gui = self.gui_renderer.as_mut().unwrap();
        // Draining is required: TexturesDelta asserts in debug builds if it is
        // dropped with commands that the integration did not consume.
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                gui.update_texture(&renderer.device, &renderer.queue, id, &delta);
            }
        }
        if let Err(error) = renderer.encode(&self.scene, &mut encoder, false) {
            // Uploads were consumed above. These frees belong after a paint that
            // will not happen, so discard them explicitly and preserve the real
            // renderer error instead of triggering TexturesDelta's drop assert.
            output.textures_delta.free.clear();
            return Err(error);
        }
        renderer.present_surface(&mut encoder, &view, self.surface_pipeline.as_ref().unwrap());
        let user_commands = gui.update_buffers(
            &renderer.device,
            &renderer.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            gui.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        renderer
            .queue
            .submit(user_commands.into_iter().chain([encoder.finish()]));
        // Free only after the GUI commands have been recorded and submitted,
        // then consume the commands so later capture errors cannot mask their
        // real diagnostic with a TexturesDelta drop panic.
        for id in output.textures_delta.free.drain() {
            gui.free_texture(&id);
        }
        if self.args.editor_smoke
            && let Some(path) = &self.args.capture
        {
            let third = self.args.frames / 3;
            let stage = if self.frames + 1 == third {
                Some("object")
            } else if self.frames + 1 == third * 2 {
                Some("light")
            } else if self.frames + 1 == self.args.frames {
                Some("deselected")
            } else {
                None
            };
            if let Some(stage) = stage {
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                let staged = path.with_file_name(format!("{stem}_{stage}.png"));
                let bgra = self.surface_config.as_ref().unwrap().format
                    == wgpu::TextureFormat::Bgra8UnormSrgb;
                renderer.capture_texture(&frame.texture, &staged, bgra)?;
                println!("Editor capture: {}", staged.display());
                if stage == "deselected" {
                    renderer.capture_texture(&frame.texture, path, bgra)?;
                }
            }
        }
        renderer.queue.present(frame);
        self.frames += 1;
        Ok(())
    }
}
fn vector(ui: &mut egui::Ui, label: &str, v: &mut Vec3) -> bool {
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        for x in [&mut v.x, &mut v.y, &mut v.z] {
            changed |= ui.add(egui::DragValue::new(x).speed(0.02)).changed();
        }
    });
    changed
}
fn triangle(ro: Vec3, rd: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = rd.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let t = ro - a;
    let u = t.dot(p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t.cross(e1);
    let v = rd.dot(q) / det;
    if v < 0. || u + v > 1. {
        return None;
    }
    let distance = e2.dot(q) / det;
    (distance > 0.).then_some(distance)
}
impl ApplicationHandler for App {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(e) = self.initialize(events)
        {
            self.error = Some(e);
            events.exit();
        }
    }
    fn window_event(&mut self, events: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let response = self
            .gui_state
            .as_mut()
            .unwrap()
            .on_window_event(self.window.as_ref().unwrap(), &event);
        match event {
            WindowEvent::CloseRequested => events.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let c = self.surface_config.as_mut().unwrap();
                    c.width = size.width;
                    c.height = size.height;
                    self.surface
                        .as_ref()
                        .unwrap()
                        .configure(&self.renderer.as_ref().unwrap().device, c);
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = (position.x, position.y),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.freelook = false;
                self.window.as_ref().unwrap().set_cursor_visible(true);
                let _ = self
                    .window
                    .as_ref()
                    .unwrap()
                    .set_cursor_grab(CursorGrabMode::None);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if event.state == ElementState::Released {
                        self.keys.remove(&key);
                    } else if !response.consumed {
                        self.keys.insert(key);
                        if key == KeyCode::Escape {
                            self.selection = Selection::None;
                        }
                        if key == KeyCode::F12
                            && let Err(e) = self
                                .renderer
                                .as_ref()
                                .unwrap()
                                .capture(std::path::Path::new("artifacts/editor.png"))
                        {
                            eprintln!("Capture: {e:#}");
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Right,
                state,
                ..
            } => {
                if state == ElementState::Released || !response.consumed {
                    self.freelook = state == ElementState::Pressed;
                    let window = self.window.as_ref().unwrap();
                    window.set_cursor_visible(!self.freelook);
                    let _ = window.set_cursor_grab(if self.freelook {
                        CursorGrabMode::Confined
                    } else {
                        CursorGrabMode::None
                    });
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state: ElementState::Pressed,
                ..
            } if !response.consumed => self.select(),
            WindowEvent::MouseWheel { delta, .. } if !response.consumed => {
                let n = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.01,
                };
                self.speed = (self.speed * (n * 0.1).exp()).clamp(0.05, 100.);
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.error = Some(e);
                    events.exit();
                }
                if self.args.editor_smoke && self.frames >= self.args.frames {
                    events.exit();
                }
            }
            _ => {}
        }
    }
    fn device_event(
        &mut self,
        _events: &ActiveEventLoop,
        _id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if self.freelook
            && !self.ctx.egui_wants_pointer_input()
            && let DeviceEvent::MouseMotion { delta } = event
        {
            let c = &mut self.scene.manifest.camera;
            let yaw = Quat::from_axis_angle(Vec3::Z, -delta.0 as f32 * 0.002);
            let pitch = Quat::from_rotation_x(-delta.1 as f32 * 0.002);
            let candidate = (yaw * c.rotation * pitch).normalize();
            let forward = candidate * -Vec3::Z;
            if forward.z.abs() < 0.995 {
                c.rotation = candidate;
            } else {
                c.rotation = (yaw * c.rotation).normalize();
            }
        }
    }
    fn about_to_wait(&mut self, _events: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picking_hit_miss_and_world_parameter() {
        assert_eq!(
            triangle(
                Vec3::new(0.2, 0.2, 2.),
                -Vec3::Z,
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y
            ),
            Some(2.)
        );
        assert!(
            triangle(
                Vec3::new(2., 2., 2.),
                -Vec3::Z,
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y
            )
            .is_none()
        );
    }
}
