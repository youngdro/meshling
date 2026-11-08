//! PSD → project objects → render, and the project directory format.

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use meshling_io::import::PSD_TAG;
use meshling_io::psd_writer::{Spec, write_psd};
use meshling_io::{ImportOptions, Imported, PsdDocument, import_psd, load_project, read_psd, save_project};
use meshling_model::{Blend, Project};
use meshling_plan::{Pixels, plan};
use meshling_rig::{ParamValues, evaluate};

fn project_of(doc: &PsdDocument, imported: &Imported) -> Project {
    let mut p = Project::new(doc.size);
    p.parts = imported.parts.clone();
    p.textures = imported.textures.clone();
    p.meshes = imported.meshes.clone();
    p
}

/// Normal-composite the layers straight onto the canvas: what the import must reproduce.
fn recomposite(doc: &PsdDocument) -> Vec<[f32; 4]> {
    let (w, h) = (doc.size.width as i32, doc.size.height as i32);
    let mut out = vec![[0f32; 4]; (w * h) as usize];
    for l in doc.layers.iter().filter(|l| l.visible) {
        let k = f32::from(l.opacity) / 255.0;
        for y in 0..l.pixels.size.height {
            for x in 0..l.pixels.size.width {
                let (cx, cy) = (l.left + x as i32, l.top + y as i32);
                if cx < 0 || cy < 0 || cx >= w || cy >= h {
                    continue;
                }
                let s = l.pixels.get(x, y).map(|c| f32::from(c) / 255.0 * k);
                let d = &mut out[(cy * w + cx) as usize];
                *d = meshling_render_cpu::composite(l.blend, s, *d);
            }
        }
    }
    out
}

fn sample_psd() -> PsdDocument {
    let soft = |x: u32, y: u32| {
        let a = ((x * 37 + y * 11) % 256) as u8;
        [(x * 20) as u8, (y * 9) as u8, 120, a]
    };
    read_psd(&write_psd(
        48,
        40,
        &[
            Spec::pixel("body", 4, 10, 30, 28, soft),
            Spec::folder("</Layer group>", 3),
            Spec { opacity: 140, ..Spec::pixel("shade", 10, 12, 9, 7, |_, _| [40, 40, 90, 255]) },
            Spec { hidden: true, ..Spec::pixel("closed eye", 20, 5, 6, 3, |_, _| [0, 0, 0, 255]) },
            Spec { blend: b"mul ", ..Spec::pixel("tint", 0, 0, 48, 40, |x, _| [255, 128 + x as u8, 200, 255]) },
            Spec::folder("head", 1),
            Spec::pixel("nose", 30, 20, 2, 3, |_, _| [250, 180, 160, 255]),
            Spec::pixel("off canvas", -10, -4, 6, 3, |_, _| [9, 9, 9, 255]),
        ],
    ))
    .unwrap()
}

#[test]
fn import_builds_a_part_tree_and_meshes_in_stacking_order() {
    let doc = sample_psd();
    let imported = import_psd(&doc, &ImportOptions { root_name: "girl.psd".into(), ..Default::default() });
    let parts: Vec<_> = imported
        .parts
        .iter()
        .map(|p| {
            let parent = p.parent.and_then(|id| imported.parts.iter().find(|q| q.meta.id == id));
            (p.meta.name.as_str(), parent.map(|q| q.meta.name.as_str()), p.visible)
        })
        .collect();
    assert_eq!(
        parts,
        [("girl.psd", None, true), ("head", Some("girl.psd"), true), ("closed eye", Some("head"), false)]
    );
    let meshes: Vec<_> = imported.meshes.iter().map(|m| (m.meta.name.as_str(), m.draw_order, m.blend)).collect();
    assert_eq!(
        meshes,
        [
            ("body", 0, Blend::Normal),
            ("shade", 10, Blend::Normal),
            ("closed eye", 20, Blend::Normal),
            ("tint", 30, Blend::Multiply),
            ("nose", 40, Blend::Normal),
            ("off canvas", 50, Blend::Normal),
        ]
    );
    assert!((imported.meshes[1].keyforms.forms[0].opacity - 140.0 / 255.0).abs() < 1e-6);
    assert!(
        imported
            .parts
            .iter()
            .map(|p| &p.meta)
            .chain(imported.meshes.iter().map(|m| &m.meta))
            .all(|m| m.tags == [PSD_TAG])
    );
    assert_eq!(imported.warnings.len(), 1, "{:?}", imported.warnings);
    assert!(imported.warnings[0].contains("off canvas"));

    let project = project_of(&doc, &imported);
    assert_eq!(project.check(), []);
}

#[test]
fn rendering_the_import_reproduces_the_layer_stack() {
    let doc = sample_psd();
    let imported = import_psd(&doc, &ImportOptions::default());
    let project = project_of(&doc, &imported);
    let pose = evaluate(&project, &ParamValues::new()).unwrap();
    let image = meshling_render_cpu::render(&project, &pose, &plan(&project, &pose), &imported.pixels).unwrap();
    let want = recomposite(&doc);
    let worst = image
        .pixels
        .iter()
        .zip(&want)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
        .fold(0f32, f32::max);
    assert!(worst <= 1.0 / 255.0, "worst channel difference {}", worst * 255.0);
}

#[test]
fn project_directories_round_trip_and_reject_tampering() {
    let doc = sample_psd();
    let imported = import_psd(&doc, &ImportOptions::default());
    let project = project_of(&doc, &imported);
    let dir = std::env::temp_dir().join(format!("meshling-io-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    save_project(&dir, &project, &imported.pixels).unwrap();
    let (loaded, pixels) = load_project(&dir).unwrap();
    assert_eq!(loaded, project);
    assert_eq!(pixels, imported.pixels);
    save_project(&dir, &project, &imported.pixels).unwrap(); // idempotent

    // Pixels that don't match their asset are refused on save…
    let mut wrong = imported.pixels.clone();
    let first = project.textures[0].meta.id;
    wrong.insert(first, Pixels::solid(project.textures[0].size, [1, 1, 1, 255]));
    assert!(save_project(&dir, &project, &wrong).is_err());

    // …and a texture edited on disk is caught on load.
    let file = dir.join("textures").join(format!("{}.png", project.textures[0].content_hash));
    let tampered = meshling_io::encode_png(&Pixels::solid(project.textures[0].size, [0, 0, 0, 255])).unwrap();
    std::fs::write(&file, tampered).unwrap();
    assert!(load_project(&dir).is_err());

    // A hash that tries to name another file is rejected before touching the disk.
    let mut sneaky = project.clone();
    sneaky.textures[0].content_hash = "../../etc/passwd".into();
    assert!(save_project(&dir, &sneaky, &imported.pixels).is_err());
    std::fs::write(dir.join("project.json"), sneaky.to_json()).unwrap();
    assert!(load_project(&dir).is_err());

    std::fs::remove_dir_all(&dir).unwrap();
}
