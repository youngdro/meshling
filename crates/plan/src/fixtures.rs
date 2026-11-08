//! Shared test scenes and textures, so the CPU and GPU backends are tested on the same input.
//! Enabled with the `fixtures` feature (dev-dependencies only).
#![allow(clippy::unwrap_used, missing_docs)]

use std::sync::Arc;

use meshling_geom::{Size, Vec2};
use meshling_model::{
    ArtMesh, Blend, Deformer, DeformerKind, KeyformGrid, MeshKeyform, Meta, ObjId, Param, ParamBinding, Part, Project,
    WarpKeyform,
};

use crate::{Pixels, Textures};

pub const PART: ObjId = ObjId::from_u128(1);
pub const PARAM: ObjId = ObjId::from_u128(2);
pub const RED: ObjId = ObjId::from_u128(100);
pub const CHECKER: ObjId = ObjId::from_u128(101);
pub const QUARTER: ObjId = ObjId::from_u128(102);
pub const GRADIENT: ObjId = ObjId::from_u128(103);

pub fn id(n: u128) -> ObjId {
    ObjId::from_u128(n)
}

pub fn textures() -> Textures {
    let mut t = Textures::new();
    t.insert(RED, Pixels::solid(Size::new(4, 4), [255, 0, 0, 255]));
    t.insert(QUARTER, Pixels::solid(Size::new(1, 1), [64, 64, 64, 64]));
    let checker: Vec<u8> = (0..64u32)
        .flat_map(|i| if (i % 8 / 2 + i / 8 / 2) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 160, 255] })
        .collect();
    t.insert(CHECKER, Pixels::new(Size::new(8, 8), checker).unwrap());
    // Horizontal ramp, half transparent on the right (premultiplied).
    let gradient: Vec<u8> = (0..16u32)
        .flat_map(|x| {
            let a = if x < 8 { 255 } else { 128 };
            let g = x * 17;
            [0, (g * a / 255) as u8, ((255 - g) * a / 255) as u8, a as u8]
        })
        .collect();
    t.insert(GRADIENT, Pixels::new(Size::new(16, 1), gradient).unwrap());
    t
}

pub fn base(size: u32) -> Project {
    let mut p = Project::new(Size::new(size, size));
    p.parts.push(Part { meta: Meta::new("root").with_id(PART), parent: None, visible: true });
    p.params.push(Param { meta: Meta::new("Slide").with_id(PARAM), min: 0.0, max: 1.0, default: 0.0 });
    p
}

pub fn quad_uvs() -> Arc<[Vec2]> {
    vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)].into()
}

pub fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Arc<[Vec2]> {
    vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)].into()
}

pub fn quad(n: u128, texture: ObjId, at: Arc<[Vec2]>, order: i32, blend: Blend) -> ArtMesh {
    ArtMesh {
        meta: Meta::new(format!("mesh{n}")).with_id(id(n)),
        part: PART,
        deformer: None,
        texture,
        uvs: quad_uvs(),
        indices: vec![0, 1, 2, 0, 2, 3].into(),
        draw_order: order,
        blend,
        clip_by: vec![],
        keyforms: KeyformGrid::single(MeshKeyform { positions: at, opacity: 1.0, draw_order: 0 }),
    }
}

/// A small scene using every feature: warp bent by a parameter, all three blends, a mask,
/// bilinear magnification and a hidden-behind ordering.
pub fn scene() -> Project {
    let mut p = base(64);
    let bend = |dx: f32| WarpKeyform {
        points: vec![
            Vec2::new(4.0, 4.0),
            Vec2::new(32.0, 4.0 + dx),
            Vec2::new(60.0, 4.0),
            Vec2::new(4.0, 60.0),
            Vec2::new(32.0, 60.0 - dx),
            Vec2::new(60.0, 60.0),
        ]
        .into(),
        opacity: 1.0,
    };
    p.deformers.push(Deformer {
        meta: Meta::new("bend").with_id(id(20)),
        parent: None,
        kind: DeformerKind::Warp {
            rows: 1,
            cols: 2,
            keyforms: KeyformGrid {
                bindings: vec![ParamBinding { param: PARAM, keys: vec![0.0, 1.0] }],
                forms: vec![bend(0.0), bend(12.0)],
            },
        },
    });
    let mut board = grid(10, CHECKER, 8);
    board.deformer = Some(id(20));
    p.meshes.push(board);
    let mut tri = quad(11, RED, rect(8.0, 36.0, 40.0, 58.0), 2, Blend::Multiply);
    tri.indices = vec![0, 1, 2].into();
    p.meshes.push(tri);
    p.meshes.push(quad(12, GRADIENT, rect(10.0, 10.0, 54.0, 30.0), 1, Blend::Normal));
    let mut glow = quad(13, QUARTER, rect(20.0, 0.0, 64.0, 64.0), 3, Blend::Add);
    glow.clip_by = vec![id(12)];
    p.meshes.push(glow);
    p
}

/// A unit-square mesh of `n × n` cells, for deformers to bend.
pub fn grid(n_id: u128, texture: ObjId, n: u32) -> ArtMesh {
    let uvs: Arc<[Vec2]> =
        (0..=n).flat_map(|y| (0..=n).map(move |x| Vec2::new(x as f32 / n as f32, y as f32 / n as f32))).collect();
    let row = n + 1;
    let indices: Arc<[u32]> = (0..n)
        .flat_map(|y| {
            (0..n).flat_map(move |x| {
                let i = y * row + x;
                [i, i + 1, i + row + 1, i, i + row + 1, i + row]
            })
        })
        .collect();
    let mut m = quad(n_id, texture, uvs.clone(), 0, Blend::Normal);
    m.uvs = uvs.clone();
    m.indices = indices;
    m.keyforms = KeyformGrid::single(MeshKeyform { positions: uvs, opacity: 1.0, draw_order: 0 });
    m
}
