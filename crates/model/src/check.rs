use std::collections::{HashMap, HashSet};

use crate::{DeformerKind, KeyformGrid, ObjId, Project};

/// A structural defect: a dangling reference, a mismatched buffer length, a broken grid.
///
/// These are invariants every command must preserve; `meshling-validate` reports the visual
/// problems (flipped triangles, seams) on top of them.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum StructureError {
    #[error("id {0} is used by more than one object")]
    DuplicateId(ObjId),
    #[error("{owner} refers to missing {what} {target}")]
    Dangling { owner: ObjId, what: &'static str, target: ObjId },
    #[error("deformer {0} is its own ancestor")]
    DeformerCycle(ObjId),
    #[error("part {0} is its own ancestor")]
    PartCycle(ObjId),
    #[error("{owner}: keyform grid has {found} forms, its bindings need {expected}")]
    GridSize { owner: ObjId, expected: usize, found: usize },
    #[error("{owner}: keys of parameter {param} are empty or not strictly increasing")]
    BadKeys { owner: ObjId, param: ObjId },
    #[error("{owner}: binds parameter {param} more than once")]
    RepeatedBinding { owner: ObjId, param: ObjId },
    #[error("{owner}: form {form} has {found} points, expected {expected}")]
    PointCount { owner: ObjId, form: usize, expected: usize, found: usize },
    #[error("{owner}: index buffer is not a triangle list over {vertices} vertices")]
    BadIndices { owner: ObjId, vertices: usize },
    #[error("parameter {0}: needs min <= default <= max")]
    ParamRange(ObjId),
}

impl Project {
    /// Every structural defect, in a stable order. An empty list means the project is well formed.
    pub fn check(&self) -> Vec<StructureError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for m in self.metas() {
            if !seen.insert(m.id) {
                out.push(StructureError::DuplicateId(m.id));
            }
        }

        let textures: HashSet<_> = self.textures.iter().map(|t| t.meta.id).collect();
        let parts: HashMap<_, _> = self.parts.iter().map(|p| (p.meta.id, p.parent)).collect();
        let params: HashSet<_> = self.params.iter().map(|p| p.meta.id).collect();
        let deformers: HashMap<_, _> = self.deformers.iter().map(|d| (d.meta.id, d.parent)).collect();
        let meshes: HashSet<_> = self.meshes.iter().map(|m| m.meta.id).collect();
        let mut dangling = |owner, what, target: ObjId, ok: bool| {
            if !ok {
                out.push(StructureError::Dangling { owner, what, target });
            }
        };

        for p in &self.parts {
            if let Some(parent) = p.parent {
                dangling(p.meta.id, "part", parent, parts.contains_key(&parent));
            }
        }
        for d in &self.deformers {
            if let Some(parent) = d.parent {
                dangling(d.meta.id, "deformer", parent, deformers.contains_key(&parent));
            }
        }
        for m in &self.meshes {
            dangling(m.meta.id, "part", m.part, parts.contains_key(&m.part));
            dangling(m.meta.id, "texture", m.texture, textures.contains(&m.texture));
            if let Some(d) = m.deformer {
                dangling(m.meta.id, "deformer", d, deformers.contains_key(&d));
            }
            for c in &m.clip_by {
                dangling(m.meta.id, "mask mesh", *c, meshes.contains(c));
            }
        }

        out.extend(cycles(&parts).into_iter().map(StructureError::PartCycle));
        out.extend(cycles(&deformers).into_iter().map(StructureError::DeformerCycle));

        for p in &self.params {
            if !(p.min <= p.default && p.default <= p.max) {
                out.push(StructureError::ParamRange(p.meta.id));
            }
        }

        for d in &self.deformers {
            let id = d.meta.id;
            match &d.kind {
                DeformerKind::Warp { rows, cols, keyforms } => {
                    check_grid(id, keyforms, &params, &mut out);
                    let expected = (*rows as usize + 1) * (*cols as usize + 1);
                    for (form, k) in keyforms.forms.iter().enumerate() {
                        if k.points.len() != expected {
                            out.push(StructureError::PointCount { owner: id, form, expected, found: k.points.len() });
                        }
                    }
                }
                DeformerKind::Rotation { keyforms } => check_grid(id, keyforms, &params, &mut out),
            }
        }

        for m in &self.meshes {
            let id = m.meta.id;
            check_grid(id, &m.keyforms, &params, &mut out);
            let n = m.vertex_count();
            for (form, k) in m.keyforms.forms.iter().enumerate() {
                if k.positions.len() != n {
                    out.push(StructureError::PointCount { owner: id, form, expected: n, found: k.positions.len() });
                }
            }
            if m.indices.len() % 3 != 0 || m.indices.iter().any(|&i| i as usize >= n) {
                out.push(StructureError::BadIndices { owner: id, vertices: n });
            }
        }
        out
    }
}

fn check_grid<T>(owner: ObjId, grid: &KeyformGrid<T>, params: &HashSet<ObjId>, out: &mut Vec<StructureError>) {
    let mut bound = HashSet::new();
    for b in &grid.bindings {
        if !params.contains(&b.param) {
            out.push(StructureError::Dangling { owner, what: "parameter", target: b.param });
        }
        if !bound.insert(b.param) {
            out.push(StructureError::RepeatedBinding { owner, param: b.param });
        }
        if b.keys.is_empty() || b.keys.windows(2).any(|w| w[0] >= w[1]) {
            out.push(StructureError::BadKeys { owner, param: b.param });
        }
    }
    let expected = grid.expected_len();
    if grid.forms.len() != expected {
        out.push(StructureError::GridSize { owner, expected, found: grid.forms.len() });
    }
}

/// Nodes that are their own ancestor through `parent` links, sorted.
fn cycles(parent_of: &HashMap<ObjId, Option<ObjId>>) -> Vec<ObjId> {
    let mut out: Vec<ObjId> = parent_of
        .keys()
        .copied()
        .filter(|&start| {
            let mut cur = parent_of.get(&start).copied().flatten();
            for _ in 0..parent_of.len() {
                match cur {
                    Some(id) if id == start => return true,
                    Some(id) => cur = parent_of.get(&id).copied().flatten(),
                    None => return false,
                }
            }
            false
        })
        .collect();
    out.sort();
    out
}
