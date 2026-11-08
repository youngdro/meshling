//! The project data model: pure data plus serde, no behaviour beyond lookups and structural checks.
//!
//! A [`Project`] is an immutable tree of objects. Every object carries a [`Meta`] with a stable
//! [`ObjId`], a human/agent-readable name, semantic tags and a [`Provenance`] that records who last
//! changed it. Large payloads (vertex and index buffers) are `Arc`-shared, so cloning a project
//! for a snapshot costs O(objects), never O(vertices).
//!
//! Texture pixels are not part of the model: a [`TextureAsset`] names its pixels by content hash,
//! and `meshling-io` stores them next to `project.json`.

mod check;
mod id;
mod keyform;

use std::sync::Arc;

use meshling_geom::{Size, Vec2};
use serde::{Deserialize, Serialize};

pub use check::StructureError;
pub use id::{ObjId, ParseIdError};
pub use keyform::{KeyformGrid, ParamBinding};

/// The project file format this build writes. Older versions are migrated on load.
pub const FORMAT_VERSION: u32 = 1;

/// Who last changed an object. Commands use it to decide whether overwriting needs confirmation.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "camelCase")]
pub enum Provenance {
    #[default]
    Human,
    Agent {
        session: String,
    },
    Model {
        name: String,
    },
}

/// Identity and labels shared by every object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: ObjId,
    /// Display and semantic name, e.g. `eye_L` or `AngleX`. Not required to be unique.
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub provenance: Provenance,
}

impl Meta {
    /// A new object made by a person, with a fresh id.
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: ObjId::new(), name: name.into(), tags: Vec::new(), provenance: Provenance::Human }
    }
}

/// A source image (usually one PSD layer). Pixels live outside the model, keyed by `content_hash`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextureAsset {
    pub meta: Meta,
    pub size: Size,
    /// Hex BLAKE3 hash of the premultiplied RGBA8 pixels.
    pub content_hash: String,
}

/// A node of the part tree (head, eyes, hair …): the grouping people and agents navigate by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub meta: Meta,
    pub parent: Option<ObjId>,
    #[serde(default = "yes")]
    pub visible: bool,
}

/// A user-facing control: a slider in the editor, a channel for motions, physics and tracking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub meta: Meta,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

impl Param {
    pub fn clamp(&self, v: f32) -> f32 {
        v.clamp(self.min, self.max)
    }
}

/// How a mesh composites over what is below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Blend {
    #[default]
    Normal,
    Add,
    Multiply,
}

/// A mesh's state at one point of its keyform grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshKeyform {
    /// Vertex positions in the parent deformer's space (canvas space without a parent).
    pub positions: Arc<[Vec2]>,
    pub opacity: f32,
    /// Added to the mesh's base draw order; lets a keyform move a part in front or behind.
    #[serde(default)]
    pub draw_order: i32,
}

/// A textured, deformable triangle mesh: what actually gets drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtMesh {
    pub meta: Meta,
    pub part: ObjId,
    /// The deformer this mesh's positions are expressed in, if any.
    pub deformer: Option<ObjId>,
    pub texture: ObjId,
    /// Texture coordinates in `[0, 1]`, one per vertex.
    pub uvs: Arc<[Vec2]>,
    /// Triangle list into `uvs` and every keyform's `positions`.
    pub indices: Arc<[u32]>,
    pub draw_order: i32,
    #[serde(default)]
    pub blend: Blend,
    /// Meshes whose union masks this one (Live2D-style clipping).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clip_by: Vec<ObjId>,
    pub keyforms: KeyformGrid<MeshKeyform>,
}

impl ArtMesh {
    pub fn vertex_count(&self) -> usize {
        self.uvs.len()
    }
}

/// A warp deformer's state at one keyform: its lattice control points, row major.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarpKeyform {
    /// `(rows + 1) * (cols + 1)` points in the parent deformer's space.
    pub points: Arc<[Vec2]>,
    pub opacity: f32,
}

/// A rotation deformer's state at one keyform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationKeyform {
    pub origin: Vec2,
    pub angle_deg: f32,
    pub scale: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DeformerKind {
    /// A Bezier-patch lattice that bends everything inside it.
    Warp { rows: u32, cols: u32, keyforms: KeyformGrid<WarpKeyform> },
    /// Rotates and scales its children about an origin.
    Rotation { keyforms: KeyformGrid<RotationKeyform> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deformer {
    pub meta: Meta,
    pub parent: Option<ObjId>,
    pub kind: DeformerKind,
}

/// A whole character or scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub format_version: u32,
    pub canvas: Size,
    #[serde(default)]
    pub textures: Vec<TextureAsset>,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub deformers: Vec<Deformer>,
    #[serde(default)]
    pub meshes: Vec<ArtMesh>,
}

/// Errors reading a project file.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("invalid project JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("project format version {found} is newer than this build supports ({supported})")]
    TooNew { found: u32, supported: u32 },
    #[error("project has no formatVersion")]
    MissingVersion,
}

impl Project {
    pub fn new(canvas: Size) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            canvas,
            textures: Vec::new(),
            parts: Vec::new(),
            params: Vec::new(),
            deformers: Vec::new(),
            meshes: Vec::new(),
        }
    }

    /// Serialize to pretty JSON. Output is deterministic: the same project gives the same bytes.
    pub fn to_json(&self) -> String {
        // Serializing plain data into a String cannot fail; fall back to an empty object rather
        // than panic if a future field type ever makes it fallible.
        serde_json::to_string_pretty(self).unwrap_or_else(|_| String::from("{}"))
    }

    /// Parse a project, migrating older format versions first.
    pub fn from_json(text: &str) -> Result<Self, LoadError> {
        let mut value: serde_json::Value = serde_json::from_str(text)?;
        let found = value.get("formatVersion").and_then(serde_json::Value::as_u64).ok_or(LoadError::MissingVersion)?;
        let found =
            u32::try_from(found).map_err(|_| LoadError::TooNew { found: u32::MAX, supported: FORMAT_VERSION })?;
        if found > FORMAT_VERSION {
            return Err(LoadError::TooNew { found, supported: FORMAT_VERSION });
        }
        migrate(&mut value, found);
        Ok(serde_json::from_value(value)?)
    }

    pub fn param(&self, id: ObjId) -> Option<&Param> {
        self.params.iter().find(|p| p.meta.id == id)
    }

    pub fn param_named(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.meta.name == name)
    }

    pub fn deformer(&self, id: ObjId) -> Option<&Deformer> {
        self.deformers.iter().find(|d| d.meta.id == id)
    }

    pub fn mesh(&self, id: ObjId) -> Option<&ArtMesh> {
        self.meshes.iter().find(|m| m.meta.id == id)
    }

    pub fn part(&self, id: ObjId) -> Option<&Part> {
        self.parts.iter().find(|p| p.meta.id == id)
    }

    /// Every object's metadata, in a fixed order (textures, parts, params, deformers, meshes).
    pub fn metas(&self) -> impl Iterator<Item = &Meta> {
        self.textures
            .iter()
            .map(|t| &t.meta)
            .chain(self.parts.iter().map(|p| &p.meta))
            .chain(self.params.iter().map(|p| &p.meta))
            .chain(self.deformers.iter().map(|d| &d.meta))
            .chain(self.meshes.iter().map(|m| &m.meta))
    }
}

/// Bring a parsed document from `from` up to [`FORMAT_VERSION`], one version at a time.
fn migrate(value: &mut serde_json::Value, from: u32) {
    // Version 1 is the first format; each later version adds an arm `1 => { …; v = 2 }`.
    let v = from;
    debug_assert!(v <= FORMAT_VERSION);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("formatVersion".into(), FORMAT_VERSION.into());
    }
}

fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests;
