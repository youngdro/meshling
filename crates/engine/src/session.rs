//! An open project with its pixels, undo history, preview parameter values and actor.

use std::path::PathBuf;

use meshling_geom::Size;
use meshling_model::{ObjId, Project, Provenance};
use meshling_ops::{Edit, History, Transaction};
use meshling_plan::{Pixels, Textures};
use meshling_rig::ParamValues;
use serde::Serialize;

use crate::CommandError;

/// The outcome of a change to the project, as commands report it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    /// The undo step's label.
    pub label: String,
    /// Every object added, changed or removed.
    pub changed: Vec<ObjId>,
}

impl Change {
    fn of(tx: &Transaction) -> Self {
        Self { label: tx.label.clone(), changed: tx.touched() }
    }
}

#[derive(Debug)]
pub struct Session {
    project: Project,
    textures: Textures,
    history: History,
    values: ParamValues,
    actor: Provenance,
    dir: Option<PathBuf>,
    revision: u64,
    saved_revision: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Project::new(Size::new(1024, 1024)), Textures::new())
    }
}

impl Session {
    pub fn new(project: Project, textures: Textures) -> Self {
        Self {
            project,
            textures,
            history: History::default(),
            values: ParamValues::new(),
            actor: Provenance::Human,
            dir: None,
            revision: 0,
            saved_revision: 0,
        }
    }

    /// Who this session's edits are attributed to.
    pub fn with_actor(mut self, actor: Provenance) -> Self {
        self.actor = actor;
        self
    }

    pub fn set_actor(&mut self, actor: Provenance) {
        self.actor = actor;
    }

    pub fn actor(&self) -> &Provenance {
        &self.actor
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn textures(&self) -> &Textures {
        &self.textures
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Preview parameter values (not part of the project, not undoable).
    pub fn values(&self) -> &ParamValues {
        &self.values
    }

    pub fn values_mut(&mut self) -> &mut ParamValues {
        &mut self.values
    }

    /// The project directory this session was opened from or last saved to.
    pub fn dir(&self) -> Option<&PathBuf> {
        self.dir.as_ref()
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// Increments on every change; frontends redraw when it moves.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Replace the whole project (new or opened): history and preview values start over.
    pub fn replace(&mut self, project: Project, textures: Textures, dir: Option<PathBuf>) {
        self.project = project;
        self.textures = textures;
        self.history.clear();
        self.values = ParamValues::new();
        self.dir = dir;
        self.revision += 1;
        self.saved_revision = self.revision;
    }

    pub(crate) fn mark_saved(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
        self.saved_revision = self.revision;
    }

    /// Add texture pixels (e.g. from an import) before the edit that references them.
    pub fn add_pixels(&mut self, id: ObjId, pixels: Pixels) {
        self.textures.insert(id, pixels);
    }

    /// Run `f` against an [`Edit`] of the project and commit the result as one undo step.
    /// Nothing changes if `f` fails or the edit would break the project's structure.
    pub fn edit<T>(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce(&mut Edit) -> Result<T, CommandError>,
    ) -> Result<(T, Change), CommandError> {
        let mut edit = Edit::new(&self.project, label, self.actor.clone());
        let out = f(&mut edit)?;
        let (project, tx) = edit.finish()?;
        let change = Change::of(&tx);
        if !tx.is_empty() {
            self.project = project;
            self.history.record(tx);
            self.revision += 1;
        }
        Ok((out, change))
    }

    pub fn undo(&mut self) -> Result<Option<Change>, CommandError> {
        let Some((project, tx)) = self.history.undo(&self.project)? else { return Ok(None) };
        let change = Change::of(tx);
        self.project = project;
        self.revision += 1;
        Ok(Some(change))
    }

    pub fn redo(&mut self) -> Result<Option<Change>, CommandError> {
        let Some((project, tx)) = self.history.redo(&self.project)? else { return Ok(None) };
        let change = Change::of(tx);
        self.project = project;
        self.revision += 1;
        Ok(Some(change))
    }
}
