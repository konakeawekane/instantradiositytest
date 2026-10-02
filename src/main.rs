mod app;
#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
mod math;
mod renderer;
mod scene;
mod settings;
use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Clone, Debug)]
#[command(about = "Experimental Vulkan Instant Radiosity / ReSTIR / SDF renderer")]
pub struct Args {
    #[arg(long, default_value = "assets/test_scene/scene.json")]
    scene: PathBuf,
    #[arg(long, default_value = "settings.json")]
    settings: PathBuf,
    #[arg(long)]
    capture: Option<PathBuf>,
    #[arg(long, default_value_t = 64)]
    frames: u32,
    #[arg(long)]
    width: Option<u32>,
    #[arg(long)]
    height: Option<u32>,
    #[arg(long,default_value_t=0,value_parser=clap::value_parser!(u32).range(0..=5))]
    debug: u32,
    #[arg(long)]
    validate_only: bool,
    #[arg(long)]
    benchmark: Option<PathBuf>,
    #[arg(long)]
    vpl_count: Option<u32>,
    #[arg(long)]
    bounces: Option<u32>,
    /// Run a short native editor session, exercise selection, and capture its UI.
    #[arg(long)]
    editor_smoke: bool,
}
fn main() -> Result<()> {
    let args = Args::parse();
    let scene = scene::Scene::load(&args.scene)?;
    let mut settings = settings::Settings::load(&args.settings)?;
    if let Some(w) = args.width {
        settings.render_width = w;
    }
    if let Some(h) = args.height {
        settings.render_height = h;
    }
    if let Some(v) = args.vpl_count {
        settings.vpl_count = v;
    }
    if let Some(b) = args.bounces {
        settings.vpl_max_bounces = b;
    }
    settings.validate()?;
    anyhow::ensure!(
        !args.editor_smoke || args.frames >= 3,
        "--editor-smoke needs at least three frames"
    );
    if args.validate_only {
        println!(
            "Valid scene: {} meshes, {} objects, {} lights",
            scene.data.len(),
            scene.manifest.objects.len(),
            scene.manifest.lights.len()
        );
        return Ok(());
    }
    if !args.editor_smoke && (args.capture.is_some() || args.benchmark.is_some()) {
        anyhow::ensure!(args.frames > 0, "--frames must be positive");
        let mut renderer = pollster::block_on(renderer::Renderer::new(&scene, settings.clone()))?;
        renderer.debug_mode = args.debug;
        let mut times = vec![];
        let start = std::time::Instant::now();
        for i in 0..args.frames {
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            renderer.encode(&scene, &mut encoder, true)?;
            renderer.queue.submit([encoder.finish()]);
            if let Some(ms) = renderer.read_gpu_time()?
                && i >= args.frames.min(8)
            {
                times.push(ms);
            }
            if i % 32 == 0 {
                eprintln!("Frame {i}/{}", args.frames);
            }
        }
        renderer.wait()?;
        if let Some(path) = &args.capture {
            renderer.capture(path)?;
            println!("Captured {}", path.display());
        }
        times.sort_by(f64::total_cmp);
        let median = times.get(times.len() / 2).copied();
        let p95 = times
            .get((times.len().saturating_sub(1) * 95) / 100)
            .copied();
        let report = serde_json::json!({"gpu":renderer.adapter.get_info().name,"settings":settings,"frames":args.frames,"elapsed_seconds":start.elapsed().as_secs_f64(),"gpu_median_ms":median,"gpu_p95_ms":p95,"last_pass_ms":renderer.last_pass_ms,"target_met":median.map(|m|m<=settings.target_frame_ms as f64),"capture":args.capture,"debug_mode":args.debug});
        println!("{}", serde_json::to_string_pretty(&report)?);
        if let Some(path) = args.benchmark {
            if let Some(p) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
        }
        Ok(())
    } else {
        app::run(scene, settings, args).context("interactive editor")
    }
}
