//! CPU reference kernels for shader invariants and deterministic tests.
use glam::Vec3;
#[derive(Default, Clone, Copy)]
pub struct Reservoir {
    pub selected: u32,
    pub sum: f32,
    pub count: u32,
    pub target: f32,
}
impl Reservoir {
    pub fn insert(&mut self, id: u32, target: f32, pdf: f32, random: f32) {
        self.merge(id, target, target / pdf, 1, random);
    }
    pub fn merge(&mut self, id: u32, target: f32, weight: f32, count: u32, random: f32) {
        self.count += count;
        if weight > 0. && weight.is_finite() {
            self.sum += weight;
            if random * self.sum < weight {
                self.selected = id;
                self.target = target;
            }
        }
    }
    pub fn normalization(&self) -> f32 {
        if self.count > 0 && self.target > 0. {
            self.sum / (self.count as f32 * self.target)
        } else {
            0.
        }
    }
}
pub fn corner_bound(p: Vec3, corners: &[(Vec3, f32)]) -> f32 {
    corners
        .iter()
        .map(|(c, d)| d - p.distance(*c))
        .fold(f32::NEG_INFINITY, f32::max)
}
pub fn quilez_step(shadow: f32, h: f32, previous: f32, t: f32, k: f32) -> f32 {
    let y = if previous.is_finite() {
        h * h / (2. * previous.max(1e-20))
    } else {
        0.
    };
    let d = (h * h - y * y).max(0.).sqrt();
    if t > y {
        shadow.min(k * d / (t - y))
    } else {
        shadow
    }
}
pub fn trilinear_bound(p: Vec3, lo: Vec3, spacing: Vec3, samples: [f32; 8]) -> f32 {
    let f = (p - lo) / spacing;
    let mut value = 0.;
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let w = Vec3::new(
                    if x == 1 { f.x } else { 1. - f.x },
                    if y == 1 { f.y } else { 1. - f.y },
                    if z == 1 { f.z } else { 1. - f.z },
                );
                value += samples[x + 2 * (y + 2 * z)] * w.x * w.y * w.z;
            }
        }
    }
    let bound = value - spacing.length() * (0.5 + 1. / 128.);
    if bound > 0. {
        bound / 3_f32.sqrt()
    } else {
        bound
    }
}
pub fn signed_box_distance(p: Vec3, lo: Vec3, hi: Vec3) -> f32 {
    let delta = (lo - p).max(p - hi);
    delta.max(Vec3::ZERO).length() + delta.max_element().min(0.)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enclosing_box_and_sphere_are_signed_lower_bounds() {
        let lo = Vec3::splat(-1.);
        let hi = Vec3::ONE;
        // A unit sphere and a torus (major radius 0.7, minor radius 0.3)
        // are both enclosed by this box, including empty corners and the hole.
        for z in -15..=15 {
            for y in -15..=15 {
                for x in -15..=15 {
                    let p = Vec3::new(x as f32, y as f32, z as f32) / 10.;
                    let bound = signed_box_distance(p, lo, hi).max(p.length() - 1.);
                    let sphere = p.length() - 1.;
                    let torus = glam::Vec2::new(p.truncate().length() - 0.7, p.z).length() - 0.3;
                    assert!(bound <= sphere + 1e-6);
                    assert!(bound <= torus + 1e-6);
                    for offset in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::ONE.normalize()] {
                        let q = p + offset * 0.031;
                        let other = signed_box_distance(q, lo, hi).max(q.length() - 1.);
                        assert!((bound - other).abs() <= p.distance(q) + 1e-6);
                    }
                }
            }
        }
        assert_eq!(signed_box_distance(Vec3::ZERO, lo, hi), -1.);
        assert_eq!(signed_box_distance(Vec3::new(2., 0., 0.), lo, hi), 1.);
        assert!((signed_box_distance(Vec3::new(2., 2., 2.), lo, hi) - 3_f32.sqrt()).abs() < 1e-6);
        assert!(signed_box_distance(Vec3::new(1., 0., 0.), lo, hi).abs() < 1e-6);
    }
    #[test]
    fn constant_target_reservoir_recovers_light_sum() {
        let mut r = Reservoir::default();
        for i in 0..8 {
            r.insert(i, 2., 1. / 4., 0.1);
        }
        assert_eq!(r.count, 8);
        assert!((2. * r.normalization() - 8.).abs() < 1e-6);
    }
    #[test]
    fn reuse_reweights_target() {
        let mut source = Reservoir::default();
        source.insert(0, 2., 0.25, 0.);
        let mut r = Reservoir::default();
        r.merge(
            source.selected,
            4.,
            4. * source.normalization() * source.count as f32,
            source.count,
            0.,
        );
        assert!((r.normalization() - 4.).abs() < 1e-6);
    }
    #[test]
    fn corner_sdf_never_oversteps_sphere() {
        let mut corners = vec![];
        for z in [0., 1.] {
            for y in [0., 1.] {
                for x in [0., 1.] {
                    let p = Vec3::new(x, y, z);
                    corners.push((p, p.length() - 0.4));
                }
            }
        }
        for z in 0..11 {
            for y in 0..11 {
                for x in 0..11 {
                    let p = Vec3::new(x as f32, y as f32, z as f32) / 10.;
                    assert!(corner_bound(p, &corners) <= p.length() - 0.4 + 1e-6);
                }
            }
        }
    }
    #[test]
    fn uniform_scale_distance_is_world_distance() {
        let p = Vec3::new(3., 0., 0.);
        let local = p / 2.;
        assert_eq!((local.length() - 1.) * 2., 1.);
    }
    #[test]
    fn second_shadow_method_is_finite_and_monotonic() {
        let a = quilez_step(1., 0.1, f32::INFINITY, 2., 16.);
        assert!((a - 0.8).abs() < 1e-5);
        let b = quilez_step(a, 0.01, 0.1, 2.1, 16.);
        assert!(b >= 0. && b <= a);
    }
    #[test]
    fn continuous_trilinear_bound_never_oversteps() {
        let samples = std::array::from_fn(|i| {
            let p = Vec3::new((i % 2) as f32, ((i / 2) % 2) as f32, ((i / 4) % 2) as f32);
            p.length() - 0.4
        });
        for z in 0..11 {
            for y in 0..11 {
                for x in 0..11 {
                    let p = Vec3::new(x as f32, y as f32, z as f32) / 10.;
                    let d = trilinear_bound(p, Vec3::ZERO, Vec3::ONE, samples);
                    let exact = p.length() - 0.4;
                    assert!(d <= exact + 1e-6);
                }
            }
        }
        let left = std::array::from_fn(|i| {
            Vec3::new((i % 2) as f32, ((i / 2) % 2) as f32, ((i / 4) % 2) as f32).length() - 0.4
        });
        let right = std::array::from_fn(|i| {
            Vec3::new(
                1. + (i % 2) as f32,
                ((i / 2) % 2) as f32,
                ((i / 4) % 2) as f32,
            )
            .length()
                - 0.4
        });
        let p = Vec3::new(1., 0.4, 0.3);
        assert!(
            (trilinear_bound(p, Vec3::ZERO, Vec3::ONE, left)
                - trilinear_bound(p, Vec3::X, Vec3::ONE, right))
            .abs()
                < 1e-6
        );
    }
}
