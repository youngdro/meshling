//! Rig evaluation (architecture: "渲染管线" step 1).
//!
//! [`evaluate`] turns a project and a set of parameter values into a [`Pose`]: every mesh's
//! vertices in canvas space, its opacity and its draw order. It is pure, CPU-only and
//! deterministic, so validation and agent comparisons reproduce exactly, and it builds for wasm.
//!
//! Coordinate spaces:
//! - A mesh or deformer without a parent is expressed in canvas pixels.
//! - Inside a **warp** deformer, children use the lattice's normalized space: `(0, 0)` is the
//!   lattice's first control point, `(1, 1)` its last. A point maps through the bilinear patch of
//!   the cell it falls in; points outside `[0, 1]²` extrapolate from the border cells.
//! - Inside a **rotation** deformer, children are offsets from its origin, which is rotated by
//!   `angle_deg` (clockwise on screen, y down), scaled, then moved to the origin in the parent
//!   space.
//!
//! Opacity multiplies down the deformer hierarchy.

mod interp;

use std::collections::HashMap;

use meshling_geom::Vec2;
use meshling_model::{ArtMesh, Deformer, DeformerKind, ObjId, Project};

pub use interp::{Lerp, Weights, weights};

/// Parameter values to evaluate at. Parameters not set take their default; values are clamped
/// to each parameter's range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamValues(HashMap<ObjId, f32>);

impl ParamValues {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, param: ObjId, value: f32) -> &mut Self {
        self.0.insert(param, value);
        self
    }

    pub fn with(mut self, param: ObjId, value: f32) -> Self {
        self.set(param, value);
        self
    }

    /// The value used for `param`: the one set (clamped), else the parameter's default.
    pub fn get(&self, project: &Project, param: ObjId) -> f32 {
        match project.param(param) {
            Some(p) => self.0.get(&param).map_or(p.default, |v| p.clamp(*v)),
            None => self.0.get(&param).copied().unwrap_or(0.0),
        }
    }
}

/// One mesh, posed.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshPose {
    pub mesh: ObjId,
    /// Vertex positions in canvas pixels, in the mesh's vertex order.
    pub positions: Vec<Vec2>,
    /// Final opacity, including every ancestor deformer's.
    pub opacity: f32,
    /// The mesh's base draw order plus its keyform offset.
    pub draw_order: i32,
}

/// Every mesh of a project, posed, in project order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pose {
    pub meshes: Vec<MeshPose>,
}

impl Pose {
    pub fn mesh(&self, id: ObjId) -> Option<&MeshPose> {
        self.meshes.iter().find(|m| m.mesh == id)
    }
}

/// The project is malformed in a way evaluation can't get past. `Project::check` reports these
/// (and more) up front; commands keep projects free of them.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum RigError {
    #[error("{owner} refers to missing deformer {target}")]
    MissingDeformer { owner: ObjId, target: ObjId },
    #[error("deformer {0} is its own ancestor")]
    Cycle(ObjId),
    #[error("{owner}: keyform grid does not match its bindings")]
    Grid { owner: ObjId },
    #[error("{owner}: form has the wrong number of points")]
    PointCount { owner: ObjId },
    #[error("a point cannot be expressed in deformer {0}'s space (it folds or collapses there)")]
    NotInvertible(ObjId),
}

/// Pose every mesh of `project` at `values`.
pub fn evaluate(project: &Project, values: &ParamValues) -> Result<Pose, RigError> {
    let mut frames: HashMap<ObjId, Frame> = HashMap::new();
    for d in &project.deformers {
        resolve(project, values, d.meta.id, &mut frames, 0)?;
    }
    let meshes = project.meshes.iter().map(|m| pose_mesh(project, values, m, &frames)).collect::<Result<_, _>>()?;
    Ok(Pose { meshes })
}

/// Map `points` given in the child space of `deformer` (canvas space for `None`) to canvas
/// space, at `values`.
pub fn to_canvas(
    project: &Project,
    values: &ParamValues,
    deformer: Option<ObjId>,
    points: &[Vec2],
) -> Result<Vec<Vec2>, RigError> {
    let chain = chain(project, values, deformer)?;
    Ok(points.iter().map(|&p| chain.iter().fold(p, |p, (_, f)| f.map.apply(p))).collect())
}

/// The inverse of [`to_canvas`]: express canvas `points` in the child space of `deformer`, so
/// that a mesh or deformer moved under it keeps its place on screen at `values`.
pub fn to_local(
    project: &Project,
    values: &ParamValues,
    deformer: Option<ObjId>,
    points: &[Vec2],
) -> Result<Vec<Vec2>, RigError> {
    let chain = chain(project, values, deformer)?;
    points
        .iter()
        .map(|&p| chain.iter().rev().try_fold(p, |p, (id, f)| f.map.invert(p).ok_or(RigError::NotInvertible(*id))))
        .collect()
}

/// `deformer` and its ancestors, innermost first, evaluated at `values`.
fn chain(project: &Project, values: &ParamValues, deformer: Option<ObjId>) -> Result<Vec<(ObjId, Frame)>, RigError> {
    let mut frames = HashMap::new();
    let mut out = Vec::new();
    let mut next = deformer;
    while let Some(id) = next {
        if out.len() > project.deformers.len() {
            return Err(RigError::Cycle(id));
        }
        resolve(project, values, id, &mut frames, 0)?;
        let frame = frames.get(&id).cloned().ok_or(RigError::MissingDeformer { owner: id, target: id })?;
        next = frame.parent;
        out.push((id, frame));
    }
    Ok(out)
}

/// A deformer evaluated at the current parameters: how it maps child points into canvas space.
#[derive(Clone, Debug)]
struct Frame {
    map: FrameMap,
    parent: Option<ObjId>,
    opacity: f32,
}

#[derive(Clone, Debug)]
enum FrameMap {
    Warp { rows: usize, cols: usize, points: Vec<Vec2> },
    Rotation { origin: Vec2, angle_deg: f32, scale: f32 },
}

impl FrameMap {
    /// Map a point from this deformer's child space into its parent's space.
    fn apply(&self, p: Vec2) -> Vec2 {
        match self {
            FrameMap::Warp { rows, cols, points } => warp(*rows, *cols, points, p),
            FrameMap::Rotation { origin, angle_deg, scale } => {
                let (s, c) = angle_deg.to_radians().sin_cos();
                *origin + Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y) * *scale
            }
        }
    }
}

impl FrameMap {
    /// The child-space point that [`apply`](Self::apply) maps to `q`, if there is one.
    fn invert(&self, q: Vec2) -> Option<Vec2> {
        match self {
            FrameMap::Warp { rows, cols, points } => unwarp(*rows, *cols, points, q),
            FrameMap::Rotation { origin, angle_deg, scale } => {
                if *scale == 0.0 || !scale.is_finite() {
                    return None;
                }
                let (s, c) = angle_deg.to_radians().sin_cos();
                let d = (q - *origin) / *scale;
                Some(Vec2::new(c * d.x + s * d.y, -s * d.x + c * d.y))
            }
        }
    }
}

/// Invert [`warp`]: find the cell whose bilinear patch (extended past the lattice border for
/// border cells, as `warp` extrapolates) contains `q`, by Newton's method in each cell.
fn unwarp(rows: usize, cols: usize, points: &[Vec2], q: Vec2) -> Option<Vec2> {
    const EPS: f32 = 1e-4;
    let at = |x: usize, y: usize| points[y * (cols + 1) + x];
    let mut fallback = None;
    for cy in 0..rows {
        for cx in 0..cols {
            let (p00, p10, p01, p11) = (at(cx, cy), at(cx + 1, cy), at(cx, cy + 1), at(cx + 1, cy + 1));
            let f = |t: Vec2| p00.lerp(p10, t.x).lerp(p01.lerp(p11, t.x), t.y);
            let mut t = Vec2::splat(0.5);
            for _ in 0..30 {
                let r = f(t) - q;
                let du = (p10 - p00).lerp(p11 - p01, t.y);
                let dv = p00.lerp(p10, t.x) * -1.0 + p01.lerp(p11, t.x);
                let det = du.x * dv.y - du.y * dv.x;
                if det.abs() < 1e-12 {
                    break;
                }
                let step = Vec2::new(r.x * dv.y - r.y * dv.x, du.x * r.y - du.y * r.x) / det;
                t -= step;
                if step.length_squared() < 1e-14 {
                    break;
                }
            }
            if (f(t) - q).length() > 1e-3 * (1.0 + q.length()) || !t.is_finite() {
                continue;
            }
            // Inside the cell, or past the lattice edge on a border cell.
            let lo = |c: usize| if c == 0 { f32::NEG_INFINITY } else { -EPS };
            let hi = |c: usize, n: usize| if c + 1 == n { f32::INFINITY } else { 1.0 + EPS };
            let inside = (-EPS..=1.0 + EPS).contains(&t.x) && (-EPS..=1.0 + EPS).contains(&t.y);
            let allowed = t.x >= lo(cx) && t.x <= hi(cx, cols) && t.y >= lo(cy) && t.y <= hi(cy, rows);
            let p = Vec2::new((cx as f32 + t.x) / cols as f32, (cy as f32 + t.y) / rows as f32);
            if inside {
                return Some(p);
            }
            if allowed && fallback.is_none() {
                fallback = Some(p);
            }
        }
    }
    fallback
}

/// Bilinear patch lookup in a `(rows + 1) × (cols + 1)` lattice, extrapolating past the border.
fn warp(rows: usize, cols: usize, points: &[Vec2], p: Vec2) -> Vec2 {
    let gx = p.x * cols as f32;
    let gy = p.y * rows as f32;
    let cx = (gx.floor() as isize).clamp(0, cols as isize - 1) as usize;
    let cy = (gy.floor() as isize).clamp(0, rows as isize - 1) as usize;
    let (tx, ty) = (gx - cx as f32, gy - cy as f32);
    let at = |x: usize, y: usize| points[y * (cols + 1) + x];
    let top = at(cx, cy).lerp(at(cx + 1, cy), tx);
    let bottom = at(cx, cy + 1).lerp(at(cx + 1, cy + 1), tx);
    top.lerp(bottom, ty)
}

fn resolve(
    project: &Project,
    values: &ParamValues,
    id: ObjId,
    frames: &mut HashMap<ObjId, Frame>,
    depth: usize,
) -> Result<(), RigError> {
    if frames.contains_key(&id) {
        return Ok(());
    }
    if depth > project.deformers.len() {
        return Err(RigError::Cycle(id));
    }
    let Some(d) = project.deformer(id) else {
        return Err(RigError::MissingDeformer { owner: id, target: id });
    };
    if let Some(parent) = d.parent {
        if project.deformer(parent).is_none() {
            return Err(RigError::MissingDeformer { owner: id, target: parent });
        }
        resolve(project, values, parent, frames, depth + 1)?;
    }
    let frame = eval_deformer(project, values, d)?;
    frames.insert(id, frame);
    Ok(())
}

fn eval_deformer(project: &Project, values: &ParamValues, d: &Deformer) -> Result<Frame, RigError> {
    let owner = d.meta.id;
    let (map, opacity) = match &d.kind {
        DeformerKind::Warp { rows, cols, keyforms } => {
            let (rows, cols) = (*rows as usize, *cols as usize);
            let n = (rows + 1) * (cols + 1);
            if rows == 0 || cols == 0 || keyforms.forms.iter().any(|k| k.points.len() != n) {
                return Err(RigError::PointCount { owner });
            }
            let w = weights(project, values, keyforms).ok_or(RigError::Grid { owner })?;
            let points = w.blend_points(keyforms.forms.iter().map(|k| &k.points[..]), n);
            let opacity = w.blend(keyforms.forms.iter().map(|k| k.opacity));
            (FrameMap::Warp { rows, cols, points }, opacity)
        }
        DeformerKind::Rotation { keyforms } => {
            let w = weights(project, values, keyforms).ok_or(RigError::Grid { owner })?;
            let origin = w.blend(keyforms.forms.iter().map(|k| k.origin));
            let angle_deg = w.blend(keyforms.forms.iter().map(|k| k.angle_deg));
            let scale = w.blend(keyforms.forms.iter().map(|k| k.scale));
            let opacity = w.blend(keyforms.forms.iter().map(|k| k.opacity));
            (FrameMap::Rotation { origin, angle_deg, scale }, opacity)
        }
    };
    Ok(Frame { map, parent: d.parent, opacity })
}

fn pose_mesh(
    project: &Project,
    values: &ParamValues,
    m: &ArtMesh,
    frames: &HashMap<ObjId, Frame>,
) -> Result<MeshPose, RigError> {
    let owner = m.meta.id;
    let n = m.vertex_count();
    if m.keyforms.forms.iter().any(|k| k.positions.len() != n) {
        return Err(RigError::PointCount { owner });
    }
    let w = weights(project, values, &m.keyforms).ok_or(RigError::Grid { owner })?;
    let mut positions = w.blend_points(m.keyforms.forms.iter().map(|k| &k.positions[..]), n);
    let mut opacity = w.blend(m.keyforms.forms.iter().map(|k| k.opacity));
    let offset = w.blend(m.keyforms.forms.iter().map(|k| k.draw_order as f32)).round() as i32;

    let mut next = m.deformer;
    while let Some(id) = next {
        let frame = frames.get(&id).ok_or(RigError::MissingDeformer { owner, target: id })?;
        for p in &mut positions {
            *p = frame.map.apply(*p);
        }
        opacity *= frame.opacity;
        next = frame.parent;
    }
    Ok(MeshPose { mesh: owner, positions, opacity, draw_order: m.draw_order + offset })
}

#[cfg(test)]
mod tests;
