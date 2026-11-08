//! Commands that create, change or delete objects.

use std::sync::Arc;

use meshling_geom::Vec2;
use meshling_mesh::{MeshOptions, auto_mesh};
use meshling_model::{
    Blend, Deformer, DeformerKind, KeyformGrid, Meta, ObjId, Param, Part, Project, RotationKeyform, WarpKeyform,
};
use meshling_ops::Object;
use meshling_rig::{ParamValues, to_canvas, to_local};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{changed, frame_at, in_edit, nullable, object, schema};
use crate::{CommandError, CommandResult, Registry, Session};

pub(super) fn register(r: &mut Registry) {
    r.add("object.rename", "Rename any object.", true, rename);
    r.add("object.tag", "Add and/or remove semantic tags on any object.", true, tag);
    r.add(
        "object.delete",
        "Delete an object. Deleting a part also deletes its sub-parts and their meshes.",
        true,
        delete,
    );
    r.add("part.create", "Create a part (a node of the part tree).", true, part_create);
    r.add("part.update", "Change a part's parent or visibility.", true, part_update);
    r.add("param.create", "Create a parameter (a slider other objects' keyforms bind to).", true, param_create);
    r.add("param.update", "Change a parameter's range or default.", true, param_update);
    r.add(
        "deformer.create_warp",
        "Create a warp deformer: a lattice over a canvas rectangle, placed under a parent deformer if given.",
        true,
        create_warp,
    );
    r.add(
        "deformer.create_rotation",
        "Create a rotation deformer pivoting at a canvas point. Its children are offsets in canvas pixels.",
        true,
        create_rotation,
    );
    r.add(
        "deformer.set_parent",
        "Move a deformer under another deformer (or to the canvas), keeping its rest shape on screen.",
        true,
        deformer_set_parent,
    );
    r.add("mesh.update", "Change a mesh's part, blend mode, draw order or clipping masks.", true, mesh_update);
    r.add(
        "mesh.set_deformer",
        "Attach a mesh to a deformer (or detach it), keeping its rest shape on screen.",
        true,
        mesh_set_deformer,
    );
    r.add(
        "mesh.remesh",
        "Regenerate a flat (not yet rigged) mesh from its texture's alpha with a different density.",
        true,
        remesh,
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    name: String,
}

fn rename(s: &mut Session, a: Rename) -> CommandResult {
    object(s.project(), a.id)?;
    let ((), c) =
        s.edit(format!("Rename to {}", a.name), |e| Ok(e.update(a.id, |o| o.meta_mut().name = a.name.clone())?))?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Tag {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

fn tag(s: &mut Session, a: Tag) -> CommandResult {
    let ((), c) = s.edit("Edit tags", |e| {
        in_edit(e, a.id)?;
        e.update(a.id, |o| {
            let tags = &mut o.meta_mut().tags;
            tags.retain(|t| !a.remove.contains(t));
            for t in &a.add {
                if !tags.contains(t) {
                    tags.push(t.clone());
                }
            }
        })?;
        Ok(())
    })?;
    let tags = object(s.project(), a.id)?.meta().tags.clone();
    changed(c, json!({ "tags": tags }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArg {
    #[schemars(with = "schema::Id")]
    id: ObjId,
}

fn delete(s: &mut Session, a: IdArg) -> CommandResult {
    let target = object(s.project(), a.id)?;
    let ((), c) = s.edit(format!("Delete {}", target.meta().name), |e| {
        if let Object::Part(_) = target {
            // The subtree, deepest parts last so they are removed first.
            let p = e.project().clone();
            let mut parts = vec![a.id];
            let mut i = 0;
            while i < parts.len() {
                let id = parts[i];
                parts.extend(p.parts.iter().filter(|q| q.parent == Some(id)).map(|q| q.meta.id));
                i += 1;
            }
            for m in p.meshes.iter().filter(|m| parts.contains(&m.part)) {
                e.remove(m.meta.id)?;
            }
            for id in parts.into_iter().rev() {
                e.remove(id)?;
            }
        } else {
            e.remove(a.id)?;
        }
        Ok(())
    })?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartCreate {
    name: String,
    #[schemars(with = "Option<schema::Id>")]
    parent: Option<ObjId>,
    #[serde(default)]
    tags: Vec<String>,
}

fn part_create(s: &mut Session, a: PartCreate) -> CommandResult {
    let mut meta = Meta::new(&a.name);
    meta.tags = a.tags;
    let part = Part { meta, parent: a.parent, visible: true };
    let (id, c) = s.edit(format!("Create part {}", a.name), |e| Ok(e.insert(Object::Part(part))?))?;
    changed(c, json!({ "id": id }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartUpdate {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    /// New parent part; `null` makes it a top-level part.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<schema::Id>")]
    parent: Option<Option<ObjId>>,
    visible: Option<bool>,
}

fn part_update(s: &mut Session, a: PartUpdate) -> CommandResult {
    let ((), c) = s.edit("Update part", |e| {
        let Object::Part(_) = in_edit(e, a.id)? else { return Err(CommandError::BadArgs("not a part".into())) };
        e.update(a.id, |o| {
            if let Object::Part(p) = o {
                if let Some(parent) = a.parent {
                    p.parent = parent;
                }
                if let Some(v) = a.visible {
                    p.visible = v;
                }
            }
        })?;
        Ok(())
    })?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ParamCreate {
    name: String,
    min: f32,
    max: f32,
    /// Defaults to 0 clamped into the range.
    default: Option<f32>,
    #[serde(default)]
    tags: Vec<String>,
}

fn param_create(s: &mut Session, a: ParamCreate) -> CommandResult {
    let mut meta = Meta::new(&a.name);
    meta.tags = a.tags;
    let default = a.default.unwrap_or_else(|| 0f32.clamp(a.min.min(a.max), a.max.max(a.min)));
    let param = Param { meta, min: a.min, max: a.max, default };
    let (id, c) = s.edit(format!("Create parameter {}", a.name), |e| Ok(e.insert(Object::Param(param))?))?;
    changed(c, json!({ "id": id }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ParamUpdate {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    min: Option<f32>,
    max: Option<f32>,
    default: Option<f32>,
}

fn param_update(s: &mut Session, a: ParamUpdate) -> CommandResult {
    let ((), c) = s.edit("Update parameter", |e| {
        let Object::Param(_) = in_edit(e, a.id)? else { return Err(CommandError::BadArgs("not a parameter".into())) };
        e.update(a.id, |o| {
            if let Object::Param(p) = o {
                p.min = a.min.unwrap_or(p.min);
                p.max = a.max.unwrap_or(p.max);
                p.default = a.default.unwrap_or(p.default);
            }
        })?;
        Ok(())
    })?;
    changed(c, json!({}))
}

fn rest() -> ParamValues {
    ParamValues::new()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateWarp {
    name: String,
    /// `[x0, y0, x1, y1]` in canvas pixels.
    rect: [f32; 4],
    rows: u32,
    cols: u32,
    #[schemars(with = "Option<schema::Id>")]
    parent: Option<ObjId>,
    #[serde(default)]
    tags: Vec<String>,
}

fn create_warp(s: &mut Session, a: CreateWarp) -> CommandResult {
    if !(1..=32).contains(&a.rows) || !(1..=32).contains(&a.cols) {
        return Err(CommandError::BadArgs("rows and cols must be 1..=32".into()));
    }
    let [x0, y0, x1, y1] = a.rect;
    if !(x1 > x0 && y1 > y0) {
        return Err(CommandError::BadArgs("rect needs x1 > x0 and y1 > y0".into()));
    }
    let canvas: Vec<Vec2> = (0..=a.rows)
        .flat_map(|r| {
            (0..=a.cols).map(move |c| {
                Vec2::new(x0 + (x1 - x0) * c as f32 / a.cols as f32, y0 + (y1 - y0) * r as f32 / a.rows as f32)
            })
        })
        .collect();
    let points = to_local(s.project(), &rest(), a.parent, &canvas)?;
    let mut meta = Meta::new(&a.name);
    meta.tags = a.tags;
    let d = Deformer {
        meta,
        parent: a.parent,
        kind: DeformerKind::Warp {
            rows: a.rows,
            cols: a.cols,
            keyforms: KeyformGrid::single(WarpKeyform { points: points.into(), opacity: 1.0 }),
        },
    };
    let (id, c) = s.edit(format!("Create warp {}", a.name), |e| Ok(e.insert(Object::Deformer(d))?))?;
    changed(c, json!({ "id": id }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateRotation {
    name: String,
    /// Pivot in canvas pixels.
    #[schemars(with = "schema::Point")]
    origin: Vec2,
    #[schemars(with = "Option<schema::Id>")]
    parent: Option<ObjId>,
    #[serde(default)]
    tags: Vec<String>,
}

fn create_rotation(s: &mut Session, a: CreateRotation) -> CommandResult {
    let p = s.project();
    let origin = to_local(p, &rest(), a.parent, &[a.origin])?[0];
    // Make one child unit one canvas pixel, unturned, whatever the parent does at the pivot.
    let (ppu, turn) = frame_at(p, &rest(), a.parent, origin)?;
    let mut meta = Meta::new(&a.name);
    meta.tags = a.tags;
    let d = Deformer {
        meta,
        parent: a.parent,
        kind: DeformerKind::Rotation {
            keyforms: KeyformGrid::single(RotationKeyform { origin, angle_deg: -turn, scale: 1.0 / ppu, opacity: 1.0 }),
        },
    };
    let (id, c) = s.edit(format!("Create rotation {}", a.name), |e| Ok(e.insert(Object::Deformer(d))?))?;
    changed(c, json!({ "id": id }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParent {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    /// New parent deformer; `null` for the canvas.
    #[schemars(with = "Option<schema::Id>")]
    parent: Option<ObjId>,
}

/// Re-express points from one deformer's child space in another's, through the canvas at rest.
fn rehome(p: &Project, from: Option<ObjId>, to: Option<ObjId>, points: &[Vec2]) -> Result<Vec<Vec2>, CommandError> {
    let canvas = to_canvas(p, &rest(), from, points)?;
    Ok(to_local(p, &rest(), to, &canvas)?)
}

fn deformer_set_parent(s: &mut Session, a: SetParent) -> CommandResult {
    let Object::Deformer(mut d) = object(s.project(), a.id)? else {
        return Err(CommandError::BadArgs("not a deformer".into()));
    };
    if d.parent == a.parent {
        return changed(crate::Change { label: String::new(), changed: Vec::new() }, json!({}));
    }
    let p = s.project();
    let from = d.parent;
    match &mut d.kind {
        DeformerKind::Warp { keyforms, .. } => {
            for f in &mut keyforms.forms {
                f.points = rehome(p, from, a.parent, &f.points)?.into();
            }
        }
        DeformerKind::Rotation { keyforms } => {
            for f in &mut keyforms.forms {
                let (ppu_old, turn_old) = frame_at(p, &rest(), from, f.origin)?;
                let origin = rehome(p, from, a.parent, &[f.origin])?[0];
                let (ppu_new, turn_new) = frame_at(p, &rest(), a.parent, origin)?;
                f.origin = origin;
                f.scale *= ppu_old / ppu_new;
                f.angle_deg += turn_old - turn_new;
            }
        }
    }
    d.parent = a.parent;
    let ((), c) = s.edit(format!("Reparent {}", d.meta.name), |e| Ok(e.update(a.id, |o| *o = Object::Deformer(d))?))?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeshUpdate {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    #[schemars(with = "Option<schema::Id>")]
    part: Option<ObjId>,
    #[schemars(with = "Option<schema::Blend>")]
    blend: Option<Blend>,
    draw_order: Option<i32>,
    /// Meshes whose union masks this one; `[]` removes clipping.
    #[schemars(with = "Option<Vec<schema::Id>>")]
    clip_by: Option<Vec<ObjId>>,
}

fn mesh_update(s: &mut Session, a: MeshUpdate) -> CommandResult {
    let ((), c) = s.edit("Update mesh", |e| {
        let Object::Mesh(_) = in_edit(e, a.id)? else { return Err(CommandError::BadArgs("not a mesh".into())) };
        if a.clip_by.as_ref().is_some_and(|c| c.contains(&a.id)) {
            return Err(CommandError::BadArgs("a mesh cannot clip itself".into()));
        }
        e.update(a.id, |o| {
            if let Object::Mesh(m) = o {
                m.part = a.part.unwrap_or(m.part);
                m.blend = a.blend.unwrap_or(m.blend);
                m.draw_order = a.draw_order.unwrap_or(m.draw_order);
                if let Some(c) = &a.clip_by {
                    m.clip_by = c.clone();
                }
            }
        })?;
        Ok(())
    })?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeshSetDeformer {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    /// The deformer to attach to; `null` to detach (canvas space).
    #[schemars(with = "Option<schema::Id>")]
    deformer: Option<ObjId>,
}

fn mesh_set_deformer(s: &mut Session, a: MeshSetDeformer) -> CommandResult {
    let Object::Mesh(mut m) = object(s.project(), a.id)? else {
        return Err(CommandError::BadArgs("not a mesh".into()));
    };
    if m.deformer == a.deformer {
        return changed(crate::Change { label: String::new(), changed: Vec::new() }, json!({}));
    }
    for f in &mut m.keyforms.forms {
        f.positions = rehome(s.project(), m.deformer, a.deformer, &f.positions)?.into();
    }
    m.deformer = a.deformer;
    let ((), c) = s.edit(format!("Attach {}", m.meta.name), |e| Ok(e.update(a.id, |o| *o = Object::Mesh(m))?))?;
    changed(c, json!({}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Remesh {
    #[schemars(with = "schema::Id")]
    id: ObjId,
    /// Cells along the texture's longer side.
    cells: u32,
}

fn remesh(s: &mut Session, a: Remesh) -> CommandResult {
    let Object::Mesh(mut m) = object(s.project(), a.id)? else {
        return Err(CommandError::BadArgs("not a mesh".into()));
    };
    if !m.keyforms.bindings.is_empty() || m.keyforms.forms.len() != 1 {
        return Err(CommandError::Invalid("the mesh has keyforms; remesh it before binding parameters".into()));
    }
    let pixels = s.textures().get(&m.texture).ok_or(CommandError::NotFound(m.texture))?;
    // A flat mesh places its texture by a scale and offset per axis: recover them.
    let fit = |pick: fn(Vec2) -> f32| -> Option<(f32, f32)> {
        let pts: Vec<(f32, f32)> =
            m.uvs.iter().zip(m.keyforms.forms[0].positions.iter()).map(|(&u, &p)| (pick(u), pick(p))).collect();
        let n = pts.len() as f32;
        let (su, sp) = pts.iter().fold((0.0, 0.0), |(a, b), &(u, p)| (a + u, b + p));
        let (mu, mp) = (su / n, sp / n);
        let (cov, var) =
            pts.iter().fold((0.0, 0.0), |(c, v), &(u, p)| (c + (u - mu) * (p - mp), v + (u - mu) * (u - mu)));
        if var <= 0.0 {
            return None;
        }
        let scale = cov / var;
        let offset = mp - scale * mu;
        pts.iter().all(|&(u, p)| (scale * u + offset - p).abs() < 0.05).then_some((scale, offset))
    };
    let (Some((sx, ox)), Some((sy, oy))) = (fit(|v| v.x), fit(|v| v.y)) else {
        return Err(CommandError::Invalid("the mesh has been reshaped; only flat meshes can be remeshed".into()));
    };
    let generated = auto_mesh(pixels.size, MeshOptions { cells: a.cells.clamp(1, 256), alpha_threshold: 0 }, |x, y| {
        pixels.get(x, y)[3]
    });
    if generated.indices.is_empty() {
        return Err(CommandError::Invalid("the texture has no visible pixels".into()));
    }
    let positions: Arc<[Vec2]> = generated.uvs.iter().map(|uv| Vec2::new(sx * uv.x + ox, sy * uv.y + oy)).collect();
    m.uvs = generated.uvs.into();
    m.indices = generated.indices.into();
    m.keyforms.forms[0].positions = positions;
    let (vertices, triangles) = (m.uvs.len(), m.indices.len() / 3);
    let ((), c) = s.edit(format!("Remesh {}", m.meta.name), |e| Ok(e.update(a.id, |o| *o = Object::Mesh(m))?))?;
    changed(c, json!({ "vertices": vertices, "triangles": triangles }))
}
