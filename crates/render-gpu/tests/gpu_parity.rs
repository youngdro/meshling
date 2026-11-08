//! The GPU backend must match the CPU reference renderer within 2/255 inside meshes.
//!
//! Every test name contains `gpu` so CI's lavapipe job can select them. Without an adapter the
//! tests pass with a note, unless `MESHLING_REQUIRE_GPU` is set (as it is in CI).

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::sync::Arc;

use meshling_geom::Vec2;
use meshling_model::{Blend, Project};
use meshling_plan::fixtures::*;
use meshling_plan::plan;
use meshling_render_gpu::{GpuContext, Renderer};
use meshling_rig::{ParamValues, evaluate};

fn renderer() -> Option<Renderer> {
    match GpuContext::headless() {
        Ok(ctx) => {
            eprintln!("GPU adapter: {}", ctx.adapter);
            Some(Renderer::new(ctx))
        }
        Err(e) if std::env::var_os("MESHLING_REQUIRE_GPU").is_none() => {
            eprintln!("skipping GPU test: {e}");
            None
        }
        Err(e) => panic!("MESHLING_REQUIRE_GPU is set but no GPU is usable: {e}"),
    }
}

/// Render on both backends and return the largest per-channel difference, plus the share of
/// pixels that differ by more than 2/255.
fn compare(r: &mut Renderer, p: &Project, values: &ParamValues, samples: u32) -> (u8, f64) {
    let pose = evaluate(p, values).unwrap();
    let frame = plan(p, &pose);
    let cpu = meshling_render_cpu::render(p, &pose, &frame, &textures()).unwrap().to_rgba8();
    let gpu = r.render_image(p, &pose, &frame, &textures(), samples).unwrap();
    assert_eq!(cpu.len(), gpu.len());
    let mut worst = 0u8;
    let mut over = 0usize;
    for (c, g) in cpu.chunks_exact(4).zip(gpu.chunks_exact(4)) {
        let d = c.iter().zip(g).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        worst = worst.max(d);
        if d > 2 {
            over += 1;
        }
    }
    (worst, over as f64 / (cpu.len() / 4) as f64)
}

fn assert_exact_parity(p: &Project, values: &ParamValues) {
    let Some(mut r) = renderer() else { return };
    let (worst, over) = compare(&mut r, p, values, 1);
    eprintln!("parity: worst {worst}/255, over {:.3}%", over * 100.0);
    assert!(worst <= 2, "max channel difference {worst}/255, {:.2}% of pixels over 2/255", over * 100.0);
}

#[test]
fn gpu_matches_cpu_on_an_opaque_quad() {
    let mut p = base(16);
    p.meshes.push(quad(10, RED, rect(0.0, 0.0, 16.0, 16.0), 0, Blend::Normal));
    assert_exact_parity(&p, &ParamValues::new());
}

#[test]
fn gpu_covers_shared_edges_exactly_once() {
    let mut p = base(16);
    let skew: Arc<[Vec2]> =
        vec![Vec2::new(-0.3, 0.2), Vec2::new(16.4, -0.1), Vec2::new(15.7, 16.3), Vec2::new(0.1, 15.9)].into();
    p.meshes.push(quad(10, QUARTER, rect(-1.0, -1.0, 17.0, 17.0), 0, Blend::Add));
    p.meshes.push(quad(11, QUARTER, skew, 1, Blend::Add));
    assert_exact_parity(&p, &ParamValues::new());
}

#[test]
fn gpu_matches_cpu_on_every_blend_mode() {
    for blend in [Blend::Normal, Blend::Add, Blend::Multiply] {
        let mut p = base(16);
        p.meshes.push(quad(10, GRADIENT, rect(0.0, 0.0, 16.0, 16.0), 0, Blend::Normal));
        let mut top = quad(11, CHECKER, rect(2.0, 3.0, 14.0, 13.0), 1, blend);
        top.keyforms.forms[0].opacity = 0.6;
        p.meshes.push(top);
        assert_exact_parity(&p, &ParamValues::new());
    }
}

#[test]
fn gpu_matches_cpu_on_the_scene_at_rest_and_bent() {
    assert_exact_parity(&scene(), &ParamValues::new());
    assert_exact_parity(&scene(), &ParamValues::new().with(PARAM, 1.0));
}

#[test]
fn gpu_msaa_only_changes_edges() {
    let Some(mut r) = renderer() else { return };
    let (_, over) = compare(&mut r, &scene(), &ParamValues::new().with(PARAM, 0.5), 4);
    assert!(over < 0.15, "{:.2}% of pixels differ by more than 2/255 with 4× MSAA", over * 100.0);
}

#[test]
fn gpu_reports_a_healthy_device_after_rendering() {
    let Some(mut r) = renderer() else { return };
    compare(&mut r, &scene(), &ParamValues::new(), 1);
    assert!(r.context().health.is_healthy(), "{:?}", r.context().health.fault());
}
