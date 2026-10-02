# Experimental Rust Renderer — Source-of-Truth Implementation Specification

**Status:** Milestone specification / implementation contract  
**Version:** 1.0  
**Target language:** Rust  
**GPU API:** `wgpu`  
**Primary ray-tracing backend for this milestone:** Native Vulkan  
**Authoring:** Blender + Blender Python CLI  
**Renderer type:** Experimental deferred PBR renderer with ReSTIR lighting, ray-traced VPL generation, hierarchical per-mesh SDF soft visibility, and temporal reconstruction

---

## 1. Purpose and status of this document

This document is the architectural source of truth for the renderer milestone. Every decision recorded below is considered part of the milestone implementation, not a proposed later enhancement.

The implementation is intentionally narrow. It is designed to investigate stochastic direct and indirect lighting using ReSTIR, ray-traced VPL generation, and SDF-based soft shadows rather than to provide a broad game-engine feature set.

The implementation is complete when all requirements in this document are implemented and the completion criteria in Section 23 are satisfied.

Where the implementation has freedom, that freedom applies only to internal code organization or equivalent GPU resource layout. It must not change the externally observable architecture or the invariants stated here.

---

## 2. Locked design decisions

### 2.1 Renderer and platform

- Rust is the implementation language.
- `wgpu` is the GPU API.
- The renderer uses compute passes heavily in addition to rasterization.
- Hardware ray queries are required for VPL generation.
- Current `wgpu` exposes ray-query support through the experimental ray-query feature, currently documented as native-only and Vulkan-only. Therefore native Vulkan is the milestone's required backend. Browser/WebGPU rendering is not a target of this milestone. See the current `wgpu` API documentation in Section 24. [1]

### 2.2 Scene authoring

- Blender is the scene authoring tool.
- Blender's Python CLI/background mode is the extraction path.
- The exporter extracts only the data required by this renderer.
- The renderer consumes a renderer-oriented JSON scene manifest rather than attempting to reproduce Blender's full scene graph.
- Heavy geometry/SDF payloads may be stored in binary assets referenced by the JSON manifest.

### 2.3 Geometry and transforms

- Mesh geometry is static after asset creation.
- Scene objects are movable in the world.
- Object movement is translation, rotation, and **uniform scale only**.
- Non-uniform scale is not an editable renderer transform.
- SDFs are mesh-local, precomputed, immutable assets.
- Moving or rotating an object never rebuilds its mesh SDF.
- If a Blender-authored object contains non-uniform scale, the export pipeline must bake that scale into the mesh geometry and export a uniform renderer scale of `1.0`, or reject the asset. The renderer itself must never expose non-uniform scale.

### 2.4 Materials

Exactly one material model is supported:

- metallic-roughness PBR
- base color / diffuse response
- roughness
- metallic
- surface normals

The underlying BRDF is:

- Lambert diffuse component
- GGX/Trowbridge-Reitz microfacet normal distribution
- Smith masking-shadowing
- Schlick Fresnel
- metallic workflow F0 derived from metallic/base-color conventions

There is no second material model and no per-material shader permutation system.

Material inputs may be represented as constants and/or textures, but only the following material channels are permitted:

- base color
- roughness
- metallic
- normal

No emissive channel, ambient-occlusion channel, clearcoat, transmission, subsurface, sheen, or other material model is included.

### 2.5 Direct lights

Two light types are supported:

1. **Point light**
   - omnidirectional
   - position
   - color
   - strength

2. **Spot light**
   - position
   - direction
   - color
   - strength
   - cone angle

The attenuation law is physically motivated inverse-square attenuation:

```text
attenuation = 1 / r^2
```

`strength` is the light's scalar radiometric intensity scale. `color` is an RGB radiometric tint. Point lights emit uniformly over the sphere. Spot lights emit within their cone.

The spot angle is the cone's full angular width in the scene file/UI unless explicitly converted internally to a half-angle for the cone test. Outside the cone the contribution is zero. The milestone does not add a separate soft-edged inner/outer cone parameter.

For direct lighting, the implementation evaluates the selected point/spot sample using the material BRDF, surface cosine, light intensity, inverse-square distance attenuation, spot-cone test when applicable, and the selected sample's SDF visibility.

### 2.6 G-buffer

The opaque raster stage produces exactly these logical G-buffer properties:

- depth
- shading normal
- base color
- roughness
- metallic
- motion vectors

Position is **not** stored. View/world position is reconstructed from depth and screen coordinates using the current camera matrices.

The milestone does not add G-buffer channels for:

- ambient occlusion
- emissive
- material IDs
- object IDs
- shading-model IDs
- arbitrary effect masks

Temporal passes may retain history copies of existing G-buffer properties such as depth and normal; those histories are not considered additional G-buffer properties.

### 2.7 Lighting architecture

There are two independent per-pixel reservoirs:

- direct-light reservoir
- VPL/indirect-light reservoir

ReSTIR is conventional per-pixel spatial/temporal reservoir resampling. The earlier experimental 2x2 quad-sharing concept is **not** the reservoir architecture for this milestone.

The expensive SDF soft-visibility evaluation happens after reservoir resampling and therefore does not perform a separate ray march for every candidate light. The renderer evaluates at most one selected direct-light visibility sample and one selected VPL visibility sample per shaded pixel, subject to invalid/zero-contribution cases.

The phrase "ReSTIR ray march" means this selected-sample visibility evaluation, not a per-light loop.

### 2.8 VPL generation

VPLs are regenerated every frame.

The number of stored VPLs is fixed per frame by renderer configuration.

A VPL stores:

```rust
struct Vpl {
    position: Vec3,
    normal: Vec3,
    flux: Vec3,
}
```

The VPL pool is produced by hardware ray tracing from the scene lights.

The VPL generator supports optional Russian-roulette path continuation so that light paths can produce indirect VPLs rather than stopping at the first hit.

The fixed-count model is implemented as **one terminal VPL per launched light path**:

- launch a configured number of light paths
- trace from a point/spot source
- on the first hit, create the path state
- if continuation is disabled, that first hit is the terminal VPL
- otherwise perform BSDF sampling and Russian roulette continuation
- repeat until Russian roulette terminates the path or the configured maximum bounce depth is reached
- the terminal surface hit becomes the stored VPL

The number of stored VPL records remains fixed by the per-frame VPL path budget.

A VPL's `flux` contains the RGB path throughput representing the source energy that reaches that terminal surface after the sampled light path, including the appropriate sampling/pdf/Russian-roulette corrections.

Because the stored VPL representation contains only position, normal, and flux, the VPL is treated as a point-based secondary emitter representation. Direction-dependent information beyond the stored normal/flux representation is not retained as a separate field.

### 2.9 Temporal systems

The renderer contains two distinct temporal mechanisms:

1. **ReSTIR temporal reuse**
   - reuses the previous frame's reservoir/sample representation
   - occurs during reservoir formation

2. **Temporal denoising/reconstruction**
   - filters the already-resolved noisy lighting result using motion vectors and history validity
   - occurs after direct and indirect lighting are combined

They are separate stages and must not be conflated.

VPLs are regenerated every frame, but the previous frame's VPL pool is retained as a temporal-resampling source so the previous VPL selected by the indirect reservoir remains addressable. A VPL temporal candidate is accepted only when the source-light/scene state is compatible with the current lighting generation; otherwise the VPL temporal reservoir is invalidated.

### 2.10 Visibility

The soft-shadow visibility algorithm follows the second soft-shadow method from Inigo Quilez's signed-distance-function shadow reference supplied for the project.

The ray marcher evaluates the minimum SDF-derived shadow factor while marching from the shading point toward the selected light/VPL. It is not a conventional per-light shadow-ray loop.

The SDF is required to be a conservative distance bound suitable for sphere-tracing-style stepping. The runtime must not use an interpolated SDF value as though it were an exact lower bound unless the offline SDF builder has marked the field as conservative.

### 2.11 Editor controls

The application is an interactive renderer/editor, not a headless renderer.

The user can:

- fly the camera using controls similar to Unreal Engine editor controls
- select scene objects by clicking them
- edit selected object transform and parameters
- edit selected light parameters
- see a small GUI panel while an object is selected
- have that panel disappear when the object is deselected

The editor exposes only uniform scale for object scaling.

The GUI uses `egui` integrated through `egui-wgpu` for the milestone. Current `egui-wgpu` provides a renderer specifically for integrating egui with an existing `wgpu` render pass. [4]

---

## 3. High-level architecture

```text
Blender .blend
    |
    | Blender Python CLI exporter
    v
Renderer Scene Manifest (JSON) + mesh/SDF assets
    |
    v
Rust scene loader
    |
    +-------------------- immutable mesh/SDF assets
    |
    +-------------------- mutable object transforms
    |
    +-------------------- mutable lights
    |
    v
GPU Scene Resources
    |
    +--> BLAS per unique mesh
    +--> TLAS for current object transforms
    +--> object/SDF acceleration hierarchy
    |
    v
Per-frame Render Graph
    |
    +--> Depth prepass
    +--> G-buffer
    +--> VPL generation via ray queries
    +--> Direct candidate generation
    +--> Indirect/VPL candidate generation
    +--> Direct temporal/spatial ReSTIR
    +--> Indirect temporal/spatial ReSTIR
    +--> Selected-sample SDF visibility
    +--> Final direct + indirect lighting resolve
    +--> Temporal denoising/reconstruction
    +--> Presentation
```

---

## 4. Renderer frame graph

Each frame is encoded as an ordered sequence of raster and compute operations in a `wgpu::CommandEncoder`. `wgpu` command encoders record render passes, compute passes, and transfers into a command buffer; the implementation should express synchronization through correct resource usage and pass ordering rather than inventing a separate application-level GPU fence model. [2]

### Pass 0 — CPU/editor update

Before GPU command encoding:

1. Poll window/input events.
2. Apply fly-camera motion.
3. Apply selection state.
4. Apply GUI changes to scene objects/lights.
5. Copy current transforms to `previous_transform` after the frame's temporal data has been consumed, so the next frame receives the correct previous-frame state.
6. Detect scene-source changes and update lighting-generation/version counters.

### Pass 1 — GPU scene update

Update dynamic buffers for:

- object transforms
- previous object transforms
- light parameters
- camera matrices
- previous camera matrices
- frame index / random seeds
- renderer configuration if changed

The static buffers and static SDF payloads are not rewritten here.

### Pass 2 — TLAS construction

Create/build the top-level acceleration structure from current object instances.

- one BLAS per unique mesh asset
- one TLAS instance per visible object
- each TLAS instance references a mesh BLAS and carries the current object transform

Since current `wgpu` documents acceleration structures and ray queries as experimental, the renderer must fail cleanly at startup if the required ray-query capability is unavailable. [1]

### Pass 3 — SDF scene acceleration update

Update the scene-level hierarchy used to find nearby object SDFs during shadow marching.

This is separate from the triangle TLAS because the SDF marcher needs distance-field queries rather than triangle intersections.

Each leaf identifies:

- mesh/SDF asset
- object transform
- conservative world-space/object-space bounds

The hierarchy is refit or rebuilt from the current object bounds each frame.

### Pass 4 — Depth prepass

Rasterize all opaque geometry into the depth attachment.

No lighting is performed here.

### Pass 5 — G-buffer

Rasterize opaque geometry and write:

```text
Depth
Normal
BaseColor
Roughness
Metallic
MotionVectors
```

Motion vectors represent the current pixel's motion from previous-frame screen position to current-frame screen position using current/previous camera and object transforms.

### Pass 6 — VPL generation

Dispatch GPU ray-query work to generate the fixed per-frame VPL pool.

Each path:

1. selects a source light
2. samples an emission direction appropriate to the light
3. ray queries the current TLAS
4. evaluates the hit material
5. optionally continues using Russian roulette and BSDF sampling
6. records exactly one terminal VPL

The VPL buffer for the current frame is written separately from the previous-frame VPL buffer.

### Pass 7 — Direct candidate generation

For each valid G-buffer pixel:

- generate the configured number of direct light candidates
- sample active point/spot lights
- evaluate the unshadowed target function
- insert candidates into the direct reservoir

No SDF shadow march occurs for every candidate.

### Pass 8 — Indirect/VPL candidate generation

For each valid G-buffer pixel:

- generate the configured number of VPL candidates from the current VPL pool
- evaluate their unshadowed target function
- insert candidates into the indirect reservoir

### Pass 9 — Direct ReSTIR temporal reuse

For each pixel:

1. reproject the previous direct reservoir using motion vectors
2. validate the history using depth/normal and reservoir validity
3. merge the previous reservoir into the current reservoir using the ReSTIR reservoir merge rules
4. preserve the selected light sample plus its reservoir state

A direct reservoir contains enough state to resample the selected sample at a new target point without recomputing its source-side proposal distribution.

### Pass 10 — Indirect/VPL ReSTIR temporal reuse

For each pixel:

1. reproject the previous indirect reservoir
2. validate the history
3. obtain the referenced VPL from the previous-frame VPL pool
4. reject it if the VPL source state is invalid for the current lighting generation
5. otherwise merge it using the indirect reservoir merge rules

### Pass 11 — Spatial ReSTIR

Spatial reuse is conventional per-pixel reservoir reuse rather than a fixed 2x2 quad-sharing scheme.

The milestone implementation uses a configurable set of nearby pixel reservoirs. The baseline configuration is:

- four spatial neighbors sampled from a small pixel-space radius
- one temporal reservoir candidate
- current-frame candidates generated independently per pixel

Spatial reuse must reject obviously incompatible samples using at least:

- depth difference
- normal difference
- pixel validity

The neighborhood/radius and rejection thresholds are renderer settings, not hard-coded algorithmic assumptions.

### Pass 12 — Selected direct visibility

For the selected direct-light sample of each pixel:

- reconstruct world position from depth
- compute the ray to the point/spot light
- compute `tmax`
- perform the SDF soft-shadow march
- obtain a scalar visibility/soft-shadow factor
- evaluate the final RGB contribution of the selected sample

At most one expensive direct-light visibility evaluation is performed for a valid pixel.

### Pass 13 — Selected indirect/VPL visibility

For the selected VPL sample of each pixel:

- reconstruct world position
- compute the ray to the VPL
- compute `tmax`
- perform the same SDF soft-shadow march
- obtain the visibility factor
- evaluate the final VPL RGB contribution

At most one expensive VPL visibility evaluation is performed for a valid pixel.

### Pass 14 — Lighting resolve

Combine:

```text
Direct selected-sample contribution
+
Indirect selected-VPL contribution
```

into an HDR scene-color texture.

There is no third lighting system in this milestone.

Background pixels produce no direct/VPL lighting contribution and therefore resolve to the configured background value, which defaults to black in the renderer.

### Pass 15 — Temporal denoising/reconstruction

The HDR lighting result is temporally accumulated against the previous resolved lighting history.

Validation uses:

- motion vectors
- reprojected depth
- current depth
- reprojected/current normals
- history-valid flag
- lighting-generation compatibility

On valid history, blend current and historical lighting using a configurable temporal accumulation factor.

On invalid history, use only the current-frame result.

The denoiser is intended to suppress stochastic variation left by VPL generation, ReSTIR resampling, and the SDF visibility evaluation.

### Pass 16 — Presentation

Present the resolved HDR image to the window.

No broad post-processing stack is part of the milestone. The internal lighting buffer remains HDR; display conversion is limited to what is required by the selected output format.

---

## 5. Scene data model

The JSON scene manifest is renderer-oriented and intentionally minimal.

### 5.1 Scene

```json
{
  "version": 1,
  "meshes": [],
  "materials": [],
  "objects": [],
  "lights": [],
  "camera": {}
}
```

### 5.2 Mesh asset

```json
{
  "id": "chair_mesh",
  "geometry": {
    "path": "meshes.bin",
    "vertex_offset": 0,
    "vertex_count": 12000,
    "index_offset": 144000,
    "index_count": 36000
  },
  "sdf": {
    "path": "sdf.bin",
    "offset": 0,
    "resolution": 128,
    "bounds_min": [-1.0, -1.0, -1.0],
    "bounds_max": [1.0, 1.0, 1.0]
  }
}
```

Exact file packing is an implementation detail, but all offsets/counts must be explicit and validated.

Each mesh has exactly one offline SDF resolution. Different meshes may have different resolutions.

Example:

```text
Cube      32^3
Chair    128^3
Detailed 256^3
```

There is no requirement that every mesh consume the same SDF resolution.

### 5.3 Material

```json
{
  "id": "painted_metal",
  "base_color": [0.8, 0.82, 0.85, 1.0],
  "roughness": 0.35,
  "metallic": 0.85,
  "base_color_texture": null,
  "roughness_texture": null,
  "metallic_texture": null,
  "normal_texture": null
}
```

Textures are optional and restricted to the four permitted PBR channels.

### 5.4 Object

```json
{
  "id": "chair_01",
  "mesh": "chair_mesh",
  "material": "painted_metal",
  "transform": {
    "position": [0.0, 0.0, 0.0],
    "rotation": [0.0, 0.0, 0.0, 1.0],
    "scale": 1.0
  }
}
```

The rotation representation in the file is a quaternion. The scale is one scalar value.

### 5.5 Point light

```json
{
  "id": "key_light",
  "type": "point",
  "position": [2.0, 4.0, 1.0],
  "color": [1.0, 0.9, 0.8],
  "strength": 100.0
}
```

### 5.6 Spot light

```json
{
  "id": "fill_spot",
  "type": "spot",
  "position": [-2.0, 3.0, 2.0],
  "direction": [0.0, -1.0, 0.0],
  "color": [0.7, 0.8, 1.0],
  "strength": 100.0,
  "angle_radians": 0.8
}
```

### 5.7 Camera

The camera stores position, orientation, and projection parameters required by the renderer. Current/previous matrices are generated on the CPU each frame.

---

## 6. Blender export pipeline

Blender is the source authoring package. The export process is a non-interactive Python CLI operation.

Canonical invocation shape:

```bash
blender --background scene.blend --python export_scene.py -- --output scene.json
```

Blender documents `--background` plus `--python` specifically for executing automation scripts without the UI. [3]

### Export responsibilities

The exporter must:

1. enumerate renderable mesh objects
2. deduplicate shared mesh data into mesh assets
3. extract vertex/index data
4. extract normals
5. extract material inputs allowed by the renderer
6. extract object transforms
7. enforce/bake the uniform-scale invariant
8. extract point/spot light parameters
9. extract the camera
10. export renderer SDF-resolution metadata per mesh
11. emit the JSON scene manifest
12. emit referenced binary assets as required

### Coordinate and transform rules

- Mesh vertices remain in mesh-local coordinates.
- Object translation/rotation/uniform scale remain object-level data.
- The mesh asset must not include per-instance transforms.
- The exporter must preserve the object origin/pivot semantics used by the transform.

### Non-uniform Blender scale

The exporter must either:

- apply/bake non-uniform scale into mesh-local vertices and set renderer scale to `1.0`, or
- reject the asset with a clear diagnostic.

The renderer never receives a non-uniform scale matrix from scene editing.

### Material color space

Base-color values authored as color data must be converted to the linear representation expected by the lighting shaders. Normal and scalar parameter textures remain in their appropriate non-color-data interpretation.

---

## 7. GPU resource model

The renderer separates immutable asset resources from per-frame dynamic resources.

### 7.1 Immutable resources

- vertex buffers
- index buffers
- mesh metadata
- material static data/textures
- mesh SDF payloads
- BLAS per unique mesh
- static sampler objects
- shader modules
- pipeline layouts
- render/compute pipelines

### 7.2 Dynamic resources

- object transforms
- previous object transforms
- light buffer
- camera/frame uniforms
- TLAS
- SDF scene hierarchy buffers
- current VPL buffer
- previous VPL buffer
- current reservoirs
- previous reservoirs
- current lighting
- previous lighting history
- previous depth/normal history as required for validation

### 7.3 Ping-pong resources

The following use two allocations or equivalent ping-pong state:

```text
Direct reservoir       A/B
Indirect reservoir     A/B
VPL buffer              A/B
Lighting history        A/B
Depth history           A/B
Normal history          A/B
```

The CPU tracks which allocation is current and which is previous. A resource must never be written and read simultaneously within a single pass where the algorithm requires prior contents.

### 7.4 Resource synchronization model

The application does not implement a custom GPU synchronization protocol.

The renderer records ordered passes into `wgpu::CommandEncoder`. Each pass declares resource usage through bind groups/attachments, and the pass sequence encodes the intended dependency graph. `wgpu`'s command encoder is designed to record render passes, compute passes, and transfers into a command buffer. [2]

Explicit fences/semaphores/barriers are not exposed as application-level renderer concepts in this milestone.

---

## 8. G-buffer definition

A robust baseline format selection is:

| Logical value | Initial GPU representation |
|---|---|
| Depth | `Depth32Float` |
| Normal | `Rgba16Float` |
| Base color | `Rgba16Float` |
| Roughness + Metallic | `Rg16Float` |
| Motion vectors | `Rg16Float` |
| HDR SceneColor | `Rgba16Float` |

These formats are intentionally correctness-oriented. The project is experimental, so initial implementation simplicity takes precedence over aggressive packing.

### Position reconstruction

The shader reconstructs view/world position from:

- pixel coordinate
- viewport dimensions
- depth
- inverse projection / inverse view-projection data

No position attachment exists.

### Motion vectors

For an opaque surface sample, the velocity is derived from:

- current object transform
- previous object transform
- current camera matrix
- previous camera matrix
- the surface position represented by the current pixel

The vector is stored in normalized screen-space units suitable for history reprojection.

---

## 9. PBR shading model

The same PBR model is used for direct shading, VPL receiver shading, and stochastic light-path continuation.

### Normalization

All lighting calculations operate in linear space.

Normals are normalized after transformation/interpolation.

### Metallic workflow

Use the conventional metallic interpretation:

```text
F0 = mix(dielectric_F0, base_color, metallic)
```

with dielectric F0 initialized to the conventional approximately 0.04 value.

The diffuse term is reduced as metallic approaches 1.

### Direct lighting

For a visible light sample:

```text
Lo += BRDF(wo, wi)
      * Li
      * max(dot(N, wi), 0)
      * visibility
```

For point/spot lights:

```text
Li = color * strength / r^2
```

with the additional spot-cone mask for spot lights.

The direct light target used by ReSTIR is the **unshadowed** contribution. Visibility is evaluated only after the reservoir has selected its sample.

### VPL lighting

A VPL is treated as a point-based secondary emitter.

A receiver-to-VPL evaluation includes:

- distance-squared falloff
- cosine at the VPL surface
- receiver BRDF
- receiver cosine
- SDF visibility
- the VPL's RGB flux/throughput

A canonical form is:

```text
E_vpl = flux
        * max(dot(N_vpl, -wi), 0)
        / (4 * pi * r^2)
        * visibility

Lo_vpl = BRDF_receiver(wo, wi)
         * E_vpl
         * max(dot(N_receiver, wi), 0)
```

The exact algebraic grouping may be rearranged in shaders without changing the defined physical terms.

---

## 10. VPL generation

### 10.1 Purpose

The VPL generator converts a fixed number of stochastic light paths into a fixed number of secondary lighting samples.

### 10.2 Source-light selection

For each light path:

1. select an active scene light according to the configured light proposal distribution
2. sample an emission direction
3. initialize path throughput from the light's color/strength and the selected emission PDF
4. ray query the TLAS

The default source-light proposal is uniform over active lights unless configuration explicitly selects another documented proposal distribution.

### 10.3 Point-light emission sampling

Point lights emit uniformly over the sphere.

A sampled direction therefore uses a uniform spherical PDF:

```text
pdf_dir = 1 / (4*pi)
```

The total source power represented by the light is derived from its intensity scale.

### 10.4 Spot-light emission sampling

Spot lights emit inside their cone.

The initial implementation samples uniformly over the cone's solid angle:

```text
Omega = 2*pi*(1 - cos(alpha))
```

where `alpha` is the spot half-angle internally used by the shader.

The direction PDF is:

```text
pdf_dir = 1 / Omega
```

Directions outside the spot cone have zero source contribution.

### 10.5 Surface interaction

At a ray hit:

1. recover world-space position and normal
2. recover material parameters
3. evaluate the PBR BSDF
4. generate a candidate continuation direction when continuation is enabled
5. update path throughput using BSDF, cosine, and sampling PDF

### 10.6 Russian roulette

Russian roulette begins after the first surface interaction when GI continuation is enabled.

The survival probability is based on path throughput and is clamped to a configurable minimum/maximum range. When the path survives, throughput is divided by the survival probability so that the estimator remains properly weighted.

The generator also uses a configured maximum bounce depth as a hard termination bound.

A path terminates when either:

- Russian roulette rejects continuation, or
- maximum bounce depth is reached.

The terminal hit becomes the VPL.

### 10.7 Fixed VPL count

The frame allocates exactly `vpl_count` VPL records.

There is one terminal VPL slot for each launched light path in the configured VPL path budget.

A path that misses geometry writes an invalid/empty VPL record and does not contribute to lighting. The fixed buffer size remains unchanged.

---

## 11. ReSTIR reservoirs

The renderer has two independent reservoir domains.

### 11.1 Direct reservoir

Sample domain:

```text
active point lights + active spot lights
```

Sample identifier:

```text
light_id
```

### 11.2 Indirect reservoir

Sample domain:

```text
current-frame VPL pool
```

Sample identifier:

```text
vpl_index
```

A temporal indirect candidate may instead reference the previous-frame VPL pool.

### 11.3 Reservoir contents

A practical GPU representation is:

```rust
struct Reservoir {
    sample_id: u32,
    source_frame: u32,
    weight_sum: f32,
    candidate_count: u32,
    selected_target: f32,
    selected_pdf: f32,
    age: u32,
    valid: u32,
}
```

The exact byte packing must obey WGSL/storage-buffer alignment rules.

`source_frame` distinguishes the current VPL buffer from the previous VPL buffer for indirect temporal reuse.

### 11.4 Initial candidates

The baseline renderer generates:

- 8 direct candidates per pixel
- 8 VPL candidates per pixel

Both counts are runtime configuration parameters and may be tuned without changing the architecture.

Each candidate is sampled from the corresponding proposal distribution and assigned a scalar importance weight from the unshadowed target function.

### 11.5 Target function

The target function is a scalar importance measure of the candidate's unshadowed RGB contribution. Luminance is the baseline scalarization.

Visibility is deliberately excluded from initial candidate target evaluation so the renderer does not perform an SDF march for every candidate.

The final selected-sample estimator multiplies by the separately evaluated SDF visibility.

### 11.6 Weighted reservoir sampling

Candidate insertion follows weighted reservoir sampling:

```text
W <- W + w
M <- M + 1
replace selected sample with probability w / W
```

For reused reservoirs, the source reservoir's accumulated representation is incorporated using the corresponding ReSTIR/resampled-importance weighting relationship between the source target and the current target.

The implementation must retain enough selected-sample target/PDF information to perform that reweighting correctly. Storing only the sample ID is insufficient.

### 11.7 Temporal reuse

Temporal reuse uses motion-vector reprojection.

The reprojected previous pixel must pass:

- screen bounds check
- depth compatibility check
- normal compatibility check
- current/history validity check
- source-sample compatibility check

If any required validation fails, the history candidate is rejected.

### 11.8 Spatial reuse

Spatial reuse uses nearby per-pixel reservoirs.

Baseline:

- four randomly selected nearby pixels
- small configurable neighborhood radius
- depth compatibility threshold
- normal compatibility threshold

Spatial reuse occurs independently for direct and VPL reservoirs.

### 11.9 Reservoir lifecycle

For each frame and each reservoir domain:

```text
new primary candidates
        |
        v
current reservoir
        |
        +--> temporal reuse
        |
        +--> spatial reuse
        |
        v
final reservoir
        |
        v
selected sample visibility + final contribution
```

The final reservoir is the reservoir written into the next frame's previous-reservoir allocation.

---

## 12. Temporal denoising/reconstruction

ReSTIR reuse is not the denoiser.

The denoiser consumes the final resolved HDR lighting:

```text
Direct + Indirect
       |
       v
current noisy HDR lighting
       |
       +---- previous HDR lighting history
       |
       v
Temporal reconstruction
       |
       v
resolved HDR output
```

### 12.1 Reprojection

Use the G-buffer motion vector to locate the previous-frame history sample.

### 12.2 History validation

Reject history when any of the following is true:

- reprojected coordinate is outside the previous frame
- no previous history exists
- current depth and history depth disagree beyond the configured threshold
- current normal and history normal disagree beyond the configured threshold
- lighting generation is incompatible

### 12.3 Accumulation

Use a configurable history weight. The baseline uses a stable exponential accumulation model with a clamp on maximum effective history length.

The denoiser must avoid unlimited accumulation.

### 12.4 Firefly/outlier handling

The temporal stage must clamp historical radiance against a neighborhood-derived or current-frame-derived luminance bound before accumulation. The exact clamp shape is an implementation detail, but historical samples must not be allowed to dominate indefinitely when a new lighting sample becomes a strong outlier.

---

## 13. SDF representation and construction

### 13.1 Asset model

Every unique mesh asset has its own precomputed signed-distance volume.

The SDF is stored in mesh-local coordinates.

Example:

```text
Cube mesh     -> 32^3 SDF
Chair mesh    -> 128^3 SDF
Complex mesh  -> 256^3 SDF
```

The resolution is selected independently for each mesh asset.

### 13.2 Storage

The first implementation stores conservative SDF samples as 32-bit floats in a GPU-accessible storage buffer.

Using a storage buffer instead of one texture object per mesh makes variable per-mesh resolutions straightforward and avoids requiring a bindless/binding-array design merely to support different SDF sizes.

Mesh metadata contains:

```rust
struct SdfMeta {
    data_offset: u32,
    resolution: u32,
    bounds_min: Vec3,
    bounds_max: Vec3,
}
```

### 13.3 Signed-distance requirements

The offline SDF generator must produce a valid signed distance representation for the mesh.

The mesh must be suitable for reliable inside/outside classification. The asset builder must validate manifoldness/closure or use a robust signed-distance classification method that produces consistent sign for the intended surface volume.

### 13.4 Conservative stepping invariant

The runtime ray marcher relies on the SDF value as a step bound. Therefore the stored/query distance must be conservative.

The field/query implementation must account for:

- voxel sampling error
- trilinear interpolation error
- floating-point error
- transformation into object-local space

A conservative safety factor may be applied to the final returned distance, but the runtime must never deliberately enlarge a conservative distance into an unsafe step.

### 13.5 Different mesh resolutions

The chair does not need to use the cube's SDF resolution.

The SDF asset builder must treat resolution as a mesh-level quality parameter. Large/simple meshes may use a coarse SDF; small/complex meshes may use a finer SDF.

### 13.6 Moving objects

The SDF data never changes when an object is moved.

At runtime, the shadow ray is transformed into the object's local SDF coordinate system using the inverse object transform.

Because scale is uniform, the world/local distance conversion is a single scalar correction.

### 13.7 Scene SDF hierarchy

The per-mesh volumes are accompanied by a scene-level broad-phase hierarchy over the objects' conservative bounds.

The hierarchy exists to avoid querying every mesh SDF at every ray-march step.

It stores:

- node AABB
- child references or leaf reference
- mesh/SDF ID for leaves
- object ID/transform reference

The hierarchy is updated from moving object bounds each frame, while the leaf SDF data remains immutable.

---

## 14. SDF soft-shadow marcher

The visibility pass evaluates the selected light/VPL with the project's specified SDF soft-shadow method.

### 14.1 Inputs

```text
ray_origin
ray_direction
t_min
t_max
soft_shadow_k
scene_sdf_hierarchy
```

### 14.2 March loop

At each step:

1. query the scene SDF for the minimum conservative distance to geometry along the ray
2. update the soft-shadow accumulator using the SDF-derived clearance and traveled distance
3. terminate with full occlusion when the geometry hit threshold is reached
4. advance by the conservative distance
5. terminate as visible when `t >= t_max`

### 14.3 Reference method

The implementation target is the second method from:

https://iquilezles.org/articles/rmshadows/

The implementation may be written differently from the reference code, but its behavior must preserve the intended SDF soft-penumbra approximation.

### 14.4 Ray start

The visibility ray starts from a surface-offset point to prevent self-shadowing. The offset is derived from the SDF/normal scale and the current world-space scene scale.

### 14.5 Point and spot light endpoints

For a direct light:

```text
origin = shaded_surface + normal * epsilon
target = point/spot light position
tmax = distance(origin, target) - epsilon
```

For a spot light, the cone test occurs before the expensive SDF march.

### 14.6 VPL endpoint

For a VPL:

```text
origin = shaded_surface + normal * epsilon
target = VPL position
tmax = distance(origin, target) - epsilon
```

The same SDF visibility algorithm is used.

---

## 15. Ray tracing acceleration structures

### 15.1 BLAS

Create one BLAS for each unique mesh asset.

BLAS geometry contains the immutable triangle mesh.

A mesh that is instanced by multiple scene objects does not require multiple BLAS copies.

### 15.2 TLAS

Create one TLAS containing all visible object instances.

Each instance contains:

- reference to mesh BLAS
- current object transform
- instance identity needed by shader-side ray-query handling

Because object transforms can change while the mesh does not, the BLAS remains static while the TLAS reflects current object placement.

Current `wgpu` documentation identifies BLAS as device-specific geometry acceleration structures and TLAS as the structure combining BLAS references with transforms. [1][5]

### 15.3 Object motion

A moving object changes only dynamic instance state and the scene-level SDF bounds hierarchy.

No vertex upload is required for translation/rotation/uniform-scale editing.

No SDF rebuild is performed for transform changes.

---

## 16. GPU buffer layouts

The following structures define the logical shader-visible records. Exact padding may be introduced to satisfy WGSL/storage layout constraints.

### 16.1 Object

```rust
struct GpuObject {
    current_model: Mat4,
    previous_model: Mat4,
    normal_matrix: Mat4,
    mesh_id: u32,
    material_id: u32,
    flags: u32,
    _pad: u32,
}
```

### 16.2 Light

```rust
struct GpuLight {
    position: Vec3,
    strength: f32,
    direction: Vec3,
    angle_radians: f32,
    color: Vec3,
    light_type: u32,
}
```

### 16.3 Material

```rust
struct GpuMaterial {
    base_color: Vec4,
    roughness: f32,
    metallic: f32,
    base_color_texture: u32,
    roughness_texture: u32,
    metallic_texture: u32,
    normal_texture: u32,
    _pad: u32,
}
```

### 16.4 VPL

```rust
struct GpuVpl {
    position: Vec3,
    _pad0: f32,
    normal: Vec3,
    _pad1: f32,
    flux: Vec3,
    _pad2: f32,
}
```

### 16.5 Reservoir

```rust
struct GpuReservoir {
    sample_id: u32,
    source_frame: u32,
    weight_sum: f32,
    candidate_count: u32,
    selected_target: f32,
    selected_pdf: f32,
    age: u32,
    valid: u32,
}
```

### 16.6 Camera/frame constants

```rust
struct FrameUniforms {
    view: Mat4,
    projection: Mat4,
    view_projection: Mat4,
    inverse_view_projection: Mat4,
    previous_view_projection: Mat4,
    camera_position: Vec3,
    frame_index: u32,
    viewport_size: Vec2,
    inverse_viewport_size: Vec2,
    delta_time: f32,
    lighting_generation: u32,
}
```

---

## 17. Shader organization

WGSL shader modules should be organized around the renderer stages rather than material permutations.

Suggested layout:

```text
shaders/
  common.wgsl
  camera.wgsl
  pbr.wgsl
  depth.wgsl
  gbuffer.wgsl
  vpl_generate.wgsl
  restir_direct.wgsl
  restir_vpl.wgsl
  sdf.wgsl
  visibility.wgsl
  lighting_resolve.wgsl
  temporal_denoise.wgsl
  present.wgsl
```

Shared code belongs in `common.wgsl`, `camera.wgsl`, `pbr.wgsl`, and `sdf.wgsl` as appropriate.

There is no compile-time permutation per material type because the renderer has exactly one material model.

Debug options may exist as ordinary renderer settings or dedicated debug pipelines without creating material-model permutations.

---

## 18. Application/editor architecture

### 18.1 Window/input layer

Use a native window/event system compatible with `wgpu` and egui integration.

The milestone application uses:

- `winit` for window/input events
- `egui` for GUI state
- `egui-wgpu` for rendering the GUI through the same GPU device

### 18.2 Fly camera

The baseline editor controls are:

- hold right mouse button: enter freelook
- mouse motion while freelook: rotate camera
- `W`: forward
- `A`: left
- `S`: backward
- `D`: right
- `Q`: move down
- `E`: move up
- `Shift`: movement-speed boost
- mouse wheel: adjust movement speed

The camera remains free-flying rather than orbiting a selected object.

When the GUI claims keyboard or pointer input, the camera controller must not also consume that input.

### 18.3 Selection

Left-click performs object selection.

The selection ray originates from the current camera through the clicked pixel.

Selection may reuse the renderer's TLAS/ray-query infrastructure. The ray query returns the intersected scene instance identity. This keeps editor selection independent of the main G-buffer and avoids adding a persistent object-ID G-buffer attachment.

### 18.4 Selection state

```rust
enum Selection {
    None,
    Object(ObjectId),
    Light(LightId),
}
```

The selected entity is the sole source for the transient inspector GUI.

### 18.5 Inspector GUI

When an object or light is selected, display a compact egui window.

Object fields:

- position X/Y/Z
- rotation X/Y/Z or equivalent Euler editor representation converted to/from the stored quaternion
- uniform scale
- material base color
- roughness
- metallic

Light fields:

- position
- direction for spot lights
- color
- strength
- angle for spot lights

When deselected:

- clear selection
- hide inspector window

The GUI edits CPU-side scene data. The renderer consumes the updated scene during the next frame update.

### 18.6 No transform gizmo requirement

A 3D translation/rotation gizmo is not required. Numeric controls in the inspector are the authoritative editing interface for this milestone.

---

## 19. Lighting-generation/version invalidation

Temporal algorithms depend on whether a reused sample still belongs to the same source distribution.

Maintain a `lighting_generation` counter.

Increment it when any change alters the light transport source distribution, including:

- light position
- light direction
- light color
- light strength
- spotlight angle
- object transform
- object material values
- material textures that affect surface scattering
- mesh/SDF asset reload

Camera motion alone does not increment `lighting_generation`.

Direct reservoirs may still re-evaluate a stable light sample after receiver motion, but a source-side light change invalidates the old direct sample distribution.

VPL temporal reuse requires matching lighting generation because the VPL source paths depend on scene geometry/materials/lights. If the generation differs, the previous VPL reservoir candidate must be rejected.

The temporal denoiser may still use an older lighting history after a camera-only change, provided depth/normal/motion validation succeeds. If lighting generation changes, denoising history must be reduced or invalidated according to the history policy so stale lighting does not persist indefinitely.

---

## 20. CPU-side Rust module plan

Suggested organization:

```text
src/
  main.rs
  app.rs

  scene/
    mod.rs
    json.rs
    loader.rs
    transform.rs
    material.rs
    light.rs
    camera.rs

  asset/
    mod.rs
    geometry.rs
    sdf.rs
    binary.rs

  renderer/
    mod.rs
    device.rs
    pipelines.rs
    resources.rs
    bind_groups.rs
    scene_gpu.rs
    acceleration.rs
    frame.rs

    passes/
      depth.rs
      gbuffer.rs
      vpl.rs
      restir_direct.rs
      restir_vpl.rs
      visibility.rs
      lighting.rs
      temporal.rs
      present.rs

  editor/
    mod.rs
    camera_controller.rs
    selection.rs
    inspector.rs
    input.rs

  shaders/
    loader.rs
```

This organization is descriptive rather than mandatory; the externally visible architecture is the frame graph and resource model defined above.

---

## 21. Configuration

Renderer configuration is data-driven and should be grouped into a dedicated settings object.

Required settings include:

```rust
struct RendererSettings {
    render_width: u32,
    render_height: u32,

    vpl_count: u32,
    vpl_max_bounces: u32,
    vpl_rr_min_survival: f32,
    vpl_rr_max_survival: f32,

    restir_direct_candidates: u32,
    restir_vpl_candidates: u32,
    restir_spatial_neighbors: u32,
    restir_spatial_radius: u32,

    temporal_history_weight: f32,
    temporal_max_history: u32,

    sdf_max_steps: u32,
    sdf_hit_epsilon: f32,
    sdf_soft_shadow_k: f32,
}
```

Baseline values are:

```text
VPL count:                 project-configurable
VPL max bounces:           project-configurable
Direct candidates:         8
VPL candidates:            8
Spatial neighbors:         4
```

The VPL count is intentionally not hard-coded; an example experiment may set it to 100.

The remaining numeric values should be treated as startup configuration rather than source-level constants.

---

## 22. Error handling and validation

The renderer must fail clearly rather than silently falling back to unsupported behavior.

Startup validation must check:

- required `wgpu` features are available
- ray-query capability is available
- output surface can be configured
- all referenced mesh IDs exist
- all referenced material IDs exist
- all referenced light/object IDs are unique
- mesh binary ranges are valid
- SDF binary ranges are valid
- SDF resolution is valid
- object scales are finite and uniform
- material roughness is in a valid range
- metallic is in a valid range
- light color/strength are finite
- spot direction is nonzero and normalizable
- spot angle is within a valid range

The asset loader must produce actionable diagnostics that identify the source JSON path and entity ID when possible.

---

## 23. Milestone completion criteria

The milestone is complete only when all of the following are true.

### Asset pipeline

- A Blender `.blend` file can be exported through Blender's Python CLI.
- The exporter produces the renderer JSON manifest and referenced assets.
- Meshes and transforms load correctly in Rust.
- Per-mesh SDF resolutions can differ.
- SDF assets are static after creation.
- Non-uniform runtime scaling is impossible through the editor/API.

### Raster renderer

- Depth prepass works.
- G-buffer contains exactly the defined logical properties.
- Position is reconstructed from depth.
- Motion vectors are generated from current/previous transforms and camera state.
- Metallic-roughness PBR shading is functional.

### Direct lighting

- Point lights work.
- Spot lights work.
- Strength, color, direction, and angle are editable.
- Inverse-square attenuation is used.
- Direct ReSTIR reservoir exists per pixel.
- Temporal and spatial reuse work.
- Selected direct samples use SDF soft visibility.
- The implementation does not trace an SDF visibility ray for every light candidate.

### VPL/indirect lighting

- VPLs are regenerated every frame.
- The number of stored VPLs is fixed by configuration.
- VPLs are generated using hardware ray queries.
- VPLs store position, normal, and flux.
- Optional Russian-roulette continuation is implemented.
- Indirect/VPL ReSTIR reservoir exists per pixel.
- Temporal and spatial reuse work.
- Previous-frame VPLs are retained for temporal indirect reuse.
- Selected VPLs use the SDF soft-visibility marcher.

### SDF visibility

- Every renderable mesh can have an independently selected SDF resolution.
- Runtime SDF data remains unchanged while object transforms change.
- Shadow rays transform into object-local SDF space.
- Uniform scaling is handled correctly.
- Scene-level object bounds acceleration is updated as objects move.
- The SDF field/query is conservative enough for the marcher to avoid unsafe sphere-tracing steps.
- The selected soft-shadow implementation follows the project's specified second Quilez method.

### Temporal reconstruction

- ReSTIR temporal reuse is distinct from temporal denoising.
- Motion-vector reprojection works.
- Depth/normal disocclusion tests work.
- History invalidation responds to incompatible lighting-generation changes.
- The final combined lighting is temporally reconstructed.

### Editor

- Unreal-style fly controls work.
- Object selection works.
- Light selection works.
- A compact inspector appears when selected.
- Object position, rotation, uniform scale, and material parameters are editable.
- Light position, direction, strength, color, and spotlight angle are editable where applicable.
- Inspector disappears on deselection.
- GUI input does not accidentally drive the fly camera.

### Architecture

- The renderer no longer contains a light-depth-map pass as part of the required lighting pipeline.
- There is no per-light shadow-ray loop for ReSTIR candidate evaluation.
- There is no second material/shading model.
- There is no non-uniform object scale.
- There is no requirement for a position G-buffer.
- GPU work is represented as ordered `wgpu` passes and resource dependencies rather than a custom manual synchronization framework.

---

## 24. Explicitly out of scope for this milestone

The following are not required for milestone completion:

- area lights
- directional/sun lights
- emissive materials
- ambient occlusion
- environment-light/sky-light systems
- transparency/transmission
- subsurface scattering
- multiple material models
- non-uniform object scale
- dynamic mesh deformation
- runtime SDF regeneration
- shadow maps
- cascaded shadow maps
- broad post-processing stacks
- bloom
- motion blur
- depth of field
- TAA as a separate post-process distinct from the defined temporal reconstruction
- browser/WebGPU ray tracing
- production asset packaging/editor tooling beyond the defined Blender exporter

These exclusions are intentional and preserve the experimental focus of the renderer.

---

## 25. Reference implementation sequence

The implementation should be constructed in the following dependency order because each stage establishes a resource contract needed by the next stage.

```text
1. Rust/wgpu window + device initialization
2. Blender JSON manifest + geometry loader
3. Basic raster pipeline
4. Depth prepass
5. G-buffer
6. Camera/object motion vectors
7. PBR material evaluation
8. Point/spot direct lighting without ReSTIR
9. Per-mesh offline SDF generation + runtime SDF query
10. SDF soft-shadow visibility
11. BLAS/TLAS + ray-query capability
12. Per-frame VPL generation
13. VPL lighting without ReSTIR
14. Direct ReSTIR reservoirs
15. VPL ReSTIR reservoirs
16. Temporal reservoir reuse
17. Spatial reservoir reuse
18. Selected-sample visibility integration
19. Temporal denoising/reconstruction
20. Object/light selection
21. egui inspector
22. Fly camera/editor controls
23. Final validation against the completion criteria
```

This order is a dependency order, not a feature wishlist. Every stage is part of the milestone.

---

## 26. Final source-of-truth summary

The renderer is a native Rust/`wgpu` experimental renderer whose central lighting path is:

```text
             RASTER
               |
               v
        Depth + G-buffer
               |
       +-------+--------+
       |                |
       v                v
   Direct lights      VPL paths
       |                |
       |          ray query + RR
       |                |
       |                v
       |              VPLs
       |                |
       v                v
 Direct ReSTIR      VPL ReSTIR
       |                |
       +-------+--------+
               |
               v
      Selected samples
               |
               v
       SDF soft visibility
               |
               v
   Direct + indirect HDR
               |
               v
   Temporal reconstruction
               |
               v
          Presentation
```

The immutable asset contract is:

```text
Mesh geometry + mesh-local SDF + material asset + BLAS
```

The dynamic scene contract is:

```text
Object transform + previous transform + light parameters + camera state
```

The transform invariant is:

```text
translation + rotation + uniform scale
```

The lighting invariant is:

```text
one direct reservoir per pixel
one VPL/indirect reservoir per pixel
at most one selected direct visibility evaluation
at most one selected VPL visibility evaluation
```

The temporal invariant is:

```text
ReSTIR temporal reuse != temporal denoising
```

The SDF invariant is:

```text
mesh-local + immutable + conservative
```

The implementation milestone is considered satisfied only when the complete system described above operates together as one interactive application.

---

## 27. References

**[1] `wgpu` current API documentation — device ray-tracing acceleration structures and experimental ray query support**  
https://docs.rs/wgpu/latest/wgpu/struct.Device.html  
https://docs.rs/wgpu/latest/wgpu/struct.Features.html  
Current documentation indicates that `EXPERIMENTAL_RAY_QUERY` enables ray-tracing queries in shaders and is currently native-only/Vulkan-only. 

**[2] `wgpu` command encoder documentation**  
https://docs.rs/wgpu/latest/wgpu/struct.CommandEncoder.html  
`CommandEncoder` records render passes, compute passes, and transfers into a command buffer.

**[3] Blender Python automation / background execution**  
https://docs.blender.org/api/main/info_tips_and_tricks.html  
Blender documents `blender --background --python myscript.py` for command-line Python execution without the interactive UI.

**[4] `egui-wgpu` current API documentation**  
https://docs.rs/egui-wgpu/latest/egui_wgpu/  
Provides an egui renderer for integration with a `wgpu` render pass.

**[5] `wgpu` BLAS documentation**  
https://docs.rs/wgpu/latest/wgpu/struct.Blas.html  
Documents BLAS as geometry acceleration structures combined into TLAS instances with transforms.

**[6] ReSTIR — Spatiotemporal Reservoir Resampling for Real-Time Ray Tracing with Dynamic Direct Lighting**  
Bitterli, Wyman, Pharr, Shirley, Lefohn, Jarosz, SIGGRAPH 2020.  
https://research.nvidia.com/publication/2020-07_spatiotemporal-reservoir-resampling-real-time-ray-tracing-dynamic-direct  
Defines the reservoir-based direct-light approach using candidate resampling and spatial/temporal reuse.

**[7] ReSTIR GI — Path Resampling for Real-Time Path Tracing**  
Ouyang, Liu, Kettunen, Pharr, Pantaleoni, 2021.  
https://research.nvidia.com/publication/2021-06_restir-gi-path-resampling-real-time-path-tracing  
Establishes reservoir-based spatiotemporal reuse for indirect lighting/path samples.

**[8] Generalized Resampled Importance Sampling: Foundations of ReSTIR**  
Lin, Kettunen, Bitterli, Pantaleoni, Yuksel, Wyman, SIGGRAPH 2022.  
https://research.nvidia.com/publication/2022-07_generalized-resampled-importance-sampling-foundations-restir  
Provides the generalized theoretical foundation for resampled importance sampling with reused/correlated samples.

**[9] SDF soft-shadow reference supplied for this renderer**  
Inigo Quilez — Rendering Soft Shadows Using Ray Marching / Signed Distance Functions.  
https://iquilezles.org/articles/rmshadows/  
The second soft-shadow method from this reference is the behavioral target for the renderer's SDF visibility pass.

---

## 28. Change-control rule

Any future change to this renderer that alters:

- the material model
- the supported light types
- the G-buffer contract
- the reservoir domains
- the VPL representation
- the SDF transform invariant
- the temporal architecture
- the editor transform capabilities
- the scene JSON contract
- the ray-tracing backend requirement

must first update this document and increment its specification version.

Code that disagrees with this document is considered non-conforming to the milestone.
