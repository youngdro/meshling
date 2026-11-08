//! Programmatic checks an editor or agent runs after editing, beyond the structural invariants
//! `meshling-model` already enforces:
//!
//! - **structure**: every [`StructureError`](meshling_model::StructureError), as errors;
//! - **textures**: pixels missing or of the wrong size;
//! - **meshes**: no triangles, texture coordinates outside `[0, 1]`;
//! - **parameters**: keys outside the parameter's range, parameters nothing is bound to;
//! - **pose sweep**: each parameter at its minimum and maximum (the others at their defaults)
//!   and the rest pose are evaluated, and triangles that flip (fold over) or collapse are
//!   reported once per mesh and pose.
//!
//! Issues come back in a stable order so agents can diff reports between edits.

use std::collections::HashSet;

use meshling_geom::Vec2;
use meshling_model::{DeformerKind, ObjId, Project};
use meshling_plan::Textures;
use meshling_rig::{ParamValues, evaluate};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
}

/// One parameter value of the pose an issue was found at.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamAt {
    pub param: ObjId,
    pub name: String,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub severity: Severity,
    /// Stable machine-readable kind, e.g. `flipped-triangles`.
    pub code: &'static str,
    pub object: Option<ObjId>,
    pub message: String,
    /// The parameter values that differ from their defaults when the issue appears; empty for
    /// the rest pose or issues that don't depend on the pose.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pose: Vec<ParamAt>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Error).count()
    }

    pub fn warnings(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Warning).count()
    }

    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Posed triangles smaller than this many square pixels count as collapsed.
    pub min_area: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { min_area: 0.01 }
    }
}

/// Check `project`. Texture checks run only when `textures` is given.
pub fn validate(project: &Project, textures: Option<&Textures>, opts: &Options) -> Report {
    let mut issues = Vec::new();
    let mut push = |severity, code, object, message: String| {
        issues.push(Issue { severity, code, object, message, pose: Vec::new() })
    };

    let structure = project.check();
    for e in &structure {
        push(Severity::Error, "structure", None, e.to_string());
    }

    if let Some(textures) = textures {
        for t in &project.textures {
            match textures.get(&t.meta.id) {
                None => push(
                    Severity::Error,
                    "missing-pixels",
                    Some(t.meta.id),
                    format!("texture {:?} has no pixels", t.meta.name),
                ),
                Some(p) if p.size != t.size => push(
                    Severity::Error,
                    "texture-size",
                    Some(t.meta.id),
                    format!(
                        "texture {:?} is {}×{} but its pixels are {}×{}",
                        t.meta.name, t.size.width, t.size.height, p.size.width, p.size.height
                    ),
                ),
                Some(_) => {}
            }
        }
    }

    for m in &project.meshes {
        if m.indices.len() < 3 {
            push(Severity::Warning, "empty-mesh", Some(m.meta.id), format!("mesh {:?} has no triangles", m.meta.name));
        }
        let outside = m.uvs.iter().filter(|uv| !(0.0..=1.0).contains(&uv.x) || !(0.0..=1.0).contains(&uv.y)).count();
        if outside > 0 {
            push(
                Severity::Warning,
                "uv-out-of-range",
                Some(m.meta.id),
                format!(
                    "mesh {:?}: {outside} texture coordinates fall outside [0, 1] and repeat the texture's edge",
                    m.meta.name
                ),
            );
        }
    }

    let mut bound: HashSet<ObjId> = HashSet::new();
    let mut bindings = |owner: ObjId, name: &str, list: &[meshling_model::ParamBinding]| {
        for b in list {
            bound.insert(b.param);
            if let Some(p) = project.param(b.param) {
                let outside = b.keys.iter().filter(|&&k| k < p.min || k > p.max).count();
                if outside > 0 {
                    push(
                        Severity::Warning,
                        "key-out-of-range",
                        Some(owner),
                        format!(
                            "{name:?}: {outside} keys on {:?} lie outside its range {}..{} and can never be reached",
                            p.meta.name, p.min, p.max
                        ),
                    );
                }
            }
        }
    };
    for d in &project.deformers {
        match &d.kind {
            DeformerKind::Warp { keyforms, .. } => bindings(d.meta.id, &d.meta.name, &keyforms.bindings),
            DeformerKind::Rotation { keyforms } => bindings(d.meta.id, &d.meta.name, &keyforms.bindings),
        }
    }
    for m in &project.meshes {
        bindings(m.meta.id, &m.meta.name, &m.keyforms.bindings);
    }
    for p in &project.params {
        if !bound.contains(&p.meta.id) {
            push(
                Severity::Warning,
                "unused-param",
                Some(p.meta.id),
                format!("parameter {:?} moves nothing", p.meta.name),
            );
        }
    }

    // Posing needs a well-formed project.
    if structure.is_empty() {
        sweep(project, opts, &mut issues);
    }

    issues.sort_by(|a, b| (a.severity, a.code, a.object).cmp(&(b.severity, b.code, b.object)));
    Report { issues }
}

/// Evaluate the rest pose and each parameter at its extremes; report flipped and collapsed
/// triangles once per mesh and pose.
fn sweep(project: &Project, opts: &Options, issues: &mut Vec<Issue>) {
    let mut poses: Vec<Vec<ParamAt>> = vec![Vec::new()];
    for p in &project.params {
        for value in [p.min, p.max] {
            if value != p.default {
                poses.push(vec![ParamAt { param: p.meta.id, name: p.meta.name.clone(), value }]);
            }
        }
    }
    for at in poses {
        let values = at.iter().fold(ParamValues::new(), |v, a| v.with(a.param, a.value));
        let where_ = if at.is_empty() {
            "at rest".to_string()
        } else {
            at.iter().map(|a| format!("{} = {}", a.name, a.value)).collect::<Vec<_>>().join(", ")
        };
        let pose = match evaluate(project, &values) {
            Ok(pose) => pose,
            Err(e) => {
                issues.push(Issue {
                    severity: Severity::Error,
                    code: "rig",
                    object: None,
                    message: format!("{where_}: {e}"),
                    pose: at,
                });
                continue;
            }
        };
        for m in &project.meshes {
            let Some(posed) = pose.mesh(m.meta.id) else { continue };
            let (mut flipped, mut collapsed) = (0, 0);
            for t in m.indices.as_chunks::<3>().0 {
                let idx = t.map(|i| i as usize);
                let (Some(&a), Some(&b), Some(&c)) =
                    (posed.positions.get(idx[0]), posed.positions.get(idx[1]), posed.positions.get(idx[2]))
                else {
                    continue;
                };
                let (Some(&ua), Some(&ub), Some(&uc)) = (m.uvs.get(idx[0]), m.uvs.get(idx[1]), m.uvs.get(idx[2]))
                else {
                    continue;
                };
                let reference = cross(ua, ub, uc);
                if reference == 0.0 {
                    continue; // degenerate in texture space: nothing to compare against
                }
                let area = cross(a, b, c);
                if area.abs() / 2.0 < opts.min_area {
                    collapsed += 1;
                } else if area.signum() != reference.signum() {
                    flipped += 1;
                }
            }
            let id = Some(m.meta.id);
            if flipped > 0 {
                issues.push(Issue {
                    severity: Severity::Error,
                    code: "flipped-triangles",
                    object: id,
                    message: format!("mesh {:?}: {flipped} triangles fold over {where_}", m.meta.name),
                    pose: at.clone(),
                });
            }
            if collapsed > 0 {
                issues.push(Issue {
                    severity: Severity::Warning,
                    code: "collapsed-triangles",
                    object: id,
                    message: format!(
                        "mesh {:?}: {collapsed} triangles collapse to (almost) nothing {where_}",
                        m.meta.name
                    ),
                    pose: at.clone(),
                });
            }
        }
    }
}

/// Twice the signed area of `(a, b, c)`.
fn cross(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - a)
}
