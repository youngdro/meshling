#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::sync::Arc;

use meshling_geom::{Size, Vec2};
use meshling_model::{
    ArtMesh, Blend, KeyformGrid, MeshKeyform, Meta, ObjId, Param, ParamBinding, Part, Project, TextureAsset,
};
use meshling_plan::{Pixels, Textures};
use meshling_validate::{Options, Severity, validate};

const PART: ObjId = ObjId::from_u128(1);
const PARAM: ObjId = ObjId::from_u128(2);
const TEX: ObjId = ObjId::from_u128(3);
const MESH: ObjId = ObjId::from_u128(4);

fn quad(x0: f32, y0: f32, x1: f32, y1: f32) -> Arc<[Vec2]> {
    vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)].into()
}

fn form(positions: Arc<[Vec2]>) -> MeshKeyform {
    MeshKeyform { positions, opacity: 1.0, draw_order: 0 }
}

/// One textured quad whose shape at `Slide = 1` is `bent`.
fn project(bent: Arc<[Vec2]>) -> Project {
    let mut p = Project::new(Size::new(32, 32));
    p.parts.push(Part { meta: Meta::new("root").with_id(PART), parent: None, visible: true });
    p.params.push(Param { meta: Meta::new("Slide").with_id(PARAM), min: -1.0, max: 1.0, default: 0.0 });
    p.textures.push(TextureAsset {
        meta: Meta::new("t").with_id(TEX),
        size: Size::new(2, 2),
        content_hash: String::new(),
    });
    p.meshes.push(ArtMesh {
        meta: Meta::new("cheek").with_id(MESH),
        part: PART,
        deformer: None,
        texture: TEX,
        uvs: quad(0.0, 0.0, 1.0, 1.0),
        indices: vec![0, 1, 2, 0, 2, 3].into(),
        draw_order: 0,
        blend: Blend::Normal,
        clip_by: Vec::new(),
        keyforms: KeyformGrid {
            bindings: vec![ParamBinding { param: PARAM, keys: vec![0.0, 1.0] }],
            forms: vec![form(quad(4.0, 4.0, 20.0, 20.0)), form(bent)],
        },
    });
    p
}

fn textures() -> Textures {
    Textures::from([(TEX, Pixels::solid(Size::new(2, 2), [9, 9, 9, 255]))])
}

fn codes(p: &Project, t: Option<&Textures>) -> Vec<&'static str> {
    validate(p, t, &Options::default()).issues.iter().map(|i| i.code).collect()
}

#[test]
fn a_healthy_rig_is_clean() {
    let p = project(quad(6.0, 4.0, 22.0, 20.0));
    let report = validate(&p, Some(&textures()), &Options::default());
    assert!(report.is_clean(), "{report:#?}");
}

#[test]
fn folds_are_errors_reported_with_the_pose() {
    // At Slide = 1 the quad is mirrored left-right: both triangles fold over.
    let p = project(quad(20.0, 4.0, 4.0, 20.0));
    let report = validate(&p, Some(&textures()), &Options::default());
    assert_eq!(report.errors(), 1, "{report:#?}");
    let issue = &report.issues[0];
    assert_eq!((issue.severity, issue.code, issue.object), (Severity::Error, "flipped-triangles", Some(MESH)));
    assert_eq!(issue.pose.len(), 1);
    assert_eq!((issue.pose[0].name.as_str(), issue.pose[0].value), ("Slide", 1.0));
    assert!(issue.message.contains("2 triangles fold over Slide = 1"), "{}", issue.message);
}

#[test]
fn collapsing_triangles_are_warnings() {
    let p = project(quad(4.0, 12.0, 20.0, 12.0));
    assert_eq!(codes(&p, None), ["collapsed-triangles"]);
}

#[test]
fn parameter_and_mesh_problems() {
    let mut p = project(quad(6.0, 4.0, 20.0, 20.0));
    p.params.push(Param { meta: Meta::new("Unused").with_id(ObjId::from_u128(9)), min: 0.0, max: 1.0, default: 0.0 });
    p.meshes[0].keyforms.bindings[0].keys = vec![0.0, 2.0];
    p.meshes[0].uvs = vec![Vec2::ZERO, Vec2::X, Vec2::new(1.0, 1.5), Vec2::Y].into();
    assert_eq!(codes(&p, None), ["key-out-of-range", "unused-param", "uv-out-of-range"]);
}

#[test]
fn texture_pixels_are_checked_when_given() {
    let p = project(quad(6.0, 4.0, 20.0, 20.0));
    assert_eq!(codes(&p, Some(&Textures::new())), ["missing-pixels"]);
    let wrong = Textures::from([(TEX, Pixels::solid(Size::new(3, 2), [0; 4]))]);
    assert_eq!(codes(&p, Some(&wrong)), ["texture-size"]);
}

#[test]
fn structural_errors_skip_the_pose_sweep() {
    let mut p = project(quad(20.0, 4.0, 4.0, 20.0)); // would also fold
    p.meshes[0].part = ObjId::from_u128(77);
    assert_eq!(codes(&p, None), ["structure"]);
}

#[test]
fn reports_serialize_for_agents() {
    let p = project(quad(20.0, 4.0, 4.0, 20.0));
    let json = serde_json::to_value(validate(&p, None, &Options::default())).unwrap();
    let issue = &json["issues"][0];
    assert_eq!(issue["severity"], "error");
    assert_eq!(issue["code"], "flipped-triangles");
    assert_eq!(issue["pose"][0]["name"], "Slide");
}
