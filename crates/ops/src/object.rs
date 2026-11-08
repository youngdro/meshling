//! A project object of any kind, and list access by kind.

use meshling_model::{ArtMesh, Deformer, Meta, ObjId, Param, Part, Project, TextureAsset};
use serde::{Deserialize, Serialize};

use crate::OpError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Texture,
    Part,
    Param,
    Deformer,
    Mesh,
}

/// Any object a project holds, by value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "object", rename_all = "camelCase")]
pub enum Object {
    Texture(TextureAsset),
    Part(Part),
    Param(Param),
    Deformer(Deformer),
    Mesh(ArtMesh),
}

impl Object {
    pub fn kind(&self) -> Kind {
        match self {
            Object::Texture(_) => Kind::Texture,
            Object::Part(_) => Kind::Part,
            Object::Param(_) => Kind::Param,
            Object::Deformer(_) => Kind::Deformer,
            Object::Mesh(_) => Kind::Mesh,
        }
    }

    pub fn meta(&self) -> &Meta {
        match self {
            Object::Texture(o) => &o.meta,
            Object::Part(o) => &o.meta,
            Object::Param(o) => &o.meta,
            Object::Deformer(o) => &o.meta,
            Object::Mesh(o) => &o.meta,
        }
    }

    pub fn meta_mut(&mut self) -> &mut Meta {
        match self {
            Object::Texture(o) => &mut o.meta,
            Object::Part(o) => &mut o.meta,
            Object::Param(o) => &mut o.meta,
            Object::Deformer(o) => &mut o.meta,
            Object::Mesh(o) => &mut o.meta,
        }
    }

    pub fn id(&self) -> ObjId {
        self.meta().id
    }
}

/// The kind and list position of object `id`.
pub(crate) fn locate(project: &Project, id: ObjId) -> Option<(Kind, usize)> {
    let find =
        |kind, mut metas: Box<dyn Iterator<Item = &Meta> + '_>| metas.position(|m| m.id == id).map(|i| (kind, i));
    find(Kind::Texture, Box::new(project.textures.iter().map(|o| &o.meta)))
        .or_else(|| find(Kind::Part, Box::new(project.parts.iter().map(|o| &o.meta))))
        .or_else(|| find(Kind::Param, Box::new(project.params.iter().map(|o| &o.meta))))
        .or_else(|| find(Kind::Deformer, Box::new(project.deformers.iter().map(|o| &o.meta))))
        .or_else(|| find(Kind::Mesh, Box::new(project.meshes.iter().map(|o| &o.meta))))
}

pub(crate) fn len(project: &Project, kind: Kind) -> usize {
    match kind {
        Kind::Texture => project.textures.len(),
        Kind::Part => project.parts.len(),
        Kind::Param => project.params.len(),
        Kind::Deformer => project.deformers.len(),
        Kind::Mesh => project.meshes.len(),
    }
}

pub(crate) fn get(project: &Project, kind: Kind, at: usize) -> Option<Object> {
    match kind {
        Kind::Texture => project.textures.get(at).cloned().map(Object::Texture),
        Kind::Part => project.parts.get(at).cloned().map(Object::Part),
        Kind::Param => project.params.get(at).cloned().map(Object::Param),
        Kind::Deformer => project.deformers.get(at).cloned().map(Object::Deformer),
        Kind::Mesh => project.meshes.get(at).cloned().map(Object::Mesh),
    }
}

pub(crate) fn insert(project: &mut Project, at: usize, object: Object) -> Result<(), OpError> {
    fn put<T>(list: &mut Vec<T>, at: usize, o: T, kind: Kind) -> Result<(), OpError> {
        if at > list.len() {
            return Err(OpError::OutOfRange { kind, at });
        }
        list.insert(at, o);
        Ok(())
    }
    let kind = object.kind();
    match object {
        Object::Texture(o) => put(&mut project.textures, at, o, kind),
        Object::Part(o) => put(&mut project.parts, at, o, kind),
        Object::Param(o) => put(&mut project.params, at, o, kind),
        Object::Deformer(o) => put(&mut project.deformers, at, o, kind),
        Object::Mesh(o) => put(&mut project.meshes, at, o, kind),
    }
}

pub(crate) fn set(project: &mut Project, at: usize, object: Object) -> Result<(), OpError> {
    fn put<T>(list: &mut [T], at: usize, o: T, kind: Kind) -> Result<(), OpError> {
        *list.get_mut(at).ok_or(OpError::OutOfRange { kind, at })? = o;
        Ok(())
    }
    let kind = object.kind();
    match object {
        Object::Texture(o) => put(&mut project.textures, at, o, kind),
        Object::Part(o) => put(&mut project.parts, at, o, kind),
        Object::Param(o) => put(&mut project.params, at, o, kind),
        Object::Deformer(o) => put(&mut project.deformers, at, o, kind),
        Object::Mesh(o) => put(&mut project.meshes, at, o, kind),
    }
}

pub(crate) fn remove(project: &mut Project, kind: Kind, at: usize) -> Result<Object, OpError> {
    fn take<T>(list: &mut Vec<T>, at: usize, kind: Kind) -> Result<T, OpError> {
        if at >= list.len() {
            return Err(OpError::OutOfRange { kind, at });
        }
        Ok(list.remove(at))
    }
    Ok(match kind {
        Kind::Texture => Object::Texture(take(&mut project.textures, at, kind)?),
        Kind::Part => Object::Part(take(&mut project.parts, at, kind)?),
        Kind::Param => Object::Param(take(&mut project.params, at, kind)?),
        Kind::Deformer => Object::Deformer(take(&mut project.deformers, at, kind)?),
        Kind::Mesh => Object::Mesh(take(&mut project.meshes, at, kind)?),
    })
}
