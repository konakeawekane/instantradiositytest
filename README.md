# Instant Radiosity Test Renderer

An experimental Rust/wgpu renderer for studying Instant Radiosity, ReSTIR direct and indirect lighting, hardware ray-query VPL generation, conservative mesh SDF soft shadows, and SVGF spatiotemporal denoising. Scenes are authored in Blender and exported to a compact runtime format. The editor provides camera controls, scene inspection, debug view modes, and live renderer settings.

## Instructions for Build and Use

### Prerequisites

Install the following before building:

1. Rust stable with Cargo and the Rust 2024 edition toolchain.
2. A Vulkan-capable GPU and driver with hardware ray-query support. The milestone currently targets native Vulkan through wgpu; browser/WebGPU is not supported.
3. Blender 4.x or another recent Blender release capable of running the supplied Python exporter.
4. Python 3.10 or newer for scene export tests.

The reference development machine is Windows 11 with an NVIDIA GeForce RTX 5070 Laptop GPU, Vulkan drivers, Rust stable, Blender, and Python 3.

### Build

From the repository root:

```powershell
cargo build --release
```

Run the Rust test suite and shader validation:

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The native GPU integration tests are marked ignored because they require Vulkan and the exported scene. In order to run them be sure to include --ignored:

```powershell
cargo test gpu_integration -- --ignored --test-threads=1
```

### Export a Blender scene

The supplied scene is [test_scene.blend](test_scene.blend). Export it after changing geometry, materials, textures, lights, or camera settings:

```powershell
blender --background test_scene.blend --python-exit-code 1 --python tools/export_scene.py -- --output assets/test_scene/scene.json
```

The exporter supports material constants and the supported texture inputs documented in [experimental_renderer_spec.md](experimental_renderer_spec.md). Renderable meshes must satisfy the watertight manifold requirement used by the SDF builder.

### Run the interactive editor

```powershell
cargo run --release
```

The default scene is `assets/test_scene/scene.json` and the default settings file is `settings.json`. Use `--scene PATH` and `--settings PATH` to select alternatives. The editor supports the normal camera and selection workflow, live settings inspection, and debug display modes.

### Produce a headless render or benchmark

```powershell
cargo run --release -- --frames 64 --capture artifacts/render.png --benchmark artifacts/benchmark.json
```

Useful options include:

```text
--width N --height N       Override the render resolution
--frames N                 Number of frames to simulate before capture
--debug N                  0 combined, 1 direct, 2 indirect, 3 albedo, 4 normal, 5 motion vectors
--vpl-count N              Override the VPL count
--bounces N                Override maximum VPL path bounces
--validate-only            Validate the scene and exit without creating a window
--editor-smoke             Run the short editor smoke workflow and capture its UI
```

Motion debug mode uses a signed two-channel visualization: red represents horizontal motion, green represents vertical motion, and gray represents zero motion.

### Verify the complete workflow

When Blender is available, run:

```powershell
python tools/verify.py --no-editor
```

This exports the scene, runs Python and Rust tests, runs the Vulkan integration tests, builds the release executable, checks deterministic repeated captures, produces full-resolution benchmark output, and saves direct/indirect SVGF captures under `artifacts/verification`.

## Development Environment

The project uses the following main components:

* Rust stable, edition 2024, with Cargo
* `wgpu 30.0.1` with native Vulkan and WGSL support
* `winit 0.30`, `egui 0.36.2`, and `egui-wgpu 0.36.2` for the editor
* `glam 0.33` for vector and matrix math
* `serde`/`serde_json` for settings and scene metadata
* `naga 30.0.0` in development tests for WGSL parsing and SPIR-V validation
* Python 3.10+ with Blender's `bpy`, `bmesh`, and `mathutils` modules for export
* NVIDIA Vulkan drivers with hardware ray-query support

The renderer uses deferred rasterization for the G-buffer, compute passes for VPL generation and ReSTIR, a stackless object BVH over conservative per-mesh SDFs, and selected-sample SDF visibility. ReSTIR uses eight direct and eight VPL candidates, four spatial neighbors, and a four-frame reservoir history by default. SVGF owns a separate 32-frame radiance/moment history and four variance-guided a trous passes.

The default settings are 100 VPL paths, up to three VPL bounces, 96 SDF steps, a 0.001 hit epsilon, and a soft-shadow factor of 16. Direct and indirect lighting each perform at most one selected-sample SDF visibility march per valid pixel. ReSTIR reuses a short four-frame reservoir window; SVGF handles longer temporal accumulation, luminance moments, variance estimation, and edge-aware spatial reconstruction.

The main implementation files are:

* [src/renderer.rs](src/renderer.rs) - wgpu resources, pass scheduling, profiling, capture, and presentation
* [src/shaders/lighting.wgsl](src/shaders/lighting.wgsl) - ReSTIR candidate generation, temporal/spatial reuse, and selected visibility resolve
* [src/svgf.rs](src/svgf.rs) and [src/shaders/svgf.wgsl](src/shaders/svgf.wgsl) - SVGF history, moments, variance, and a trous filtering
* [src/shaders/vpl.wgsl](src/shaders/vpl.wgsl) - Instant Radiosity VPL generation and path continuation
* [src/shaders/sdf.wgsl](src/shaders/sdf.wgsl) - conservative SDF queries and soft-shadow marching
* [tools/export_scene.py](tools/export_scene.py) - Blender scene exporter and SDF construction
* [tools/verify.py](tools/verify.py) - end-to-end export, test, capture, and benchmark workflow
* [settings.json](settings.json) - default 1920x1200 renderer configuration
* [experimental_renderer_spec.md](experimental_renderer_spec.md) - architecture and implementation contract

## Useful Websites to Learn More

* [Rust Book](https://doc.rust-lang.org/book/)
* [Cargo Book](https://doc.rust-lang.org/cargo/)
* [wgpu documentation](https://docs.rs/wgpu/latest/wgpu/)
* [WGSL specification](https://www.w3.org/TR/WGSL/)
* [Blender Python API](https://docs.blender.org/api/current/)
* [Blender command-line reference](https://docs.blender.org/manual/en/latest/advanced/command_line/)
* [Spatiotemporal Variance-Guided Filtering - NVIDIA Research](https://research.nvidia.com/labs/rtr/publication/schied2017spatiotemporal/)
* [Instant Radiosity overview](https://en.wikipedia.org/wiki/Instant_radiosity)
* [Inigo Quilez signed-distance-function soft shadows](https://iquilezles.org/articles/rmshadows/)

## Future Work

The following items remain useful follow-up work:

* [ ] Reduce the full-resolution GPU cost from the current approximately 20 ms combined median toward a 8 ms target on RTX 5070.
* [ ] Improve VPL importance sampling and temporal reuse so indirect illumination needs fewer regenerated paths.
* [ ] Add exposure and tone-mapping controls for HDR scenes whose values currently clip at presentation.
* [ ] Add an explicit SVGF variance/history debug view and on-screen pass timings in the editor.
