//! Whole-project commands: create, open, save, inspect, import, undo, validate, render.

use std::collections::BTreeMap;
use std::path::PathBuf;

use meshling_geom::Size;
use meshling_io::{ImportOptions, import_psd, read_psd};
use meshling_mesh::MeshOptions;
use meshling_model::{DeformerKind, ObjId, Project};
use meshling_ops::{Kind, Object};
use meshling_plan::{Pixels, Textures, plan};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{changed, object, ok, schema};
use crate::{CommandError, CommandResult, Registry, Session};

pub(super) fn register(r: &mut Registry) {
    r.add("project.new", "Start a new empty project with a canvas size. Clears undo history.", false, new);
    r.add("project.open", "Open a project directory (project.json + textures/).", false, open);
    r.add(
        "project.save",
        "Save the project to a directory (defaults to where it was opened or last saved).",
        false,
        save,
    );
    r.add("project.info", "Canvas, object counts, unsaved changes, undo/redo steps.", false, info);
    r.add(
        "project.tree",
        "Parts with their meshes, deformers, parameters and textures: the map to navigate by.",
        false,
        tree,
    );
    r.add("object.get", "Full JSON of one object by id.", false, get);
    r.add("object.find", "Find objects by name (case-insensitive substring), tag and/or kind.", false, find);
    r.add("values.set", "Set preview parameter values (not saved, not undoable).", false, set_values);
    r.add("history.undo", "Undo the last change.", true, undo);
    r.add("history.redo", "Redo the last undone change.", true, redo);
    r.add(
        "validate.run",
        "Check structure, textures, parameters and fold-overs across a parameter sweep.",
        false,
        validate,
    );
    r.add("render.png", "Render with the CPU reference renderer to a PNG file, or return it as base64.", false, render);
    r.add(
        "import.psd",
        "Import a PSD: a part per group, a texture and auto-mesh per layer, in stacking order.",
        true,
        import,
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct New {
    width: u32,
    height: u32,
}

fn new(s: &mut Session, a: New) -> CommandResult {
    if a.width == 0 || a.height == 0 || a.width > 16384 || a.height > 16384 {
        return Err(CommandError::BadArgs("canvas sides must be 1..=16384".into()));
    }
    s.replace(Project::new(Size::new(a.width, a.height)), Textures::new(), None);
    info(s, Empty {})
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathArg {
    path: PathBuf,
}

fn open(s: &mut Session, a: PathArg) -> CommandResult {
    let (project, textures) = meshling_io::load_project(&a.path).map_err(|e| CommandError::Io(e.to_string()))?;
    s.replace(project, textures, Some(a.path));
    info(s, Empty {})
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Save {
    /// Directory to save into; omit to save where the project came from.
    path: Option<PathBuf>,
}

fn save(s: &mut Session, a: Save) -> CommandResult {
    let dir = a
        .path
        .or_else(|| s.dir().cloned())
        .ok_or_else(|| CommandError::BadArgs("no path given and the project has never been saved".into()))?;
    meshling_io::save_project(&dir, s.project(), s.textures()).map_err(|e| CommandError::Io(e.to_string()))?;
    s.mark_saved(dir.clone());
    ok(json!({ "path": dir }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn info(s: &mut Session, _: Empty) -> CommandResult {
    let p = s.project();
    ok(json!({
        "canvas": p.canvas,
        "counts": {
            "textures": p.textures.len(),
            "parts": p.parts.len(),
            "params": p.params.len(),
            "deformers": p.deformers.len(),
            "meshes": p.meshes.len(),
        },
        "dirty": s.is_dirty(),
        "path": s.dir(),
        "actor": s.actor(),
        "undo": s.history().undo_labels().take(10).collect::<Vec<_>>(),
        "redo": s.history().redo_labels().take(10).collect::<Vec<_>>(),
    }))
}

fn tree(s: &mut Session, _: Empty) -> CommandResult {
    let p = s.project();
    fn part_node(p: &Project, id: ObjId) -> Value {
        let Some(part) = p.part(id) else { return Value::Null };
        let children: Vec<Value> =
            p.parts.iter().filter(|c| c.parent == Some(id)).map(|c| part_node(p, c.meta.id)).collect();
        let meshes: Vec<Value> = p
            .meshes
            .iter()
            .filter(|m| m.part == id)
            .map(|m| {
                json!({
                    "id": m.meta.id, "name": m.meta.name, "tags": m.meta.tags, "texture": m.texture,
                    "deformer": m.deformer, "drawOrder": m.draw_order, "blend": m.blend,
                    "vertices": m.vertex_count(), "triangles": m.indices.len() / 3,
                    "bindings": m.keyforms.bindings.iter().map(|b| b.param).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "id": id, "name": part.meta.name, "tags": part.meta.tags, "visible": part.visible,
            "parts": children, "meshes": meshes,
        })
    }
    let roots: Vec<Value> = p.parts.iter().filter(|q| q.parent.is_none()).map(|q| part_node(p, q.meta.id)).collect();
    let deformers: Vec<Value> = p
        .deformers
        .iter()
        .map(|d| {
            let (kind, bindings) = match &d.kind {
                DeformerKind::Warp { rows, cols, keyforms } => {
                    (json!({"type": "warp", "rows": rows, "cols": cols}), &keyforms.bindings)
                }
                DeformerKind::Rotation { keyforms } => (json!({"type": "rotation"}), &keyforms.bindings),
            };
            json!({
                "id": d.meta.id, "name": d.meta.name, "tags": d.meta.tags, "parent": d.parent, "kind": kind,
                "bindings": bindings.iter().map(|b| b.param).collect::<Vec<_>>(),
            })
        })
        .collect();
    let params: Vec<Value> = p
        .params
        .iter()
        .map(|q| {
            json!({
                "id": q.meta.id, "name": q.meta.name, "tags": q.meta.tags, "min": q.min, "max": q.max,
                "default": q.default, "value": s.values().get(p, q.meta.id),
            })
        })
        .collect();
    let textures: Vec<Value> =
        p.textures.iter().map(|t| json!({"id": t.meta.id, "name": t.meta.name, "size": t.size})).collect();
    ok(json!({ "canvas": p.canvas, "parts": roots, "deformers": deformers, "params": params, "textures": textures }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArg {
    #[schemars(with = "schema::Id")]
    id: ObjId,
}

fn get(s: &mut Session, a: IdArg) -> CommandResult {
    ok(object(s.project(), a.id)?)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Find {
    name: Option<String>,
    tag: Option<String>,
    #[schemars(with = "Option<schema::Kind>")]
    kind: Option<Kind>,
}

fn find(s: &mut Session, a: Find) -> CommandResult {
    let needle = a.name.map(|n| n.to_lowercase());
    let p = s.project();
    let found: Vec<Value> = p
        .metas()
        .filter_map(|m| meshling_ops::find(p, m.id))
        .filter(|o| a.kind.is_none_or(|k| o.kind() == k))
        .filter(|o| needle.as_ref().is_none_or(|n| o.meta().name.to_lowercase().contains(n)))
        .filter(|o| a.tag.as_ref().is_none_or(|t| o.meta().tags.contains(t)))
        .map(|o| json!({"id": o.id(), "kind": o.kind(), "name": o.meta().name, "tags": o.meta().tags}))
        .collect();
    ok(found)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetValues {
    /// Parameter id → value. Values are clamped to each parameter's range when posing.
    #[schemars(with = "BTreeMap<String, f32>")]
    values: BTreeMap<ObjId, f32>,
    /// Reset every other parameter to its default first.
    #[serde(default)]
    reset: bool,
}

fn set_values(s: &mut Session, a: SetValues) -> CommandResult {
    for &id in a.values.keys() {
        if s.project().param(id).is_none() {
            return Err(CommandError::NotFound(id));
        }
    }
    if a.reset {
        *s.values_mut() = meshling_rig::ParamValues::new();
    }
    for (&id, &v) in &a.values {
        s.values_mut().set(id, v);
    }
    let p = s.project();
    let out: BTreeMap<String, f32> =
        p.params.iter().map(|q| (q.meta.id.to_string(), s.values().get(p, q.meta.id))).collect();
    ok(json!({ "values": out }))
}

fn undo(s: &mut Session, _: Empty) -> CommandResult {
    ok(json!({ "undone": s.undo()? }))
}

fn redo(s: &mut Session, _: Empty) -> CommandResult {
    ok(json!({ "redone": s.redo()? }))
}

fn validate(s: &mut Session, _: Empty) -> CommandResult {
    let report = meshling_validate::validate(s.project(), Some(s.textures()), &meshling_validate::Options::default());
    ok(json!({ "errors": report.errors(), "warnings": report.warnings(), "issues": report.issues }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Render {
    /// Write the PNG here; omit to get it back as base64.
    path: Option<PathBuf>,
    /// Parameter values to render at; omitted parameters use the session's preview values.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, f32>")]
    values: BTreeMap<ObjId, f32>,
}

fn render(s: &mut Session, a: Render) -> CommandResult {
    let mut values = s.values().clone();
    for (&id, &v) in &a.values {
        values.set(id, v);
    }
    let p = s.project();
    let pose = meshling_rig::evaluate(p, &values)?;
    let image = meshling_render_cpu::render(p, &pose, &plan(p, &pose), s.textures())
        .map_err(|e| CommandError::Invalid(e.to_string()))?;
    let pixels =
        Pixels::new(image.size, image.to_rgba8()).ok_or_else(|| CommandError::Invalid("render size".into()))?;
    let png = meshling_io::encode_png(&pixels).map_err(|e| CommandError::Io(e.to_string()))?;
    let size = json!({ "width": image.size.width, "height": image.size.height });
    match a.path {
        Some(path) => {
            std::fs::write(&path, &png).map_err(|e| CommandError::Io(format!("{}: {e}", path.display())))?;
            ok(json!({ "path": path, "size": size }))
        }
        None => ok(json!({ "pngBase64": base64(&png), "size": size })),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Import {
    path: PathBuf,
    /// Name of the part holding the import; defaults to the file name.
    root_name: Option<String>,
    /// Part to put the import under; omit for a new top-level part.
    #[schemars(with = "Option<schema::Id>")]
    parent: Option<ObjId>,
    /// Mesh cells along each layer's longer side (default 12).
    cells: Option<u32>,
}

fn import(s: &mut Session, a: Import) -> CommandResult {
    let bytes = std::fs::read(&a.path).map_err(|e| CommandError::Io(format!("{}: {e}", a.path.display())))?;
    let doc = read_psd(&bytes).map_err(|e| CommandError::Invalid(format!("{}: {e}", a.path.display())))?;
    if let Some(parent) = a.parent
        && s.project().part(parent).is_none()
    {
        return Err(CommandError::NotFound(parent));
    }
    let top = s.project().meshes.iter().map(|m| m.draw_order).max();
    let opts = ImportOptions {
        root_name: a
            .root_name
            .unwrap_or_else(|| a.path.file_name().map_or_else(|| "psd".into(), |n| n.to_string_lossy().into_owned())),
        parent: a.parent,
        mesh: MeshOptions { cells: a.cells.unwrap_or(12).clamp(1, 256), ..MeshOptions::default() },
        draw_order_start: top.map_or(0, |t| t.saturating_add(10)),
        ..ImportOptions::default()
    };
    let imported = import_psd(&doc, &opts);
    for (&id, px) in &imported.pixels {
        s.add_pixels(id, px.clone());
    }
    let root = imported.parts.first().map(|p| p.meta.id);
    let label = format!("Import {}", opts.root_name);
    let ((), change) = s.edit(label, |e| {
        for o in imported.parts.iter().cloned().map(Object::Part) {
            e.insert(o)?;
        }
        for o in imported.textures.iter().cloned().map(Object::Texture) {
            e.insert(o)?;
        }
        for o in imported.meshes.iter().cloned().map(Object::Mesh) {
            e.insert(o)?;
        }
        Ok(())
    })?;
    let layers: Vec<Value> = imported
        .meshes
        .iter()
        .map(|m| json!({"mesh": m.meta.id, "name": m.meta.name, "drawOrder": m.draw_order, "triangles": m.indices.len() / 3}))
        .collect();
    let canvas_differs = doc.size != s.project().canvas;
    let mut warnings = imported.warnings;
    if canvas_differs {
        warnings.push(format!(
            "the PSD is {}×{} but the canvas is {}×{}; layers keep their PSD pixel positions",
            doc.size.width,
            doc.size.height,
            s.project().canvas.width,
            s.project().canvas.height
        ));
    }
    changed(change, json!({ "root": root, "layers": layers, "warnings": warnings, "psdSize": doc.size }))
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(T[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            assert_eq!(super::base64(plain.as_bytes()), encoded);
        }
    }
}
