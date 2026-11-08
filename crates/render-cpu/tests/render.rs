//! Behaviour of the reference rasterizer, plus golden images.
//!
//! Regenerate goldens after an intended change with `MESHLING_BLESS=1 cargo test -p meshling-render-cpu`
//! and review the new PNGs in the diff.

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::PathBuf;
use std::sync::Arc;

use meshling_geom::{Size, Vec2};
use meshling_model::{Blend, Deformer, DeformerKind, KeyformGrid, Meta, ParamBinding, WarpKeyform};
use meshling_plan::fixtures::*;
use meshling_plan::{Pixels, plan};
use meshling_render_cpu::{Image, composite, render, sample};
use meshling_rig::{ParamValues, evaluate};

fn draw(p: &meshling_model::Project, values: &ParamValues) -> Image {
    let pose = evaluate(p, values).unwrap();
    render(p, &pose, &plan(p, &pose), &textures()).unwrap()
}

#[test]
fn opaque_quad_fills_the_canvas() {
    let mut p = base(16);
    p.meshes.push(quad(10, RED, rect(0.0, 0.0, 16.0, 16.0), 0, Blend::Normal));
    let img = draw(&p, &ParamValues::new());
    assert!(img.pixels.iter().all(|px| *px == [1.0, 0.0, 0.0, 1.0]));
}

#[test]
fn shared_edges_cover_each_pixel_exactly_once() {
    // Additive quarter-grey: a pixel hit twice would read 0.5, a missed one 0.
    let mut p = base(16);
    let skew: Arc<[Vec2]> =
        vec![Vec2::new(-0.3, 0.2), Vec2::new(16.4, -0.1), Vec2::new(15.7, 16.3), Vec2::new(0.1, 15.9)].into();
    p.meshes.push(quad(10, QUARTER, rect(-1.0, -1.0, 17.0, 17.0), 0, Blend::Add));
    p.meshes.push(quad(11, QUARTER, skew, 1, Blend::Add));
    let img = draw(&p, &ParamValues::new());
    let q = 64.0 / 255.0;
    for y in 1..15 {
        for x in 1..15 {
            let px = img.get(x, y);
            assert!((px[0] - 2.0 * q).abs() < 1e-5, "pixel ({x}, {y}) = {px:?}");
        }
    }
}

#[test]
fn opacity_scales_premultiplied_colour() {
    let mut p = base(4);
    let mut m = quad(10, RED, rect(0.0, 0.0, 4.0, 4.0), 0, Blend::Normal);
    m.keyforms.forms[0].opacity = 0.5;
    p.meshes.push(m);
    assert_eq!(draw(&p, &ParamValues::new()).get(2, 2), [0.5, 0.0, 0.0, 0.5]);
}

#[test]
fn blend_modes_follow_the_plan_table() {
    let d = [0.5, 0.25, 1.0, 1.0];
    let s = [0.2, 0.4, 0.0, 0.5];
    assert_eq!(composite(Blend::Normal, s, d), [0.45, 0.525, 0.5, 1.0]);
    assert_eq!(composite(Blend::Add, s, d), [0.7, 0.65, 1.0, 1.0]);
    assert_eq!(composite(Blend::Multiply, s, d), [0.35, 0.225, 0.5, 1.0]);
    assert_eq!(composite(Blend::Add, [0.9; 4], [0.9; 4]), [1.0, 1.0, 1.0, 0.9]);
}

#[test]
fn clipped_mesh_shows_only_inside_its_mask() {
    let mut p = base(16);
    let mut mask = quad(10, RED, rect(0.0, 0.0, 8.0, 16.0), 0, Blend::Normal);
    mask.keyforms.forms[0].opacity = 0.0; // masks ignore their own opacity, and this one is not drawn
    p.meshes.push(mask);
    let mut clipped = quad(11, CHECKER, rect(0.0, 0.0, 16.0, 16.0), 1, Blend::Normal);
    clipped.clip_by = vec![id(10)];
    p.meshes.push(clipped);
    let img = draw(&p, &ParamValues::new());
    assert_eq!(img.get(3, 8)[3], 1.0);
    assert_eq!(img.get(12, 8), [0.0; 4]);
}

#[test]
fn parameters_move_meshes_through_a_warp() {
    let mut p = base(32);
    // A 1×1 lattice is row major: (0,0) (1,0) (0,1) (1,1).
    let lattice = |x: f32| WarpKeyform {
        points: vec![Vec2::new(x, 0.0), Vec2::new(x + 8.0, 0.0), Vec2::new(x, 8.0), Vec2::new(x + 8.0, 8.0)].into(),
        opacity: 1.0,
    };
    p.deformers.push(Deformer {
        meta: Meta::new("slide").with_id(id(20)),
        parent: None,
        kind: DeformerKind::Warp {
            rows: 1,
            cols: 1,
            keyforms: KeyformGrid {
                bindings: vec![ParamBinding { param: PARAM, keys: vec![0.0, 1.0] }],
                forms: vec![lattice(0.0), lattice(24.0)],
            },
        },
    });
    let mut m = quad(10, RED, rect(0.0, 0.0, 1.0, 1.0), 0, Blend::Normal);
    m.deformer = Some(id(20));
    p.meshes.push(m);
    let left = draw(&p, &ParamValues::new().with(PARAM, 0.0));
    let right = draw(&p, &ParamValues::new().with(PARAM, 1.0));
    assert_eq!(left.get(4, 4)[3], 1.0);
    assert_eq!(left.get(28, 4)[3], 0.0);
    assert_eq!(right.get(4, 4)[3], 0.0);
    assert_eq!(right.get(28, 4)[3], 1.0);
}

#[test]
fn bilinear_sampling_with_clamped_edges() {
    let t = Pixels::new(Size::new(2, 1), vec![0, 0, 0, 0, 255, 255, 255, 255]).unwrap();
    assert_eq!(sample(&t, Vec2::new(0.25, 0.5)), [0.0; 4]);
    assert_eq!(sample(&t, Vec2::new(0.5, 0.5)), [0.5; 4]);
    assert_eq!(sample(&t, Vec2::new(1.0, 0.5)), [1.0; 4]);
    assert_eq!(sample(&t, Vec2::new(-3.0, 9.0)), [0.0; 4]);
}

fn golden(name: &str, img: &Image) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.png"));
    let rgba = img.to_rgba8();
    if std::env::var_os("MESHLING_BLESS").is_some() {
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.size.width, img.size.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        return;
    }
    let file = std::fs::File::open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with MESHLING_BLESS=1 to create it", path.display()));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().unwrap();
    let mut want = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut want).unwrap();
    assert_eq!((info.width, info.height), (img.size.width, img.size.height), "{name}: size");
    let worst = rgba.iter().zip(&want[..info.buffer_size()]).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(worst <= 1, "{name}: differs from golden by up to {worst}/255");
}

#[test]
fn golden_scene_at_rest() {
    golden("scene_rest", &draw(&scene(), &ParamValues::new()));
}

#[test]
fn golden_scene_bent() {
    golden("scene_bent", &draw(&scene(), &ParamValues::new().with(PARAM, 1.0)));
}
