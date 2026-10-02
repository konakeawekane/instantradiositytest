# Instant Radiosity Lab

Native Rust/wgpu Vulkan experiment implementing [the renderer specification](experimental_renderer_spec.md): deferred metallic-roughness PBR, hardware ray-query VPL paths, independent direct/indirect per-pixel ReSTIR, mesh-local conservative SDF soft shadows using Quilez's second method, temporal reconstruction, and an egui editor.

The exported `test_scene.blend` assets are included. Run from the repository root:

```powershell
cargo run --release
```

Requirements: a current Rust toolchain, native Vulkan hardware ray queries, and a compatible graphics driver. This workspace was tested with Rust 1.98.1, Blender 4.5.2 LTS, and the RTX 5070 Laptop GPU. The app selects a ray-query-capable discrete Vulkan adapter and reports unsupported capabilities clearly. Blender and Python/Pillow are needed for asset export and the verification workflow.

## Editor

Hold RMB to look and fly. WASD moves relative to the camera, Q/E moves vertically in the Z-up world, Shift increases speed, and the wheel changes fly speed. Left-click selects geometry; the entity menu selects objects and lights. Escape or the Scene button deselects and hides the inspector. GUI input takes priority over camera input.

The inspector edits position, rotation in degrees, **uniform** scale, base color, roughness, metallic, and applicable point/spotlight parameters. Spot angle is the full cone width. Material edits affect all objects sharing that material. Edits stay in memory until **Save edits** writes the renderer manifest. The source Blender file is never modified. F12 writes a renderer-only PNG to `artifacts/editor.png`.

The toolbar provides Combined, Direct, Indirect, Base color, Normals, and Motion views. Internal resolution is fixed by settings or CLI overrides; resizing the window scales presentation.

## Export a Blender scene

```powershell
& 'C:\Program Files\Blender Foundation\Blender 4.5\blender.exe' --background test_scene.blend --python-exit-code 1 --python tools/export_scene.py -- --output assets/test_scene/scene.json --sdf-resolution 48
```

Use an active perspective camera, closed watertight consistently oriented meshes with outward normals, and point/spot lights. Evaluated modifiers are baked. Positive nonuniform Blender scale is baked into geometry; runtime scale is always a single positive scalar. Shear, negative scale, open meshes, and unsupported material/light inputs are rejected. One material per object is supported by scene version 1. Shared geometry is deduplicated. An object or mesh custom property `sdf_resolution` overrides the default independently, in the range 4–256.

Materials accept constants or direct Image Texture connections to Principled base color, roughness, and metallic, and a tangent Normal Map image for normals. Scalar Color images must be grayscale; Separate Color in RGB mode supports packed scalar channels, and Image Texture Alpha supports scalar alpha. Base color and normal maps require the full image Color output. Images use repeat sampling, Linear interpolation, and the active UV map. Base-color textures may be sRGB or linear/Non-Color; scalar and normal textures must be Non-Color. Color decoding precedes interpolation: CPU resampling operates in linear space, and GPU sRGB texture views decode before bilinear filtering. Procedural graphs, alternate UV sets, emissive/transparent materials, and additional scattering models are rejected. Material texture layers are resampled to a common square size up to 2048 pixels, without mipmaps. Texture precision is eight bits per channel in this milestone.

Blender light power is converted to intensity by dividing by the point sphere or spot cone solid angle. Geometry stays Z-up, cameras look along local -Z, quaternion storage is XYZW, and the exported vertical FOV is derived for the requested 1920:1200 aspect ratio.

The exporter atomically publishes a JSON manifest referencing immutable, content-addressed binary payloads. A failed export preserves the previous usable manifest and payloads. Cached `sdf_*.bin` files accelerate repeated exports. Mesh vertices are little-endian 64-byte position/normal/tangent/UV records, indices are uint32, and SDF grid nodes are float32 in x-fastest order. The manifest validates all ranges and references.

## Test, capture, and profile

```powershell
python tools/verify.py
```

This runs formatting and Clippy, exports the supplied Blender scene, runs exporter and Rust mathematical/shader tests, tests the provided image analysis package, runs explicit Vulkan GPU integration tests, builds release, checks scene loading, compares two deterministic renders exactly, captures object/light/deselected editor states, and measures 64 frames at 1920×1200. Logs, PNGs, timing data, and `report.json` go to `artifacts/verification/`. Supply `--blender PATH` if automatic discovery fails; `--no-editor` skips the native window smoke test.

Use `--enforce-budget` to make verification return failure when the measured GPU median exceeds 8 ms. Functional validation and performance acceptance are reported separately. **The 8 ms target is not yet achieved** at full resolution; see the generated `benchmark.json` for the current measurement and pass breakdown. GPU timestamps exclude CPU editor work, PNG readback, and OS presentation. The first eight frames are excluded from the median and p95.

The verified 64-frame run on the supplied scene measured **15.9 ms median / 17.4 ms p95** at 1920×1200, improved from the previous 24.4 ms median. Selected-sample visibility remains the largest cost: 2.7 ms direct and 6.7 ms indirect in the final measured frame. The full workflow passed 106 tests, exact repeat captures, the lit-wall assertion, and editor smoke captures. These are experimental measurements on this laptop, not a guarantee for other scenes or power settings.

Individual commands:

```powershell
cargo test
cargo test gpu_integration -- --ignored --test-threads=1
cargo run --release -- --validate-only
cargo run --release -- --capture artifacts/render.png --benchmark artifacts/timing.json --frames 64
cargo run --release -- --width 640 --height 400 --debug 1 --capture artifacts/direct.png
cargo run --release -- --editor-smoke --width 960 --height 600 --frames 24 --capture artifacts/editor_ui.png
python -m image_analysis_tools.render_debug --select all --evaluate statistics --images artifacts/render.png --compact
```

GPU tests check fixed VPL counts and finite flux, path-budget and source-power normalization, metallic terminal reflection, retained VPL samples across archive wrap, temporal/spatial represented candidate counts, immediate lighting-history invalidation, camera motion, inverse-square attenuation, spotlight cone rejection, moving uniformly scaled shadow occluders, packed RGB/alpha texture channels, normal mapping, and sRGB/linear decoding. Export tests compare signed cube distances against analytic values and check rejection, baked scale, different resolutions, and atomic failure behavior. Every WGSL entry point is validated and compiled to SPIR-V in the Rust tests. Image assertions check exact repeated captures and absence of black occlusion streaks in the supplied scene's known lit wall patch.

## Rendering details and experimental limits

Each domain uses three distinct 32-byte-per-pixel reservoir allocations for candidate, temporal, and spatial results. VPLs regenerate into separate current/previous pools every frame. A bounded archive keeps older selected pools addressable by frame stamp; stale or incompatible generations are rejected before reuse. The fixed path budget stores one terminal record per launched path, including empty miss slots. Full Lambert/GGX BSDF sampling and roulette/pdf correction handle path continuation; terminal flux estimates reflected hemispherical energy in the point-emitter representation.

An updated stackless object BVH accelerates SDF queries. Local float32 fields live in an immutable 3D texture atlas with hardware interpolation when available and manual interpolation otherwise. The query subtracts a voxel-diagonal interpolation/error allowance, extends conservatively outside padded bounds, and scales positive distances by a per-field Lipschitz bound computed from analytic trilinear cell-corner gradients. Enclosing geometry boxes and spheres provide additional signed lower bounds; their maximum tightens conservative stepping. Verified box topology lets the exact box bound skip a dominated field lookup. BVH pruning matches these query bounds. Uniform scale converts local distances and offsets to world units. Separate direct and indirect visibility kernels retain float32 direct lighting until combination, followed by the distinct temporal denoising pass. Only selected positive contributions march shadows, at most once per domain and pixel. Budget-exhausted marches return occluded.

Coarse SDF resolution requires a voxel-sized surface offset and can detach or soften shadows around small features. VPLs remain a point-emitter approximation, and 100 stochastic paths per frame leave visible indirect noise despite temporal filtering. HDR lighting is displayed directly through an sRGB attachment, so values above display range clip; exposure controls and tone mapping are outside this specification. These artifacts and the measured frame-budget gap are recorded rather than hidden by lowering resolution or changing the architecture.
