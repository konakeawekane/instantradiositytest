use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub render_width: u32,
    pub render_height: u32,
    pub vpl_count: u32,
    pub vpl_max_bounces: u32,
    pub vpl_rr_min_survival: f32,
    pub vpl_rr_max_survival: f32,
    pub restir_direct_candidates: u32,
    pub restir_vpl_candidates: u32,
    pub restir_spatial_neighbors: u32,
    pub restir_spatial_radius: u32,
    pub depth_threshold: f32,
    pub normal_threshold: f32,
    pub temporal_history_weight: f32,
    pub temporal_max_history: u32,
    pub sdf_max_steps: u32,
    pub sdf_hit_epsilon: f32,
    pub sdf_soft_shadow_k: f32,
    pub target_frame_ms: f32,
}

impl Settings {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let s: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        s.validate()?;
        Ok(s)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.render_width > 0
                && self.render_height > 0
                && self.render_width <= 8192
                && self.render_height <= 8192,
            "invalid render size"
        );
        ensure!(
            self.vpl_count > 0
                && self.vpl_count <= 65536
                && self.vpl_max_bounces >= 1
                && self.vpl_max_bounces <= 32,
            "invalid VPL budget"
        );
        ensure!(
            self.vpl_rr_min_survival > 0.
                && self.vpl_rr_max_survival < 1.
                && self.vpl_rr_min_survival <= self.vpl_rr_max_survival,
            "invalid roulette probabilities"
        );
        ensure!(
            self.restir_direct_candidates > 0
                && self.restir_vpl_candidates > 0
                && self.restir_direct_candidates <= 256
                && self.restir_vpl_candidates <= 256
                && self.restir_spatial_neighbors <= 32
                && self.restir_spatial_radius <= 256,
            "invalid ReSTIR settings"
        );
        ensure!(
            self.temporal_history_weight >= 0.
                && self.temporal_history_weight < 1.
                && self.temporal_max_history > 0
                && self.temporal_max_history <= 256,
            "invalid temporal settings"
        );
        ensure!(
            self.sdf_max_steps > 0
                && self.sdf_max_steps <= 1024
                && self.sdf_hit_epsilon > 0.
                && self.sdf_hit_epsilon.is_finite()
                && self.sdf_soft_shadow_k > 0.
                && self.sdf_soft_shadow_k.is_finite(),
            "invalid SDF settings"
        );
        ensure!(
            self.depth_threshold > 0.
                && self.depth_threshold.is_finite()
                && self.normal_threshold >= 0.
                && self.normal_threshold <= 1.
                && self.target_frame_ms > 0.
                && self.target_frame_ms.is_finite(),
            "invalid rejection/frame budget"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_budgets_and_nonfinite_settings_are_rejected() {
        let original: Settings = serde_json::from_str(include_str!("../settings.json")).unwrap();
        original.validate().unwrap();
        for edit in [
            |s: &mut Settings| s.render_width = 0,
            |s: &mut Settings| s.vpl_count = 0,
            |s: &mut Settings| s.temporal_max_history = 0,
            |s: &mut Settings| s.sdf_hit_epsilon = f32::INFINITY,
            |s: &mut Settings| s.vpl_rr_max_survival = 1.,
            |s: &mut Settings| s.depth_threshold = f32::NAN,
        ] {
            let mut s = original.clone();
            edit(&mut s);
            assert!(s.validate().is_err());
        }
    }
}
