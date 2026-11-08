//! The on-disk project format: a directory holding `project.json` and the texture pixels as
//! `textures/<content hash>.png` (straight alpha, so any image tool can open them). Textures are
//! content addressed: identical pixels are stored once and never rewritten.

use std::path::{Path, PathBuf};

use meshling_model::{LoadError, ObjId, Project};
use meshling_plan::Textures;

use crate::image::{ImageError, content_hash, decode_png, encode_png};

pub const PROJECT_FILE: &str = "project.json";
pub const TEXTURE_DIR: &str = "textures";

#[derive(Debug, thiserror::Error)]
pub enum ProjectDirError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{0}")]
    Project(#[from] LoadError),
    #[error("{path}: {source}")]
    Image { path: PathBuf, source: ImageError },
    #[error("texture {0} has no pixels")]
    MissingPixels(ObjId),
    #[error("texture {texture}: pixels do not match its size or content hash")]
    Mismatch { texture: ObjId },
    #[error("texture {texture}: content hash {hash:?} is not a 64-digit hex string")]
    BadHash { texture: ObjId, hash: String },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ProjectDirError + '_ {
    move |source| ProjectDirError::Io { path: path.to_path_buf(), source }
}

fn texture_path(dir: &Path, texture: ObjId, hash: &str) -> Result<PathBuf, ProjectDirError> {
    // The hash names a file, so it must not be able to name anything else.
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(ProjectDirError::BadHash { texture, hash: hash.to_string() });
    }
    Ok(dir.join(TEXTURE_DIR).join(format!("{hash}.png")))
}

/// Write `path` via a temporary file and a rename, so a crash never leaves it half written.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ProjectDirError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

/// Save `project` and the pixels of each of its textures into `dir` (created if missing).
pub fn save_project(dir: &Path, project: &Project, textures: &Textures) -> Result<(), ProjectDirError> {
    let tex_dir = dir.join(TEXTURE_DIR);
    std::fs::create_dir_all(&tex_dir).map_err(io(&tex_dir))?;
    for asset in &project.textures {
        let id = asset.meta.id;
        let pixels = textures.get(&id).ok_or(ProjectDirError::MissingPixels(id))?;
        if pixels.size != asset.size || content_hash(pixels) != asset.content_hash {
            return Err(ProjectDirError::Mismatch { texture: id });
        }
        let path = texture_path(dir, id, &asset.content_hash)?;
        if !path.exists() {
            let png = encode_png(pixels).map_err(|source| ProjectDirError::Image { path: path.clone(), source })?;
            write_atomic(&path, &png)?;
        }
    }
    write_atomic(&dir.join(PROJECT_FILE), project.to_json().as_bytes())
}

/// Load a project directory written by [`save_project`], checking every texture's hash.
pub fn load_project(dir: &Path) -> Result<(Project, Textures), ProjectDirError> {
    let file = dir.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&file).map_err(io(&file))?;
    let project = Project::from_json(&text)?;
    let mut textures = Textures::new();
    for asset in &project.textures {
        let id = asset.meta.id;
        let path = texture_path(dir, id, &asset.content_hash)?;
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        let pixels = decode_png(&bytes).map_err(|source| ProjectDirError::Image { path: path.clone(), source })?;
        if pixels.size != asset.size || content_hash(&pixels) != asset.content_hash {
            return Err(ProjectDirError::Mismatch { texture: id });
        }
        textures.insert(id, pixels);
    }
    Ok((project, textures))
}
