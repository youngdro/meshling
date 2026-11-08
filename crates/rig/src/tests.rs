use std::sync::Arc;

use meshling_geom::{Size, Vec2};
use meshling_model::{
    ArtMesh, Blend, Deformer, DeformerKind, KeyformGrid, MeshKeyform, Meta, ObjId, Param, ParamBinding, Part, Project,
    RotationKeyform, TextureAsset, WarpKeyform,
};

use super::*;

fn id(n: u128) -> ObjId {
    ObjId::from_u128(n)
}

const PART: ObjId = ObjId::from_u128(1);
const TEX: ObjId = ObjId::from_u128(2);
const P: ObjId = ObjId::from_u128(10);
const Q: ObjId = ObjId::from_u128(11);

fn project() -> Project {
    let mut p = Project::new(Size::new(100, 100));
    p.parts.push(Part { meta: Meta::new("root").with_id(PART), parent: None, visible: true });
    p.textures.push(TextureAsset {
        meta: Meta::new("t").with_id(TEX),
        size: Size::new(1, 1),
        content_hash: String::new(),
    });
    p.params.push(Param { meta: Meta::new("P").with_id(P), min: -1.0, max: 1.0, default: 0.0 });
    p.params.push(Param { meta: Meta::new("Q").with_id(Q), min: 0.0, max: 1.0, default: 0.0 });
    p
}

fn pts(v: &[(f32, f32)]) -> Arc<[Vec2]> {
    v.iter().map(|&(x, y)| Vec2::new(x, y)).collect()
}

fn mesh(n: u128, deformer: Option<ObjId>, keyforms: KeyformGrid<MeshKeyform>) -> ArtMesh {
    let uvs = pts(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]);
    ArtMesh {
        meta: Meta::new("m").with_id(id(n)),
        part: PART,
        deformer,
        texture: TEX,
        uvs,
        indices: vec![0, 1, 2].into(),
        draw_order: 100,
        blend: Blend::Normal,
        clip_by: vec![],
        keyforms,
    }
}

fn form(points: &[(f32, f32)], opacity: f32, draw_order: i32) -> MeshKeyform {
    MeshKeyform { positions: pts(points), opacity, draw_order }
}

fn close(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 1e-4
}

#[test]
fn unbound_mesh_is_its_single_form() {
    let mut p = project();
    p.meshes.push(mesh(20, None, KeyformGrid::single(form(&[(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)], 0.5, 3))));
    let pose = evaluate(&p, &ParamValues::new()).unwrap();
    assert_eq!(pose.meshes[0].positions, vec![Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0), Vec2::new(5.0, 6.0)]);
    assert_eq!(pose.meshes[0].opacity, 0.5);
    assert_eq!(pose.meshes[0].draw_order, 103);
}

#[test]
fn one_parameter_interpolates_and_clamps() {
    let mut p = project();
    let grid = KeyformGrid {
        bindings: vec![ParamBinding { param: P, keys: vec![-1.0, 0.0, 1.0] }],
        forms: vec![form(&[(0.0, 0.0); 3], 0.0, 0), form(&[(10.0, 0.0); 3], 1.0, 0), form(&[(30.0, 0.0); 3], 1.0, 10)],
    };
    p.meshes.push(mesh(20, None, grid));
    let at = |v: f32| evaluate(&p, &ParamValues::new().with(P, v)).unwrap().meshes[0].clone();
    assert!(close(at(-0.5).positions[0], Vec2::new(5.0, 0.0)));
    assert_eq!(at(-0.5).opacity, 0.5);
    assert!(close(at(0.5).positions[0], Vec2::new(20.0, 0.0)));
    assert_eq!(at(0.5).draw_order, 105);
    // Outside the parameter's range the value is clamped; at a key the form is exact.
    assert!(close(at(7.0).positions[0], Vec2::new(30.0, 0.0)));
    assert!(close(at(0.0).positions[0], Vec2::new(10.0, 0.0)));
    // Default (0) when not set.
    assert!(close(evaluate(&p, &ParamValues::new()).unwrap().meshes[0].positions[0], Vec2::new(10.0, 0.0)));
}

#[test]
fn two_parameters_interpolate_bilinearly() {
    let mut p = project();
    let grid = KeyformGrid {
        bindings: vec![
            ParamBinding { param: P, keys: vec![0.0, 1.0] },
            ParamBinding { param: Q, keys: vec![0.0, 1.0] },
        ],
        // First binding fastest: (P0,Q0) (P1,Q0) (P0,Q1) (P1,Q1).
        forms: vec![
            form(&[(0.0, 0.0); 3], 1.0, 0),
            form(&[(10.0, 0.0); 3], 1.0, 0),
            form(&[(0.0, 10.0); 3], 1.0, 0),
            form(&[(10.0, 10.0); 3], 1.0, 0),
        ],
    };
    p.meshes.push(mesh(20, None, grid));
    let pose = evaluate(&p, &ParamValues::new().with(P, 0.25).with(Q, 0.75)).unwrap();
    assert!(close(pose.meshes[0].positions[0], Vec2::new(2.5, 7.5)));
}

fn warp(n: u128, parent: Option<ObjId>, keyforms: KeyformGrid<WarpKeyform>) -> Deformer {
    Deformer { meta: Meta::new("w").with_id(id(n)), parent, kind: DeformerKind::Warp { rows: 1, cols: 1, keyforms } }
}

fn lattice(x0: f32, y0: f32, x1: f32, y1: f32) -> WarpKeyform {
    WarpKeyform { points: pts(&[(x0, y0), (x1, y0), (x0, y1), (x1, y1)]), opacity: 1.0 }
}

#[test]
fn warp_maps_normalized_space_onto_its_lattice() {
    let mut p = project();
    p.deformers.push(warp(30, None, KeyformGrid::single(lattice(10.0, 20.0, 50.0, 100.0))));
    p.meshes.push(mesh(20, Some(id(30)), KeyformGrid::single(form(&[(0.0, 0.0), (1.0, 1.0), (0.5, 0.25)], 1.0, 0))));
    let m = &evaluate(&p, &ParamValues::new()).unwrap().meshes[0];
    assert!(close(m.positions[0], Vec2::new(10.0, 20.0)));
    assert!(close(m.positions[1], Vec2::new(50.0, 100.0)));
    assert!(close(m.positions[2], Vec2::new(30.0, 40.0)));
}

#[test]
fn warp_extrapolates_outside_the_unit_square() {
    let mut p = project();
    p.deformers.push(warp(30, None, KeyformGrid::single(lattice(0.0, 0.0, 10.0, 10.0))));
    p.meshes.push(mesh(20, Some(id(30)), KeyformGrid::single(form(&[(-0.5, 0.0), (1.5, 2.0), (0.0, 0.0)], 1.0, 0))));
    let m = &evaluate(&p, &ParamValues::new()).unwrap().meshes[0];
    assert!(close(m.positions[0], Vec2::new(-5.0, 0.0)));
    assert!(close(m.positions[1], Vec2::new(15.0, 20.0)));
}

#[test]
fn bent_warp_cell_is_bilinear() {
    let mut p = project();
    let bent = WarpKeyform { points: pts(&[(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (20.0, 10.0)]), opacity: 1.0 };
    p.deformers.push(warp(30, None, KeyformGrid::single(bent)));
    p.meshes.push(mesh(20, Some(id(30)), KeyformGrid::single(form(&[(0.5, 0.5), (1.0, 0.5), (0.0, 0.0)], 1.0, 0))));
    let m = &evaluate(&p, &ParamValues::new()).unwrap().meshes[0];
    assert!(close(m.positions[0], Vec2::new(7.5, 5.0)));
    assert!(close(m.positions[1], Vec2::new(15.0, 5.0)));
}

#[test]
fn rotation_turns_scales_and_moves_children() {
    let mut p = project();
    let rot = RotationKeyform { origin: Vec2::new(50.0, 50.0), angle_deg: 90.0, scale: 2.0, opacity: 0.5 };
    p.deformers.push(Deformer {
        meta: Meta::new("r").with_id(id(31)),
        parent: None,
        kind: DeformerKind::Rotation { keyforms: KeyformGrid::single(rot) },
    });
    p.meshes.push(mesh(20, Some(id(31)), KeyformGrid::single(form(&[(1.0, 0.0), (0.0, 0.0), (0.0, 1.0)], 1.0, 0))));
    let m = &evaluate(&p, &ParamValues::new()).unwrap().meshes[0];
    // +x turns to +y (down on screen) at 90°.
    assert!(close(m.positions[0], Vec2::new(50.0, 52.0)));
    assert!(close(m.positions[1], Vec2::new(50.0, 50.0)));
    assert!(close(m.positions[2], Vec2::new(48.0, 50.0)));
    assert_eq!(m.opacity, 0.5);
}

#[test]
fn nested_deformers_compose_child_first() {
    let mut p = project();
    p.deformers.push(warp(30, None, KeyformGrid::single(lattice(0.0, 0.0, 100.0, 100.0))));
    let rot = RotationKeyform { origin: Vec2::new(0.5, 0.5), angle_deg: 0.0, scale: 0.1, opacity: 0.5 };
    p.deformers.push(Deformer {
        meta: Meta::new("r").with_id(id(31)),
        parent: Some(id(30)),
        kind: DeformerKind::Rotation { keyforms: KeyformGrid::single(rot) },
    });
    p.meshes.push(mesh(20, Some(id(31)), KeyformGrid::single(form(&[(1.0, 0.0), (0.0, 0.0), (0.0, 0.0)], 1.0, 0))));
    let m = &evaluate(&p, &ParamValues::new()).unwrap().meshes[0];
    // (1, 0) → rotation: (0.6, 0.5) in warp space → warp: (60, 50) on canvas.
    assert!(close(m.positions[0], Vec2::new(60.0, 50.0)));
    assert_eq!(m.opacity, 0.5);
}

#[test]
fn deformer_keyforms_follow_parameters() {
    let mut p = project();
    let grid = KeyformGrid {
        bindings: vec![ParamBinding { param: Q, keys: vec![0.0, 1.0] }],
        forms: vec![lattice(0.0, 0.0, 10.0, 10.0), lattice(0.0, 10.0, 10.0, 20.0)],
    };
    p.deformers.push(warp(30, None, grid));
    p.meshes.push(mesh(20, Some(id(30)), KeyformGrid::single(form(&[(0.0, 0.0); 3], 1.0, 0))));
    let m = &evaluate(&p, &ParamValues::new().with(Q, 0.5)).unwrap().meshes[0];
    assert!(close(m.positions[0], Vec2::new(0.0, 5.0)));
}

#[test]
fn malformed_projects_are_errors_not_panics() {
    let mut p = project();
    p.meshes.push(mesh(20, Some(id(99)), KeyformGrid::single(form(&[(0.0, 0.0); 3], 1.0, 0))));
    assert_eq!(evaluate(&p, &ParamValues::new()), Err(RigError::MissingDeformer { owner: id(20), target: id(99) }));

    let mut p = project();
    p.deformers.push(warp(30, Some(id(31)), KeyformGrid::single(lattice(0.0, 0.0, 1.0, 1.0))));
    p.deformers.push(warp(31, Some(id(30)), KeyformGrid::single(lattice(0.0, 0.0, 1.0, 1.0))));
    assert!(matches!(evaluate(&p, &ParamValues::new()), Err(RigError::Cycle(_))));

    let mut p = project();
    p.meshes.push(mesh(20, None, KeyformGrid::single(form(&[(0.0, 0.0); 2], 1.0, 0))));
    assert_eq!(evaluate(&p, &ParamValues::new()), Err(RigError::PointCount { owner: id(20) }));
}

mod spaces {
    use meshling_geom::Vec2;
    use meshling_model::{Deformer, DeformerKind, KeyformGrid, Meta, ObjId, Project, RotationKeyform, WarpKeyform};

    use crate::{ParamValues, RigError, to_canvas, to_local};

    fn id(n: u128) -> ObjId {
        ObjId::from_u128(n)
    }

    /// A bent 2×3 warp at the canvas, holding a rotation deformer.
    fn project() -> Project {
        let mut p = Project::new(meshling_geom::Size::new(100, 100));
        let points: Vec<Vec2> = (0..3)
            .flat_map(|y| {
                (0..4).map(move |x| {
                    Vec2::new(10.0 + 25.0 * x as f32 + 4.0 * y as f32, 10.0 + 30.0 * y as f32 + (x * x) as f32)
                })
            })
            .collect();
        p.deformers.push(Deformer {
            meta: Meta::new("warp").with_id(id(1)),
            parent: None,
            kind: DeformerKind::Warp {
                rows: 2,
                cols: 3,
                keyforms: KeyformGrid::single(WarpKeyform { points: points.into(), opacity: 1.0 }),
            },
        });
        p.deformers.push(Deformer {
            meta: Meta::new("rot").with_id(id(2)),
            parent: Some(id(1)),
            kind: DeformerKind::Rotation {
                keyforms: KeyformGrid::single(RotationKeyform {
                    origin: Vec2::new(0.5, 0.4),
                    angle_deg: 30.0,
                    scale: 0.01,
                    opacity: 1.0,
                }),
            },
        });
        p
    }

    #[test]
    fn to_local_inverts_to_canvas_inside_and_past_the_lattice() {
        let p = project();
        let v = ParamValues::new();
        let canvas = [
            Vec2::new(40.0, 40.0),
            Vec2::new(12.0, 11.0),
            Vec2::new(95.0, 75.0),
            Vec2::new(5.0, 90.0),
            Vec2::new(120.0, -3.0),
        ];
        for deformer in [None, Some(id(1)), Some(id(2))] {
            let local = to_local(&p, &v, deformer, &canvas).unwrap();
            let back = to_canvas(&p, &v, deformer, &local).unwrap();
            for (a, b) in canvas.iter().zip(&back) {
                assert!(a.distance(*b) < 1e-2, "{deformer:?}: {a} -> {b}");
            }
        }
        let corner = to_local(&p, &v, Some(id(1)), &[Vec2::new(10.0, 10.0)]).unwrap();
        assert!(corner[0].length() < 1e-4);
    }

    #[test]
    fn a_collapsed_rotation_cannot_be_inverted() {
        let mut p = project();
        if let DeformerKind::Rotation { keyforms } = &mut p.deformers[1].kind {
            keyforms.forms[0].scale = 0.0;
        }
        assert_eq!(to_local(&p, &ParamValues::new(), Some(id(2)), &[Vec2::ONE]), Err(RigError::NotInvertible(id(2))));
    }
}
