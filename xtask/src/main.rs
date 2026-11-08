//! Build tasks.
//!
//! - `cargo xtask layers`: every workspace crate may depend only on crates in lower layers, and
//!   nothing below L6 may depend on a UI toolkit or windowing crate.
//! - `cargo xtask wasm`: `cargo check` every crate in L0–L5 for `wasm32-unknown-unknown`.

use std::collections::BTreeMap;
use std::process::{Command, ExitCode};

/// The architecture's layer table (docs: "整体分层与 crate 划分"). A new crate must be added here.
const LAYERS: &[(&str, u8)] = &[
    ("meshling-geom", 0),
    ("meshling-model", 1),
    ("meshling-rig", 2),
    ("meshling-physics", 2),
    ("meshling-mesh", 2),
    ("meshling-ops", 2),
    ("meshling-plan", 3),
    ("meshling-render-cpu", 3),
    ("meshling-render-gpu", 3),
    ("meshling-io", 4),
    ("meshling-validate", 4),
    ("meshling-ai", 4),
    ("meshling-engine", 5),
    ("meshling-ui-egui", 6),
    ("meshling-automation", 6),
    ("meshling-editor", 7),
    ("meshling-cli", 7),
];

/// Same-layer dependencies the architecture allows: both backends execute the planner's output,
/// and the GPU backend's parity tests use the CPU renderer as their oracle (dev-dependency only).
const SIDEWAYS: &[(&str, &str)] = &[
    ("meshling-render-cpu", "meshling-plan"),
    ("meshling-render-gpu", "meshling-plan"),
    ("meshling-render-gpu", "meshling-render-cpu"),
];

/// Crates that tie code to a UI toolkit or a window; only L6 and above may use them.
const UI_CRATES: &[&str] = &["egui", "eframe", "egui-wgpu", "egui-winit", "winit", "rfd"];

/// Highest layer that must build for the web.
const WASM_MAX_LAYER: u8 = 5;

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "layers" => layers(),
        "wasm" => wasm(),
        _ => Err("usage: cargo xtask <layers|wasm>".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn layers() -> Result<(), String> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata output: {e}"))?;
    let packages = meta["packages"].as_array().ok_or("cargo metadata: no packages")?;
    let deps: BTreeMap<String, Vec<String>> = packages
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let deps =
                p["dependencies"].as_array()?.iter().filter_map(|d| d["name"].as_str().map(String::from)).collect();
            Some((name, deps))
        })
        .collect();
    let errors = check_layers(&deps);
    if errors.is_empty() {
        println!("layers: {} crates OK", LAYERS.len());
        Ok(())
    } else {
        Err(format!("layer check failed:\n  {}", errors.join("\n  ")))
    }
}

fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

fn check_layers(deps: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, list) in deps {
        if name == "xtask" {
            continue;
        }
        let Some(layer) = layer_of(name) else {
            errors.push(format!("{name}: not in the layer table (xtask/src/main.rs)"));
            continue;
        };
        for dep in list {
            if dep == name {
                continue; // a crate dev-depending on itself, only to switch on test features
            }
            if let Some(dl) = layer_of(dep) {
                if dl > layer || (dl == layer && !SIDEWAYS.contains(&(name.as_str(), dep.as_str()))) {
                    errors.push(format!("{name} (L{layer}) depends on {dep} (L{dl}): only lower layers (or a listed SIDEWAYS pair) are allowed"));
                }
            } else if layer <= WASM_MAX_LAYER && UI_CRATES.contains(&dep.as_str()) {
                errors.push(format!("{name} (L{layer}) depends on UI crate {dep}: only L6 and above may"));
            }
        }
    }
    errors
}

fn wasm() -> Result<(), String> {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["check", "--target", "wasm32-unknown-unknown"]);
    for (name, layer) in LAYERS {
        if *layer <= WASM_MAX_LAYER {
            cmd.args(["-p", name]);
        }
    }
    let status = cmd.status().map_err(|e| format!("cargo check: {e}"))?;
    if status.success() { Ok(()) } else { Err("wasm check failed".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        edges.iter().map(|(n, d)| (n.to_string(), d.iter().map(|s| s.to_string()).collect())).collect()
    }

    #[test]
    fn accepts_downward_dependencies() {
        assert!(check_layers(&graph(&[("meshling-rig", &["meshling-model", "meshling-geom", "glam"])])).is_empty());
        assert!(check_layers(&graph(&[("meshling-render-gpu", &["meshling-plan"])])).is_empty());
    }

    #[test]
    fn rejects_sideways_upward_and_ui_dependencies() {
        let errors = check_layers(&graph(&[
            ("meshling-rig", &["meshling-ops"]),
            ("meshling-model", &["meshling-engine"]),
            ("meshling-engine", &["egui"]),
            ("meshling-ui-egui", &["egui"]),
            ("meshling-new", &[]),
        ]));
        assert_eq!(errors.len(), 4, "{errors:#?}");
    }
}
