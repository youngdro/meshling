//! The built-in commands, grouped by what they touch.

mod edit;
mod keyform;
mod project;

use std::collections::BTreeMap;

use meshling_geom::Vec2;
use meshling_model::{ObjId, Project};
use meshling_ops::{Edit, Object};
use meshling_rig::{ParamValues, to_canvas};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{Change, CommandError, CommandResult, Registry};

pub(crate) fn register(r: &mut Registry) {
    project::register(r);
    edit::register(r);
    keyform::register(r);
}

/// Serialize a command's answer.
fn ok(v: impl Serialize) -> CommandResult {
    serde_json::to_value(v).map_err(|e| CommandError::Invalid(e.to_string()))
}

/// A [`Change`] plus command-specific fields.
fn changed(change: Change, extra: Value) -> CommandResult {
    let mut v = ok(change)?;
    if let (Some(obj), Value::Object(more)) = (v.as_object_mut(), extra) {
        obj.extend(more);
    }
    Ok(v)
}

fn object(project: &Project, id: ObjId) -> Result<Object, CommandError> {
    meshling_ops::find(project, id).ok_or(CommandError::NotFound(id))
}

fn in_edit(edit: &Edit, id: ObjId) -> Result<Object, CommandError> {
    object(edit.project(), id)
}

/// Deserialize a field that may be absent (`None`), `null` (`Some(None)`) or a value.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Parameter values from an id → value map.
fn values_of(at: &BTreeMap<ObjId, f32>) -> ParamValues {
    at.iter().fold(ParamValues::new(), |v, (&p, &x)| v.with(p, x))
}

/// Canvas pixels per unit of `deformer`'s child space near local point `p`, and how far (in
/// degrees, clockwise) that space is turned relative to the canvas.
fn frame_at(
    project: &Project,
    values: &ParamValues,
    deformer: Option<ObjId>,
    p: Vec2,
) -> Result<(f32, f32), CommandError> {
    if deformer.is_none() {
        return Ok((1.0, 0.0));
    }
    let h = 1e-3;
    let q = to_canvas(project, values, deformer, &[p, p + Vec2::new(h, 0.0)])?;
    let v = (q[1] - q[0]) / h;
    let ppu = v.length();
    if !(ppu > 0.0 && ppu.is_finite()) {
        return Err(CommandError::Invalid("the deformer collapses space here".into()));
    }
    Ok((ppu, v.y.atan2(v.x).to_degrees()))
}

/// JSON-schema stand-ins for types defined outside this crate.
mod schema {
    use schemars::JsonSchema;

    /// An object id: a 26-character ULID string.
    #[allow(dead_code)]
    #[derive(JsonSchema)]
    #[schemars(transparent)]
    pub struct Id(String);

    /// A point or vector `[x, y]`.
    #[allow(dead_code)]
    #[derive(JsonSchema)]
    #[schemars(transparent)]
    pub struct Point([f32; 2]);

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub enum Blend {
        Normal,
        Add,
        Multiply,
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub enum Kind {
        Texture,
        Part,
        Param,
        Deformer,
        Mesh,
    }
}
