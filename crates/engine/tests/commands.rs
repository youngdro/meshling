//! The command surface end to end, driven the way the UI, CLI and MCP server drive it:
//! command names and JSON in, JSON out.

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::PathBuf;

use meshling_engine::{CommandError, Registry, Session};
use meshling_io::psd_writer::{Spec, write_psd};
use meshling_model::Provenance;
use serde_json::{Value, json};

struct Harness {
    r: Registry,
    s: Session,
    dir: PathBuf,
}

impl Harness {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("meshling-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { r: Registry::standard(), s: Session::default(), dir }
    }

    fn run(&mut self, name: &str, args: Value) -> Value {
        self.r.run(&mut self.s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn fail(&mut self, name: &str, args: Value) -> CommandError {
        match self.r.run(&mut self.s, name, args) {
            Ok(v) => panic!("{name} should fail, gave {v}"),
            Err(e) => e,
        }
    }

    fn id(v: &Value) -> Value {
        v["id"].clone()
    }

    /// A head (face, eye) and a body, 64 × 48.
    fn psd(&self) -> PathBuf {
        let path = self.dir.join("girl.psd");
        let bytes = write_psd(
            64,
            48,
            &[
                Spec::pixel("body", 16, 28, 32, 20, |x, _| [40, 80, (x * 6) as u8, 255]),
                Spec::folder("</Layer group>", 3),
                Spec::pixel("face", 20, 4, 24, 24, |x, y| [230, 200, 180, if (x + y) % 9 == 0 { 200 } else { 255 }]),
                Spec::pixel("eye", 26, 12, 6, 4, |_, _| [20, 30, 90, 255]),
                Spec::folder("head", 1),
            ],
        );
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn render(&mut self, values: Value) -> String {
        self.run("render.png", json!({ "values": values }))["pngBase64"].as_str().unwrap().to_string()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn find_mesh(tree: &Value, name: &str) -> Value {
    fn walk(part: &Value, name: &str) -> Option<Value> {
        part["meshes"]
            .as_array()?
            .iter()
            .find(|m| m["name"] == name)
            .cloned()
            .or_else(|| part["parts"].as_array()?.iter().find_map(|p| walk(p, name)))
    }
    tree["parts"].as_array().unwrap().iter().find_map(|p| walk(p, name)).unwrap()
}

#[test]
fn every_command_has_a_summary_and_an_object_schema() {
    let r = Registry::standard();
    let specs: Vec<_> = r.specs().collect();
    assert!(specs.len() >= 30, "{} commands", specs.len());
    for spec in specs {
        assert!(!spec.summary.is_empty(), "{}", spec.name);
        assert_eq!(spec.input_schema["type"], "object", "{}: {}", spec.name, spec.input_schema);
    }
    assert!(r.spec("import.psd").unwrap().mutates);
    assert!(!r.spec("project.tree").unwrap().mutates);
}

#[test]
fn import_rig_and_animate_a_head() {
    let mut h = Harness::new("rig");
    h.s.set_actor(Provenance::Agent { session: "test".into() });
    h.run("project.new", json!({ "width": 64, "height": 48 }));
    let psd = h.psd();
    let imported = h.run("import.psd", json!({ "path": psd, "cells": 6 }));
    assert_eq!(imported["layers"].as_array().unwrap().len(), 3);
    assert_eq!(imported["warnings"], json!([]));
    let rest = h.render(json!({}));

    let tree = h.run("project.tree", json!({}));
    assert_eq!(tree["parts"][0]["name"], "girl.psd");
    assert_eq!(tree["parts"][0]["parts"][0]["name"], "head");
    let face = find_mesh(&tree, "face")["id"].clone();
    let eye = find_mesh(&tree, "eye")["id"].clone();
    let found = h.run("object.find", json!({ "name": "EY", "kind": "mesh" }));
    assert_eq!(found.as_array().unwrap().len(), 1);

    // A head rotation, with the face and a warp holding the eye under it.
    let angle = Harness::id(&h.run("param.create", json!({ "name": "AngleZ", "min": -30, "max": 30 })));
    let head = Harness::id(&h.run("deformer.create_rotation", json!({ "name": "head", "origin": [32, 28] })));
    let eye_warp = Harness::id(&h.run(
        "deformer.create_warp",
        json!({ "name": "eye", "rect": [24, 10, 34, 18], "rows": 2, "cols": 2, "parent": head }),
    ));
    h.run("mesh.set_deformer", json!({ "id": face, "deformer": head }));
    h.run("mesh.set_deformer", json!({ "id": eye, "deformer": eye_warp }));
    assert_eq!(h.render(json!({})), rest, "attaching to deformers keeps everything in place");

    // Turn the head with AngleZ.
    h.run("keyform.bind", json!({ "id": head, "param": angle, "keys": [-30, 0, 30] }));
    h.run("keyform.set_rotation", json!({ "id": head, "at": { angle.as_str().unwrap(): 30 }, "angle": 15 }));
    h.run("keyform.set_rotation", json!({ "id": head, "at": { angle.as_str().unwrap(): -30 }, "angle": -15 }));
    assert_eq!(h.render(json!({})), rest, "the rest pose is unchanged");
    let turned = h.render(json!({ angle.as_str().unwrap(): 30 }));
    assert_ne!(turned, rest);

    // Blink: squash the eye mesh vertically at EyeOpen = 0, in canvas pixels.
    let open = Harness::id(&h.run("param.create", json!({ "name": "EyeOpen", "min": 0, "max": 1, "default": 1 })));
    h.run("keyform.bind", json!({ "id": eye, "param": open, "keys": [0, 1] }));
    let eye_obj = h.run("object.get", json!({ "id": eye }));
    let n = eye_obj["object"]["uvs"].as_array().unwrap().len();
    let squash: Vec<Value> = eye_obj["object"]["keyforms"]["forms"][0]["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|_| json!([0.0, 2.0]))
        .collect();
    assert_eq!(squash.len(), n);
    let shut =
        h.run("keyform.set_mesh", json!({ "id": eye, "at": { open.as_str().unwrap(): 0 }, "canvasOffsets": squash }));
    assert_eq!(shut["form"], 0);
    assert_ne!(h.render(json!({ open.as_str().unwrap(): 0 })), rest);
    // Through the warp and the rotation, every eye vertex lands exactly 2 px lower on screen.
    let eye_id: meshling_model::ObjId = eye.as_str().unwrap().parse().unwrap();
    let open_id: meshling_model::ObjId = open.as_str().unwrap().parse().unwrap();
    let posed = |v: f32| {
        let pose = meshling_rig::evaluate(h.s.project(), &meshling_rig::ParamValues::new().with(open_id, v)).unwrap();
        pose.mesh(eye_id).unwrap().positions.clone()
    };
    for (a, b) in posed(1.0).iter().zip(posed(0.0)) {
        assert!((b - *a - meshling_geom::Vec2::new(0.0, 2.0)).length() < 1e-3, "{a} -> {b}");
    }

    let report = h.run("validate.run", json!({}));
    assert_eq!(report["errors"], 0, "{report}");

    // Everything is attributed to the agent.
    let head_obj = h.run("object.get", json!({ "id": head }));
    assert_eq!(head_obj["object"]["meta"]["provenance"], json!({ "by": "agent", "session": "test" }));

    // Undo everything back to the empty canvas, then redo it all.
    let after = h.s.project().clone();
    let mut steps = 0;
    while h.run("history.undo", json!({}))["undone"] != Value::Null {
        steps += 1;
    }
    assert_eq!(steps, 12);
    assert!(h.s.project().meshes.is_empty());
    while h.run("history.redo", json!({}))["redone"] != Value::Null {}
    assert_eq!(h.s.project(), &after);
    assert_eq!(h.render(json!({ angle.as_str().unwrap(): 30 })), turned);
}

#[test]
fn save_open_and_dirty_tracking() {
    let mut h = Harness::new("save");
    h.run("project.new", json!({ "width": 64, "height": 48 }));
    let psd = h.psd();
    h.run("import.psd", json!({ "path": psd }));
    assert_eq!(h.run("project.info", json!({}))["dirty"], true);
    let err = h.fail("project.save", json!({}));
    assert_eq!(err.code(), "bad-args");
    let dir = h.dir.join("girl.meshling");
    h.run("project.save", json!({ "path": dir }));
    assert_eq!(h.run("project.info", json!({}))["dirty"], false);
    let saved = h.s.project().clone();
    let picture = h.render(json!({}));

    h.run("project.new", json!({ "width": 8, "height": 8 }));
    let info = h.run("project.open", json!({ "path": dir }));
    assert_eq!(info["counts"]["meshes"], 3);
    assert_eq!(h.s.project(), &saved);
    assert_eq!(h.render(json!({})), picture);
    assert_eq!(info["undo"], json!([]), "opening starts a fresh history");
}

#[test]
fn deleting_a_part_takes_its_subtree_and_refusals_change_nothing() {
    let mut h = Harness::new("delete");
    h.run("project.new", json!({ "width": 64, "height": 48 }));
    let psd = h.psd();
    let imported = h.run("import.psd", json!({ "path": psd }));
    let root = imported["root"].clone();

    let p = Harness::id(&h.run("param.create", json!({ "name": "P", "min": 0, "max": 1 })));
    let body = imported["layers"][0]["mesh"].clone();
    h.run("keyform.bind", json!({ "id": body, "param": p, "keys": [0, 1] }));
    let before = h.s.project().clone();
    let err = h.fail("object.delete", json!({ "id": p }));
    assert_eq!(err.code(), "edit-rejected", "{err}");
    assert_eq!(h.s.project(), &before);

    let head = h.run("object.find", json!({ "name": "head", "kind": "part" }))[0]["id"].clone();
    let deleted = h.run("object.delete", json!({ "id": head }));
    assert_eq!(deleted["changed"].as_array().unwrap().len(), 3, "head part, face and eye meshes");
    h.run("object.delete", json!({ "id": root }));
    assert!(h.s.project().meshes.is_empty() && h.s.project().parts.is_empty());
    assert_eq!(h.s.project().textures.len(), 3, "textures stay until deleted themselves");
}

#[test]
fn bad_input_is_rejected_with_a_code() {
    let mut h = Harness::new("errors");
    assert_eq!(h.fail("no.such", json!({})).code(), "unknown-command");
    assert_eq!(h.fail("part.create", json!({ "name": "x", "colour": "red" })).code(), "bad-args");
    assert_eq!(h.fail("object.get", json!({ "id": "01J00000000000000000000000" })).code(), "not-found");
    let part = Harness::id(&h.run("part.create", json!({ "name": "a" })));
    let param = Harness::id(&h.run("param.create", json!({ "name": "p", "min": 0, "max": 1 })));
    assert_eq!(h.fail("keyform.bind", json!({ "id": part, "param": param, "keys": [0, 1] })).code(), "bad-args");
    let ghost = "not-an-id";
    assert_eq!(h.fail("part.update", json!({ "id": part, "parent": ghost })).code(), "bad-args");
    assert_eq!(
        h.fail("part.update", json!({ "id": part, "parent": "01J00000000000000000000000" })).code(),
        "edit-rejected"
    );
    h.run("part.update", json!({ "id": part, "parent": null, "visible": false }));
    assert!(!h.s.project().parts[0].visible);
}

/// The real See-through export (licence unclear, so not in the repo):
/// `MESHLING_SAMPLE_PSD=/path/to/seethrough_output.psd cargo test -p meshling-engine -- --ignored --nocapture`
#[test]
#[ignore = "needs MESHLING_SAMPLE_PSD"]
fn imports_and_renders_a_real_see_through_export() {
    let path = std::env::var("MESHLING_SAMPLE_PSD").expect("set MESHLING_SAMPLE_PSD");
    let mut h = Harness::new("sample");
    h.run("project.new", json!({ "width": 768, "height": 768 }));
    let imported = h.run("import.psd", json!({ "path": path }));
    let layers = imported["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 16);
    for l in layers {
        eprintln!("{:12} {:>4} triangles", l["name"].as_str().unwrap(), l["triangles"]);
    }
    assert_eq!(imported["warnings"], json!([]));
    let report = h.run("validate.run", json!({}));
    assert_eq!(report["errors"], 0, "{report}");

    // The CPU render of the import against the document's stored image (straight colour,
    // alpha dropped: premultiply it by the render's alpha).
    let out = h.dir.join("render.png");
    h.run("render.png", json!({ "path": out }));
    let ours = meshling_io::decode_png(&std::fs::read(&out).unwrap()).unwrap();
    let doc = meshling_io::read_psd(&std::fs::read(&path).unwrap()).unwrap();
    let theirs = doc.composite.unwrap();
    let mut sum = 0.0;
    for (o, t) in ours.rgba.chunks(4).zip(theirs.rgba.chunks(4)) {
        for k in 0..3 {
            sum += (f64::from(o[k]) - f64::from(t[k]) * f64::from(o[3]) / 255.0).abs();
        }
    }
    let mean = sum / (ours.rgba.len() / 4 * 3) as f64;
    eprintln!("mean difference from the PSD's own composite: {mean:.3}/255");
    assert!(mean < 0.5);
    if let Ok(keep) = std::env::var("MESHLING_SAMPLE_RENDER") {
        std::fs::copy(&out, keep).unwrap();
    }
}
