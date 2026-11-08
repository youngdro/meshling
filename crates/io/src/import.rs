//! Turn a [`PsdDocument`] into project objects: a part per group, a texture and an auto-meshed
//! art mesh per layer, in the document's stacking order.

use std::collections::HashMap;
use std::sync::Arc;

use meshling_geom::Vec2;
use meshling_mesh::{MeshOptions, auto_mesh};
use meshling_model::{ArtMesh, KeyformGrid, MeshKeyform, Meta, ObjId, Part, TextureAsset};
use meshling_plan::Textures;

use crate::image::content_hash;
use crate::psd::PsdDocument;

/// Tag on every object an import creates, so agents can find them.
pub const PSD_TAG: &str = "psd";

#[derive(Clone, Debug, PartialEq)]
pub struct ImportOptions {
    /// Name of the part that holds everything imported (usually the file name).
    pub root_name: String,
    /// Parent of that root part, when importing into an existing part tree.
    pub parent: Option<ObjId>,
    pub mesh: MeshOptions,
    /// Draw order of the bottom layer; each layer above adds `draw_order_step`.
    pub draw_order_start: i32,
    pub draw_order_step: i32,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            root_name: "psd".into(),
            parent: None,
            mesh: MeshOptions::default(),
            draw_order_start: 0,
            draw_order_step: 10,
        }
    }
}

/// Objects to add to a project, plus the texture pixels they use.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    /// Parents come before their children; the first is the root.
    pub parts: Vec<Part>,
    pub textures: Vec<TextureAsset>,
    /// Back to front.
    pub meshes: Vec<ArtMesh>,
    pub pixels: Textures,
    /// Things a person should know: skipped layers, approximated blend modes, layers off canvas.
    pub warnings: Vec<String>,
}

fn meta(name: &str) -> Meta {
    let mut m = Meta::new(name);
    m.tags.push(PSD_TAG.into());
    m
}

pub fn import_psd(doc: &PsdDocument, opts: &ImportOptions) -> Imported {
    let mut out = Imported {
        parts: Vec::new(),
        textures: Vec::new(),
        meshes: Vec::new(),
        pixels: Textures::new(),
        warnings: Vec::new(),
    };
    let root = Part { meta: meta(&opts.root_name), parent: opts.parent, visible: true };
    let root_id = root.meta.id;
    out.parts.push(root);

    // Groups arrive outer before inner, so a parent's part always exists already.
    let mut group_part: HashMap<Vec<String>, ObjId> = HashMap::new();
    for g in &doc.groups {
        let Some((name, outer)) = g.path.split_last() else { continue };
        let parent = group_part.get(outer).copied().unwrap_or(root_id);
        let part = Part { meta: meta(name), parent: Some(parent), visible: g.visible };
        group_part.insert(g.path.clone(), part.meta.id);
        out.parts.push(part);
    }

    for name in &doc.empty_layers {
        out.warnings.push(format!("layer {name:?} has no visible pixels and was skipped"));
    }

    let (cw, ch) = (doc.size.width as i32, doc.size.height as i32);
    for (index, layer) in doc.layers.iter().enumerate() {
        let size = layer.pixels.size;
        let generated = auto_mesh(size, opts.mesh, |x, y| layer.pixels.get(x, y)[3]);
        if generated.indices.is_empty() {
            out.warnings.push(format!("layer {:?} produced no triangles and was skipped", layer.name));
            continue;
        }
        if layer.blend_approximated {
            out.warnings.push(format!(
                "layer {:?}: blend mode {:?} is not supported and was imported as Normal",
                layer.name, layer.blend_key
            ));
        }
        let (right, bottom) = (layer.left + size.width as i32, layer.top + size.height as i32);
        if right <= 0 || bottom <= 0 || layer.left >= cw || layer.top >= ch {
            out.warnings.push(format!("layer {:?} lies entirely outside the canvas", layer.name));
        }

        let mut part = group_part.get(&layer.groups).copied().unwrap_or(root_id);
        if !layer.visible {
            // Meshes have no visibility of their own; a hidden layer gets a hidden part.
            let hidden = Part { meta: meta(&layer.name), parent: Some(part), visible: false };
            part = hidden.meta.id;
            out.parts.push(hidden);
        }

        let texture = TextureAsset { meta: meta(&layer.name), size, content_hash: content_hash(&layer.pixels) };
        let offset = Vec2::new(layer.left as f32, layer.top as f32);
        let positions: Arc<[Vec2]> = generated.translated(offset).into();
        let draw_order = opts.draw_order_start.saturating_add(opts.draw_order_step.saturating_mul(index as i32));
        out.meshes.push(ArtMesh {
            meta: meta(&layer.name),
            part,
            deformer: None,
            texture: texture.meta.id,
            uvs: generated.uvs.into(),
            indices: generated.indices.into(),
            draw_order,
            blend: layer.blend,
            clip_by: Vec::new(),
            keyforms: KeyformGrid::single(MeshKeyform {
                positions,
                opacity: f32::from(layer.opacity) / 255.0,
                draw_order: 0,
            }),
        });
        out.pixels.insert(texture.meta.id, layer.pixels.clone());
        out.textures.push(texture);
    }
    out
}
