use anyhow::{Context, Result, bail, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}
impl Transform {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            self.rotation.normalize(),
            self.position,
        )
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.position.is_finite()
                && self.rotation.is_finite()
                && self.rotation.length_squared().is_finite()
                && self.rotation.length_squared() > 1e-8,
            "invalid position/quaternion"
        );
        ensure!(
            self.scale.is_finite() && self.scale > 1e-5,
            "scale must be a positive finite uniform scalar"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Geometry {
    pub path: String,
    pub vertex_offset: u64,
    pub vertex_count: u32,
    pub index_offset: u64,
    pub index_count: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sdf {
    pub path: String,
    pub offset: u64,
    pub resolution: u32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub conservative: bool,
    pub query: String,
    pub error_bound: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Mesh {
    pub id: String,
    pub geometry: Geometry,
    pub sdf: Sdf,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Material {
    pub id: String,
    pub base_color: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    pub base_color_texture: Option<String>,
    pub roughness_texture: Option<String>,
    pub metallic_texture: Option<String>,
    pub normal_texture: Option<String>,
    #[serde(default = "yes")]
    pub base_color_srgb: bool,
    #[serde(default = "one")]
    pub normal_strength: f32,
    #[serde(default)]
    pub texture_channels: [u32; 4],
}
fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Object {
    pub id: String,
    pub mesh: String,
    pub material: String,
    pub transform: Transform,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Light {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub position: Vec3,
    pub color: Vec3,
    pub strength: f32,
    #[serde(default)]
    pub direction: Vec3,
    #[serde(default)]
    pub angle_radians: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Camera {
    pub position: Vec3,
    pub rotation: Quat,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}
impl Camera {
    pub fn matrix(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(
            self.fov_y_radians,
            aspect,
            self.near,
            self.far,
        ) * Mat4::from_rotation_translation(self.rotation.normalize(), self.position).inverse()
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub objects: Vec<Object>,
    pub lights: Vec<Light>,
    pub camera: Camera,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 4],
    pub normal: [f32; 4],
    pub tangent: [f32; 4],
    pub uv: [f32; 4],
}
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub sdf: Vec<f32>,
}
pub struct Scene {
    pub manifest: Manifest,
    pub root: PathBuf,
    pub data: Vec<MeshData>,
    pub mesh_ids: HashMap<String, usize>,
    pub material_ids: HashMap<String, usize>,
    pub generation: u32,
}
fn range(bytes: &[u8], offset: u64, count: u64, stride: u64) -> Result<&[u8]> {
    let end = count
        .checked_mul(stride)
        .and_then(|n| offset.checked_add(n))
        .context("binary range overflow")?;
    ensure!(
        end <= bytes.len() as u64,
        "binary range [{offset},{end}) exceeds {} bytes",
        bytes.len()
    );
    Ok(&bytes[offset as usize..end as usize])
}
fn ids<'a>(items: impl Iterator<Item = &'a str>, kind: &str) -> Result<()> {
    let mut set = HashSet::new();
    for id in items {
        ensure!(
            !id.is_empty() && set.insert(id),
            "duplicate/empty {kind} ID: {id}"
        );
    }
    Ok(())
}
impl Scene {
    pub fn load(path: &Path) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(
            &fs::read(path).with_context(|| format!("reading {}", path.display()))?,
        )
        .with_context(|| format!("parsing {}", path.display()))?;
        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut scene = Self {
            manifest,
            root,
            data: vec![],
            mesh_ids: HashMap::new(),
            material_ids: HashMap::new(),
            generation: 1,
        };
        scene
            .validate()
            .with_context(|| format!("scene {}", path.display()))?;
        for (i, mesh) in scene.manifest.meshes.iter().enumerate() {
            scene.mesh_ids.insert(mesh.id.clone(), i);
            let load = || -> Result<MeshData> {
                let g = &mesh.geometry;
                let bytes = fs::read(scene.root.join(&g.path)).with_context(|| g.path.clone())?;
                let vertices = range(&bytes, g.vertex_offset, g.vertex_count as u64, 64)?
                    .as_chunks::<64>()
                    .0
                    .iter()
                    .map(|x| bytemuck::pod_read_unaligned(x))
                    .collect::<Vec<Vertex>>();
                let indices = range(&bytes, g.index_offset, g.index_count as u64, 4)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|x| u32::from_le_bytes(*x))
                    .collect::<Vec<_>>();
                ensure!(
                    g.vertex_count > 0 && g.index_count > 0 && g.index_count % 3 == 0,
                    "geometry must have triangles"
                );
                ensure!(
                    indices.iter().all(|&j| j < g.vertex_count),
                    "index outside vertex range"
                );
                ensure!(
                    vertices.iter().all(|v| v
                        .position
                        .iter()
                        .chain(&v.normal)
                        .chain(&v.tangent)
                        .chain(&v.uv)
                        .all(|x| x.is_finite())),
                    "nonfinite vertex"
                );
                let bytes = fs::read(scene.root.join(&mesh.sdf.path))
                    .with_context(|| mesh.sdf.path.clone())?;
                let sdf = range(
                    &bytes,
                    mesh.sdf.offset,
                    (mesh.sdf.resolution as u64).pow(3),
                    4,
                )?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|x| f32::from_le_bytes(*x))
                .collect::<Vec<_>>();
                ensure!(sdf.iter().all(|v| v.is_finite()), "nonfinite SDF");
                Ok(MeshData {
                    vertices,
                    indices,
                    sdf,
                })
            };
            scene
                .data
                .push(load().with_context(|| format!("mesh {}", mesh.id))?);
        }
        for (i, m) in scene.manifest.materials.iter().enumerate() {
            scene.material_ids.insert(m.id.clone(), i);
        }
        Ok(scene)
    }
    pub fn validate(&self) -> Result<()> {
        let m = &self.manifest;
        ensure!(m.version == 1, "unsupported scene version {}", m.version);
        ids(m.meshes.iter().map(|x| x.id.as_str()), "mesh")?;
        ids(m.materials.iter().map(|x| x.id.as_str()), "material")?;
        ids(
            m.objects
                .iter()
                .map(|x| x.id.as_str())
                .chain(m.lights.iter().map(|x| x.id.as_str())),
            "object/light",
        )?;
        ensure!(!m.objects.is_empty(), "scene has no objects");
        for mesh in &m.meshes {
            let s = &mesh.sdf;
            ensure!(
                (4..=256).contains(&s.resolution)
                    && s.bounds_min.is_finite()
                    && s.bounds_max.is_finite()
                    && s.bounds_max.cmpgt(s.bounds_min).all(),
                "mesh {}: invalid SDF bounds/resolution",
                mesh.id
            );
            ensure!(
                s.conservative
                    && s.query == "trilinear_bound"
                    && s.error_bound.is_finite()
                    && s.error_bound >= 0.,
                "mesh {}: SDF must be conservative with trilinear_bound query",
                mesh.id
            );
        }
        for mat in &m.materials {
            ensure!(
                mat.base_color.iter().all(|x| x.is_finite() && *x >= 0.)
                    && mat.roughness.is_finite()
                    && (0.0..=1.0).contains(&mat.roughness)
                    && mat.metallic.is_finite()
                    && (0.0..=1.0).contains(&mat.metallic),
                "material {}: invalid PBR values",
                mat.id
            );
            ensure!(
                mat.normal_strength.is_finite()
                    && mat.normal_strength >= 0.
                    && mat.texture_channels.iter().all(|&c| c < 4),
                "material {}: invalid texture settings",
                mat.id
            );
            for tex in [
                &mat.base_color_texture,
                &mat.roughness_texture,
                &mat.metallic_texture,
                &mat.normal_texture,
            ]
            .into_iter()
            .flatten()
            {
                ensure!(
                    self.root.join(tex).is_file(),
                    "material {}: missing texture {tex}",
                    mat.id
                );
            }
        }
        for obj in &m.objects {
            obj.transform
                .validate()
                .with_context(|| format!("object {}", obj.id))?;
            ensure!(
                m.meshes.iter().any(|x| x.id == obj.mesh),
                "object {}: missing mesh {}",
                obj.id,
                obj.mesh
            );
            ensure!(
                m.materials.iter().any(|x| x.id == obj.material),
                "object {}: missing material {}",
                obj.id,
                obj.material
            );
        }
        for l in &m.lights {
            ensure!(
                l.position.is_finite()
                    && l.color.is_finite()
                    && l.color.min_element() >= 0.
                    && l.strength.is_finite()
                    && l.strength >= 0.,
                "light {}: invalid values",
                l.id
            );
            match l.kind.as_str() {
                "point" => {}
                "spot" => ensure!(
                    l.direction.is_finite()
                        && l.direction.length_squared().is_finite()
                        && l.direction.length_squared() > 1e-8
                        && l.angle_radians.is_finite()
                        && l.angle_radians > 0.
                        && l.angle_radians < std::f32::consts::PI,
                    "light {}: invalid spot cone/direction",
                    l.id
                ),
                _ => bail!("light {}: unsupported type {}", l.id, l.kind),
            }
        }
        let c = &m.camera;
        ensure!(
            c.position.is_finite()
                && c.rotation.is_finite()
                && c.rotation.length_squared().is_finite()
                && c.rotation.length_squared() > 1e-8
                && c.fov_y_radians > 0.
                && c.fov_y_radians < 3.13
                && c.near > 0.
                && c.far.is_finite()
                && c.far > c.near,
            "invalid camera"
        );
        Ok(())
    }
    pub fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_vec_pretty(&self.manifest)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_scale_validates_and_roundtrips() {
        let mut t = Transform {
            position: Vec3::new(1., 2., 3.),
            rotation: Quat::from_rotation_z(0.6),
            scale: 2.,
        };
        t.validate().unwrap();
        let p = Vec3::new(-2., 0.1, 3.);
        assert!(
            t.matrix()
                .inverse()
                .transform_point3(t.matrix().transform_point3(p))
                .distance(p)
                < 1e-5
        );
        t.scale = f32::NAN;
        assert!(t.validate().is_err());
        t.scale = -1.;
        assert!(t.validate().is_err());
        assert!(
            serde_json::from_str::<Transform>(
                r#"{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,2,1]}"#
            )
            .is_err()
        );
    }
    #[test]
    fn binary_ranges_reject_overflow_and_oob() {
        let b = [0; 16];
        assert!(range(&b, 8, 2, 4).is_ok());
        assert!(range(&b, 8, 3, 4).is_err());
        assert!(range(&b, u64::MAX, 1, 4).is_err());
    }
    #[test]
    fn depth_reconstruction_roundtrip() {
        let c = Camera {
            position: Vec3::new(4., -3., 2.),
            rotation: Quat::from_rotation_z(0.3),
            fov_y_radians: 1.,
            near: 0.1,
            far: 100.,
        };
        let vp = c.matrix(1.6);
        let p = c.position + c.rotation * Vec3::new(0.2, 0.1, -4.);
        let h = vp * p.extend(1.);
        let ndc = h / h.w;
        let w = vp.inverse() * ndc;
        assert!((w.truncate() / w.w).distance(p) < 1e-4);
    }
    #[test]
    fn scene_validation_rejects_invalid_entities_with_diagnostics() {
        let mut scene = Scene::load(Path::new("assets/test_scene/scene.json")).unwrap();
        let original = scene.manifest.clone();
        scene.manifest.objects[0].mesh = "missing_mesh".into();
        assert!(
            scene
                .validate()
                .unwrap_err()
                .to_string()
                .contains("missing mesh")
        );
        scene.manifest = original.clone();
        scene.manifest.objects[0].transform.scale = -1.;
        assert!(scene.validate().is_err());
        scene.manifest = original.clone();
        scene.manifest.materials[0].roughness = f32::NAN;
        assert!(
            scene
                .validate()
                .unwrap_err()
                .to_string()
                .contains("material")
        );
        scene.manifest = original.clone();
        scene.manifest.lights[0].kind = "spot".into();
        scene.manifest.lights[0].direction = Vec3::ZERO;
        assert!(scene.validate().unwrap_err().to_string().contains("spot"));
        scene.manifest = original.clone();
        scene.manifest.meshes[0].sdf.conservative = false;
        assert!(
            scene
                .validate()
                .unwrap_err()
                .to_string()
                .contains("conservative")
        );
        scene.manifest = original.clone();
        scene.manifest.objects[1].id = scene.manifest.objects[0].id.clone();
        assert!(
            scene
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        scene.manifest = original;
        scene.manifest.camera.rotation = Quat::from_xyzw(f32::MAX, f32::MAX, f32::MAX, f32::MAX);
        assert!(scene.validate().is_err());
    }
}
