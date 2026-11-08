//! Binding objects to parameters and shaping their keyforms.
//!
//! A keyform is addressed by `at`: one value per bound parameter (parameter id → key value),
//! which must equal one of that binding's keys.

use std::collections::BTreeMap;

use meshling_geom::Vec2;
use meshling_model::{DeformerKind, KeyformGrid, ObjId, ParamBinding, Project};
use meshling_ops::Object;
use meshling_rig::{to_canvas, to_local};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{changed, object, schema, values_of};
use crate::{CommandError, CommandResult, Registry, Session};

/// The rig interpolates over at most this many parameters per object.
const MAX_BINDINGS: usize = 3;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "keyform.bind",
        "Bind a mesh or deformer to a parameter with keys; every existing form is copied to each key.",
        true,
        bind,
    );
    r.add("keyform.unbind", "Remove a parameter binding, keeping the forms at one of its keys.", true, unbind);
    r.add(
        "keyform.set_mesh",
        "Shape a mesh at one keyform: vertex positions or offsets (own space or canvas pixels), opacity, draw order.",
        true,
        set_mesh,
    );
    r.add(
        "keyform.set_warp",
        "Shape a warp deformer at one keyform: lattice points or offsets (parent space or canvas pixels), opacity.",
        true,
        set_warp,
    );
    r.add(
        "keyform.set_rotation",
        "Set a rotation deformer's pivot, angle (degrees, clockwise), scale or opacity at one keyform.",
        true,
        set_rotation,
    );
}

/// Apply `f` to the keyform grid of a mesh or deformer, whatever its form type.
macro_rules! with_grid {
    ($object:expr, |$g:ident| $body:expr) => {
        match $object {
            Object::Mesh(m) => {
                let $g = &mut m.keyforms;
                $body
            }
            Object::Deformer(d) => match &mut d.kind {
                DeformerKind::Warp { keyforms, .. } => {
                    let $g = keyforms;
                    $body
                }
                DeformerKind::Rotation { keyforms } => {
                    let $g = keyforms;
                    $body
                }
            },
            _ => Err(CommandError::BadArgs("only meshes and deformers have keyforms".into())),
        }
    };
}

fn add_binding<T: Clone>(g: &mut KeyformGrid<T>, param: ObjId, keys: &[f32]) -> Result<(), CommandError> {
    if g.bindings.iter().any(|b| b.param == param) {
        return Err(CommandError::Invalid("already bound to this parameter".into()));
    }
    if g.bindings.len() >= MAX_BINDINGS {
        return Err(CommandError::Invalid(format!("an object can bind at most {MAX_BINDINGS} parameters")));
    }
    // The new binding varies slowest: the old grid repeats once per key.
    let old = std::mem::take(&mut g.forms);
    g.forms = keys.iter().flat_map(|_| old.iter().cloned()).collect();
    g.bindings.push(ParamBinding { param, keys: keys.to_vec() });
    Ok(())
}

fn remove_binding<T>(g: &mut KeyformGrid<T>, param: ObjId, keep: f32) -> Result<(), CommandError> {
    let i = g
        .bindings
        .iter()
        .position(|b| b.param == param)
        .ok_or_else(|| CommandError::Invalid("not bound to this parameter".into()))?;
    let key = key_index(&g.bindings[i], keep)?;
    let stride: usize = g.bindings[..i].iter().map(|b| b.keys.len()).product();
    let n = g.bindings[i].keys.len();
    let forms = std::mem::take(&mut g.forms);
    g.forms = forms.into_iter().enumerate().filter(|(f, _)| f / stride % n == key).map(|(_, t)| t).collect();
    g.bindings.remove(i);
    Ok(())
}

fn key_index(b: &ParamBinding, value: f32) -> Result<usize, CommandError> {
    b.keys.iter().position(|&k| (k - value).abs() <= 1e-4 * (1.0 + k.abs())).ok_or_else(|| {
        CommandError::BadArgs(format!("{value} is not a key of parameter {} (keys: {:?})", b.param, b.keys))
    })
}

/// Flat index of the form addressed by `at`.
fn form_index<T>(g: &KeyformGrid<T>, at: &BTreeMap<ObjId, f32>) -> Result<usize, CommandError> {
    if let Some(extra) = at.keys().find(|p| !g.bindings.iter().any(|b| b.param == **p)) {
        return Err(CommandError::BadArgs(format!("not bound to parameter {extra}")));
    }
    let coords = g
        .bindings
        .iter()
        .map(|b| {
            let v = at
                .get(&b.param)
                .ok_or_else(|| CommandError::BadArgs(format!("`at` needs a value for parameter {}", b.param)))?;
            key_index(b, *v)
        })
        .collect::<Result<Vec<_>, _>>()?;
    g.index(&coords).ok_or_else(|| CommandError::BadArgs("no such keyform".into()))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Bind {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    #[schemars(with = "schema::Id")]
    param: ObjId,
    /// Strictly increasing values within the parameter's range, e.g. `[-30, 0, 30]`.
    keys: Vec<f32>,
}

fn bind(s: &mut Session, a: Bind) -> CommandResult {
    let p = s.project().param(a.param).ok_or(CommandError::NotFound(a.param))?;
    if a.keys.is_empty() || a.keys.windows(2).any(|w| w[0] >= w[1]) {
        return Err(CommandError::BadArgs("keys must be non-empty and strictly increasing".into()));
    }
    if a.keys.iter().any(|k| *k < p.min || *k > p.max) {
        return Err(CommandError::BadArgs(format!("keys must lie within the parameter's range {}..{}", p.min, p.max)));
    }
    let label = format!("Bind to {}", p.meta.name);
    let mut o = object(s.project(), a.id)?;
    with_grid!(&mut o, |g| add_binding(g, a.param, &a.keys))?;
    let ((), c) = s.edit(label, |e| Ok(e.update(a.id, |x| *x = o)?))?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Unbind {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    #[schemars(with = "schema::Id")]
    param: ObjId,
    /// The key whose forms to keep.
    keep: f32,
}

fn unbind(s: &mut Session, a: Unbind) -> CommandResult {
    let mut o = object(s.project(), a.id)?;
    with_grid!(&mut o, |g| remove_binding(g, a.param, a.keep))?;
    let ((), c) = s.edit("Unbind parameter", |e| Ok(e.update(a.id, |x| *x = o)?))?;
    changed(c, json!({}))
}

/// New point positions from exactly one of: absolute positions, offsets in the same space, or
/// offsets in canvas pixels (converted through `space` posed at `at`).
#[derive(Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
struct Shape {
    #[schemars(with = "Option<Vec<schema::Point>>")]
    positions: Option<Vec<Vec2>>,
    #[schemars(with = "Option<Vec<schema::Point>>")]
    offsets: Option<Vec<Vec2>>,
    #[schemars(with = "Option<Vec<schema::Point>>")]
    canvas_offsets: Option<Vec<Vec2>>,
}

impl Shape {
    fn apply(
        &self,
        project: &Project,
        space: Option<ObjId>,
        at: &BTreeMap<ObjId, f32>,
        current: &[Vec2],
    ) -> Result<Option<Vec<Vec2>>, CommandError> {
        let given = [self.positions.is_some(), self.offsets.is_some(), self.canvas_offsets.is_some()];
        if given.iter().filter(|&&g| g).count() > 1 {
            return Err(CommandError::BadArgs("give only one of positions, offsets, canvasOffsets".into()));
        }
        let check = |v: &Vec<Vec2>| {
            if v.len() != current.len() {
                return Err(CommandError::BadArgs(format!("expected {} points, got {}", current.len(), v.len())));
            }
            if v.iter().any(|p| !p.is_finite()) {
                return Err(CommandError::BadArgs("points must be finite".into()));
            }
            Ok(())
        };
        if let Some(v) = &self.positions {
            check(v)?;
            return Ok(Some(v.clone()));
        }
        if let Some(v) = &self.offsets {
            check(v)?;
            return Ok(Some(current.iter().zip(v).map(|(a, b)| *a + *b).collect()));
        }
        if let Some(v) = &self.canvas_offsets {
            check(v)?;
            let values = values_of(at);
            let canvas = to_canvas(project, &values, space, current)?;
            let moved: Vec<Vec2> = canvas.iter().zip(v).map(|(a, b)| *a + *b).collect();
            return Ok(Some(to_local(project, &values, space, &moved)?));
        }
        Ok(None)
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetMesh {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    /// Parameter id → key value of the keyform to change; `{}` for an unbound mesh.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, f32>")]
    at: BTreeMap<ObjId, f32>,
    #[serde(flatten)]
    shape: Shape,
    opacity: Option<f32>,
    draw_order: Option<i32>,
}

fn set_mesh(s: &mut Session, a: SetMesh) -> CommandResult {
    let Object::Mesh(mut m) = object(s.project(), a.id)? else {
        return Err(CommandError::BadArgs("not a mesh".into()));
    };
    let i = form_index(&m.keyforms, &a.at)?;
    let form = &mut m.keyforms.forms[i];
    if let Some(p) = a.shape.apply(s.project(), m.deformer, &a.at, &form.positions)? {
        form.positions = p.into();
    }
    if let Some(o) = a.opacity {
        form.opacity = o.clamp(0.0, 1.0);
    }
    if let Some(d) = a.draw_order {
        form.draw_order = d;
    }
    let label = format!("Shape {}", m.meta.name);
    let ((), c) = s.edit(label, |e| Ok(e.update(a.id, |x| *x = Object::Mesh(m))?))?;
    changed(c, json!({ "form": i }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetWarp {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, f32>")]
    at: BTreeMap<ObjId, f32>,
    #[serde(flatten)]
    shape: Shape,
    opacity: Option<f32>,
}

fn set_warp(s: &mut Session, a: SetWarp) -> CommandResult {
    let Object::Deformer(mut d) = object(s.project(), a.id)? else {
        return Err(CommandError::BadArgs("not a deformer".into()));
    };
    let parent = d.parent;
    let DeformerKind::Warp { keyforms, .. } = &mut d.kind else {
        return Err(CommandError::BadArgs("not a warp deformer".into()));
    };
    let i = form_index(keyforms, &a.at)?;
    let form = &mut keyforms.forms[i];
    if let Some(p) = a.shape.apply(s.project(), parent, &a.at, &form.points)? {
        form.points = p.into();
    }
    if let Some(o) = a.opacity {
        form.opacity = o.clamp(0.0, 1.0);
    }
    let label = format!("Shape {}", d.meta.name);
    let ((), c) = s.edit(label, |e| Ok(e.update(a.id, |x| *x = Object::Deformer(d))?))?;
    changed(c, json!({ "form": i }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetRotation {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, f32>")]
    at: BTreeMap<ObjId, f32>,
    /// Pivot in the parent's space.
    #[schemars(with = "Option<schema::Point>")]
    origin: Option<Vec2>,
    /// Rotation in degrees, clockwise on screen (the parent's own turn is compensated).
    angle: Option<f32>,
    /// Scale on screen: 1 keeps children at their pixel size (the parent's scale is compensated).
    scale: Option<f32>,
    opacity: Option<f32>,
}

fn set_rotation(s: &mut Session, a: SetRotation) -> CommandResult {
    let Object::Deformer(mut d) = object(s.project(), a.id)? else {
        return Err(CommandError::BadArgs("not a deformer".into()));
    };
    let parent = d.parent;
    let DeformerKind::Rotation { keyforms } = &mut d.kind else {
        return Err(CommandError::BadArgs("not a rotation deformer".into()));
    };
    let i = form_index(keyforms, &a.at)?;
    let form = &mut keyforms.forms[i];
    if let Some(o) = a.origin {
        form.origin = o;
    }
    // Screen angle and scale: compensate for how the parent turns and scales space at the pivot.
    let (ppu, turn) = super::frame_at(s.project(), &values_of(&a.at), parent, form.origin)?;
    if let Some(angle) = a.angle {
        form.angle_deg = angle - turn;
    }
    if let Some(scale) = a.scale {
        if !(scale > 0.0 && scale.is_finite()) {
            return Err(CommandError::BadArgs("scale must be positive".into()));
        }
        form.scale = scale / ppu;
    }
    if let Some(o) = a.opacity {
        form.opacity = o.clamp(0.0, 1.0);
    }
    let label = format!("Shape {}", d.meta.name);
    let ((), c) = s.edit(label, |e| Ok(e.update(a.id, |x| *x = Object::Deformer(d))?))?;
    changed(c, json!({ "form": i }))
}
