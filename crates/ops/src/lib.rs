//! Reversible operations, transactions and history over immutable project snapshots.
//!
//! Every edit is a [`Transaction`]: a list of object-level [`Op`]s (insert, remove, replace,
//! resize the canvas). Each op carries the full before and after state of what it touches, so
//! its inverse is exact and undo/redo never re-runs editing logic. Applying a transaction
//! yields a new [`Project`]; the old one is untouched (clones share every large buffer).
//!
//! Commands build transactions with an [`Edit`], which applies each change to a working copy as
//! it goes, stamps the actor's [`Provenance`] on every object it touches, and refuses to finish
//! if the result has structural errors the starting project did not.

mod object;

use meshling_geom::Size;
use meshling_model::{ObjId, Project, Provenance, StructureError};
use serde::{Deserialize, Serialize};

pub use object::{Kind, Object};

/// One reversible change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Put `object` at position `at` of its kind's list.
    Insert {
        at: usize,
        object: Object,
    },
    /// Take `object` out of position `at` of its kind's list.
    Remove {
        at: usize,
        object: Object,
    },
    /// Swap an object for a new state with the same id and kind.
    Replace {
        before: Object,
        after: Object,
    },
    SetCanvas {
        before: Size,
        after: Size,
    },
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum OpError {
    #[error("no object with id {0}")]
    NotFound(ObjId),
    #[error("an object with id {0} already exists")]
    DuplicateId(ObjId),
    #[error("position {at} is out of range for {kind:?}")]
    OutOfRange { kind: Kind, at: usize },
    #[error("object {0} changed since this operation was recorded")]
    Stale(ObjId),
    #[error("canvas changed since this operation was recorded")]
    StaleCanvas,
    #[error("a replacement must keep the object's id and kind ({0})")]
    IdentityChanged(ObjId),
    #[error("the edit would break the project's structure: {}", list(.0))]
    Structure(Vec<StructureError>),
}

fn list(errors: &[StructureError]) -> String {
    errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}

impl Op {
    pub fn inverse(&self) -> Op {
        match self {
            Op::Insert { at, object } => Op::Remove { at: *at, object: object.clone() },
            Op::Remove { at, object } => Op::Insert { at: *at, object: object.clone() },
            Op::Replace { before, after } => Op::Replace { before: after.clone(), after: before.clone() },
            Op::SetCanvas { before, after } => Op::SetCanvas { before: *after, after: *before },
        }
    }

    /// Apply in place. On error `project` is unchanged.
    pub fn apply(&self, project: &mut Project) -> Result<(), OpError> {
        match self {
            Op::Insert { at, object } => {
                if object::locate(project, object.id()).is_some() {
                    return Err(OpError::DuplicateId(object.id()));
                }
                object::insert(project, *at, object.clone())
            }
            Op::Remove { at, object } => {
                match object::get(project, object.kind(), *at) {
                    Some(current) if current == *object => {}
                    Some(_) | None => return Err(OpError::Stale(object.id())),
                }
                object::remove(project, object.kind(), *at).map(|_| ())
            }
            Op::Replace { before, after } => {
                if before.id() != after.id() || before.kind() != after.kind() {
                    return Err(OpError::IdentityChanged(before.id()));
                }
                let (kind, at) = object::locate(project, before.id()).ok_or(OpError::NotFound(before.id()))?;
                if object::get(project, kind, at).as_ref() != Some(before) {
                    return Err(OpError::Stale(before.id()));
                }
                object::set(project, at, after.clone())
            }
            Op::SetCanvas { before, after } => {
                if project.canvas != *before {
                    return Err(OpError::StaleCanvas);
                }
                project.canvas = *after;
                Ok(())
            }
        }
    }
}

/// A named group of ops that is applied, undone and redone as one step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub label: String,
    pub ops: Vec<Op>,
}

impl Transaction {
    /// The project after this transaction, or the first op's error (nothing applied).
    pub fn apply(&self, project: &Project) -> Result<Project, OpError> {
        let mut next = project.clone();
        for op in &self.ops {
            op.apply(&mut next)?;
        }
        Ok(next)
    }

    pub fn inverse(&self) -> Transaction {
        Transaction { label: self.label.clone(), ops: self.ops.iter().rev().map(Op::inverse).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Ids of every object the transaction touches, in first-touched order.
    pub fn touched(&self) -> Vec<ObjId> {
        let mut out = Vec::new();
        for op in &self.ops {
            let id = match op {
                Op::Insert { object, .. } | Op::Remove { object, .. } => Some(object.id()),
                Op::Replace { after, .. } => Some(after.id()),
                Op::SetCanvas { .. } => None,
            };
            if let Some(id) = id
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out
    }
}

/// Builds a transaction against a working copy of a project.
#[derive(Clone, Debug)]
pub struct Edit {
    base_errors: Vec<StructureError>,
    working: Project,
    ops: Vec<Op>,
    label: String,
    provenance: Provenance,
}

impl Edit {
    pub fn new(project: &Project, label: impl Into<String>, provenance: Provenance) -> Self {
        Self {
            base_errors: project.check(),
            working: project.clone(),
            ops: Vec::new(),
            label: label.into(),
            provenance,
        }
    }

    /// The project with every change so far.
    pub fn project(&self) -> &Project {
        &self.working
    }

    fn record(&mut self, op: Op) -> Result<(), OpError> {
        op.apply(&mut self.working)?;
        self.ops.push(op);
        Ok(())
    }

    /// Append `object` to its kind's list (stamped with this edit's provenance). Returns its id.
    pub fn insert(&mut self, object: Object) -> Result<ObjId, OpError> {
        let at = object::len(&self.working, object.kind());
        self.insert_at(at, object)
    }

    /// Insert `object` at position `at` of its kind's list.
    pub fn insert_at(&mut self, at: usize, mut object: Object) -> Result<ObjId, OpError> {
        object.meta_mut().provenance = self.provenance.clone();
        let id = object.id();
        self.record(Op::Insert { at, object })?;
        Ok(id)
    }

    /// Change the object `id` with `f`. Records nothing when `f` changes nothing.
    pub fn update(&mut self, id: ObjId, f: impl FnOnce(&mut Object)) -> Result<(), OpError> {
        let (kind, at) = object::locate(&self.working, id).ok_or(OpError::NotFound(id))?;
        let before = object::get(&self.working, kind, at).ok_or(OpError::NotFound(id))?;
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return Ok(());
        }
        after.meta_mut().provenance = self.provenance.clone();
        self.record(Op::Replace { before, after })
    }

    /// Remove the object `id` and return it.
    pub fn remove(&mut self, id: ObjId) -> Result<Object, OpError> {
        let (kind, at) = object::locate(&self.working, id).ok_or(OpError::NotFound(id))?;
        let object = object::get(&self.working, kind, at).ok_or(OpError::NotFound(id))?;
        self.record(Op::Remove { at, object: object.clone() })?;
        Ok(object)
    }

    /// Move the object `id` to position `to` of its kind's list (after removing it).
    pub fn reorder(&mut self, id: ObjId, to: usize) -> Result<(), OpError> {
        let (kind, at) = object::locate(&self.working, id).ok_or(OpError::NotFound(id))?;
        if at == to {
            return Ok(());
        }
        if to >= object::len(&self.working, kind) {
            return Err(OpError::OutOfRange { kind, at: to });
        }
        let object = self.remove(id)?;
        self.record(Op::Insert { at: to, object })
    }

    pub fn set_canvas(&mut self, size: Size) -> Result<(), OpError> {
        let before = self.working.canvas;
        if before != size {
            self.record(Op::SetCanvas { before, after: size })?;
        }
        Ok(())
    }

    /// Finish: the new project and the transaction that produced it. Fails if the edit leaves
    /// structural errors the starting project did not have.
    pub fn finish(self) -> Result<(Project, Transaction), OpError> {
        let new: Vec<_> = self.working.check().into_iter().filter(|e| !self.base_errors.contains(e)).collect();
        if !new.is_empty() {
            return Err(OpError::Structure(new));
        }
        Ok((self.working, Transaction { label: self.label, ops: self.ops }))
    }
}

/// Undo and redo stacks.
#[derive(Clone, Debug)]
pub struct History {
    done: Vec<Transaction>,
    undone: Vec<Transaction>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(500)
    }
}

impl History {
    /// Keep at most `limit` undo steps.
    pub fn new(limit: usize) -> Self {
        Self { done: Vec::new(), undone: Vec::new(), limit: limit.max(1) }
    }

    /// Record a transaction that has just been applied. Clears the redo stack; empty
    /// transactions are not recorded.
    pub fn record(&mut self, tx: Transaction) {
        if tx.is_empty() {
            return;
        }
        self.undone.clear();
        self.done.push(tx);
        if self.done.len() > self.limit {
            let excess = self.done.len() - self.limit;
            self.done.drain(..excess);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Labels of the steps undo would revert, most recent first.
    pub fn undo_labels(&self) -> impl Iterator<Item = &str> {
        self.done.iter().rev().map(|t| t.label.as_str())
    }

    /// Labels of the steps redo would re-apply, next first.
    pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
        self.undone.iter().rev().map(|t| t.label.as_str())
    }

    /// Revert the last step on `project`. `Ok(None)` when there is nothing to undo; on error the
    /// history is unchanged.
    pub fn undo(&mut self, project: &Project) -> Result<Option<(Project, &Transaction)>, OpError> {
        let Some(tx) = self.done.pop() else { return Ok(None) };
        match tx.inverse().apply(project) {
            Ok(next) => {
                self.undone.push(tx);
                Ok(self.undone.last().map(|t| (next, t)))
            }
            Err(e) => {
                self.done.push(tx);
                Err(e)
            }
        }
    }

    /// Re-apply the last undone step on `project`.
    pub fn redo(&mut self, project: &Project) -> Result<Option<(Project, &Transaction)>, OpError> {
        let Some(tx) = self.undone.pop() else { return Ok(None) };
        match tx.apply(project) {
            Ok(next) => {
                self.done.push(tx);
                Ok(self.done.last().map(|t| (next, t)))
            }
            Err(e) => {
                self.undone.push(tx);
                Err(e)
            }
        }
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }
}

#[cfg(test)]
mod tests;
