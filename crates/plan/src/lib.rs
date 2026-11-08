//! Frame planner (architecture: "渲染管线" step 2).
//!
//! [`plan`] turns a project and a [`Pose`] into a [`FramePlan`]: a linear list of passes that
//! both backends (`render-cpu`, `render-gpu`) execute without knowing anything about parts,
//! deformers or parameters. Draw order, visibility, clipping and blending rules live here and
//! nowhere else.
//!
//! Compositing rules, shared by both backends (all colours premultiplied, values in gamma space,
//! as Live2D-style rigs expect):
//!
//! | blend | colour | alpha |
//! | --- | --- | --- |
//! | Normal | `src + dst·(1 − src.a)` | `src.a + dst.a·(1 − src.a)` |
//! | Add | `src + dst` | `dst.a` |
//! | Multiply | `src·dst + dst·(1 − src.a)` | `dst.a` |
//!
//! Each maps onto one fixed-function GPU blend state. `src` is the texture sample times the
//! mesh's opacity times its mask coverage. A mask is the alpha of its meshes composited Normal
//! (their own opacity ignored), so a clipped mesh shows only where its masks have ink.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use meshling_geom::Size;
use meshling_model::{Blend, ObjId, Project};
use meshling_rig::Pose;

/// Index of a mask in the frame's mask set.
pub type MaskSlot = u32;

#[derive(Clone, Debug, PartialEq)]
pub enum Pass {
    /// Render the union of `meshes` (their alpha) into mask `slot`. All mask passes come first.
    Mask { slot: MaskSlot, meshes: Vec<ObjId> },
    /// Draw one mesh with its texture over the frame.
    Draw { mesh: ObjId, blend: Blend, opacity: f32, mask: Option<MaskSlot> },
}

/// Everything a backend needs to draw one frame, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePlan {
    pub canvas: Size,
    pub passes: Vec<Pass>,
    pub mask_slots: u32,
}

impl FramePlan {
    /// The meshes drawn, back to front.
    pub fn draws(&self) -> impl Iterator<Item = ObjId> + '_ {
        self.passes.iter().filter_map(|p| match p {
            Pass::Draw { mesh, .. } => Some(*mesh),
            Pass::Mask { .. } => None,
        })
    }
}

/// Plan a frame. Meshes are drawn back to front by posed draw order, ties broken by their order
/// in the project. A mesh is skipped when its opacity is zero or any part above it is hidden.
pub fn plan(project: &Project, pose: &Pose) -> FramePlan {
    let hidden = hidden_parts(project);
    let mut draws: Vec<(i32, usize, ObjId, f32)> = Vec::new();
    for (index, m) in project.meshes.iter().enumerate() {
        let Some(posed) = pose.mesh(m.meta.id) else { continue };
        if posed.opacity <= 0.0 || hidden.contains(&m.part) {
            continue;
        }
        draws.push((posed.draw_order, index, m.meta.id, posed.opacity.min(1.0)));
    }
    draws.sort_by_key(|&(order, index, ..)| (order, index));

    let mut masks: HashMap<Vec<ObjId>, MaskSlot> = HashMap::new();
    let mut mask_passes = Vec::new();
    let mut draw_passes = Vec::new();
    for (_, index, mesh, opacity) in draws {
        let m = &project.meshes[index];
        let mask = if m.clip_by.is_empty() {
            None
        } else {
            let mut key = m.clip_by.clone();
            key.sort();
            key.dedup();
            let next = masks.len() as MaskSlot;
            let slot = *masks.entry(key.clone()).or_insert_with(|| {
                mask_passes.push(Pass::Mask { slot: next, meshes: key });
                next
            });
            Some(slot)
        };
        draw_passes.push(Pass::Draw { mesh, blend: m.blend, opacity, mask });
    }
    let mask_slots = mask_passes.len() as u32;
    mask_passes.extend(draw_passes);
    FramePlan { canvas: project.canvas, passes: mask_passes, mask_slots }
}

/// Parts that are hidden themselves or under a hidden ancestor.
fn hidden_parts(project: &Project) -> HashSet<ObjId> {
    let mut hidden = HashSet::new();
    for p in &project.parts {
        let mut cur = Some(p);
        for _ in 0..=project.parts.len() {
            match cur {
                Some(q) if !q.visible => {
                    hidden.insert(p.meta.id);
                    break;
                }
                Some(q) => cur = q.parent.and_then(|id| project.part(id)),
                None => break,
            }
        }
    }
    hidden
}

/// Premultiplied RGBA8 pixels of one texture, row major, top row first.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub size: Size,
    pub rgba: Arc<[u8]>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pixels").field("size", &self.size).field("bytes", &self.rgba.len()).finish()
    }
}

impl Pixels {
    /// `None` when `rgba` isn't `width × height × 4` bytes.
    pub fn new(size: Size, rgba: impl Into<Arc<[u8]>>) -> Option<Self> {
        let rgba = rgba.into();
        (rgba.len() == size.width as usize * size.height as usize * 4).then_some(Self { size, rgba })
    }

    /// One colour everywhere (premultiplied).
    pub fn solid(size: Size, rgba: [u8; 4]) -> Self {
        let n = size.width as usize * size.height as usize;
        Self { size, rgba: rgba.repeat(n).into() }
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.size.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// Texture pixels by texture id. Backends read textures only through this.
pub type Textures = HashMap<ObjId, Pixels>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use meshling_geom::Vec2;
    use meshling_model::{ArtMesh, KeyformGrid, MeshKeyform, Meta, Part};
    use meshling_rig::{ParamValues, evaluate};

    use super::*;

    fn id(n: u128) -> ObjId {
        ObjId::from_u128(n)
    }

    fn project(meshes: &[(u128, i32, f32, &[u128], u128)]) -> Project {
        let mut p = Project::new(Size::new(10, 10));
        p.parts.push(Part { meta: Meta::new("root").with_id(id(1)), parent: None, visible: true });
        p.parts.push(Part { meta: Meta::new("hidden").with_id(id(2)), parent: None, visible: false });
        p.parts.push(Part { meta: Meta::new("under hidden").with_id(id(3)), parent: Some(id(2)), visible: true });
        let tri: Arc<[Vec2]> = vec![Vec2::ZERO, Vec2::X, Vec2::Y].into();
        for &(n, order, opacity, clip, part) in meshes {
            p.meshes.push(ArtMesh {
                meta: Meta::new("m").with_id(id(n)),
                part: id(part),
                deformer: None,
                texture: id(9),
                uvs: tri.clone(),
                indices: vec![0, 1, 2].into(),
                draw_order: order,
                blend: Blend::Normal,
                clip_by: clip.iter().map(|&c| id(c)).collect(),
                keyforms: KeyformGrid::single(MeshKeyform { positions: tri.clone(), opacity, draw_order: 0 }),
            });
        }
        p
    }

    fn plan_of(p: &Project) -> FramePlan {
        plan(p, &evaluate(p, &ParamValues::new()).unwrap())
    }

    #[test]
    fn draws_back_to_front_with_stable_ties() {
        let p = project(&[(10, 5, 1.0, &[], 1), (11, 1, 1.0, &[], 1), (12, 5, 1.0, &[], 1), (13, 3, 1.0, &[], 1)]);
        assert_eq!(plan_of(&p).draws().collect::<Vec<_>>(), vec![id(11), id(13), id(10), id(12)]);
    }

    #[test]
    fn skips_transparent_and_hidden_meshes() {
        let p = project(&[(10, 0, 0.0, &[], 1), (11, 0, 1.0, &[], 2), (12, 0, 1.0, &[], 3), (13, 0, 0.4, &[], 1)]);
        let plan = plan_of(&p);
        assert_eq!(plan.passes, vec![Pass::Draw { mesh: id(13), blend: Blend::Normal, opacity: 0.4, mask: None }]);
    }

    #[test]
    fn masks_come_first_and_are_shared_by_identical_sets() {
        let p = project(&[
            (10, 0, 1.0, &[], 1),
            (11, 1, 1.0, &[10], 1),
            (12, 2, 1.0, &[10], 1),
            (13, 3, 1.0, &[14, 10], 1),
            (14, 4, 1.0, &[], 1),
        ]);
        let plan = plan_of(&p);
        assert_eq!(plan.mask_slots, 2);
        assert_eq!(plan.passes[0], Pass::Mask { slot: 0, meshes: vec![id(10)] });
        assert_eq!(plan.passes[1], Pass::Mask { slot: 1, meshes: vec![id(10), id(14)] });
        let masks: Vec<_> = plan
            .passes
            .iter()
            .filter_map(|p| match p {
                Pass::Draw { mask, .. } => Some(*mask),
                Pass::Mask { .. } => None,
            })
            .collect();
        assert_eq!(masks, vec![None, Some(0), Some(0), Some(1), None]);
    }

    #[test]
    fn pixels_check_their_length() {
        assert!(Pixels::new(Size::new(2, 2), vec![0u8; 16]).is_some());
        assert!(Pixels::new(Size::new(2, 2), vec![0u8; 15]).is_none());
        assert_eq!(Pixels::solid(Size::new(2, 1), [1, 2, 3, 4]).get(1, 0), [1, 2, 3, 4]);
    }
}
