//! The façade every frontend talks to. The editor UI, the CLI and the MCP server are thin shells
//! over it: they hold a [`Session`] and run commands from the [`Registry`] by name with JSON
//! arguments, so a person clicking and an agent calling a tool go through the same code.
//!
//! Every command that changes the project builds one [`Transaction`](meshling_ops::Transaction)
//! (one undo step), stamped with the session's actor as provenance. Commands never key objects
//! by name or position: they take and return [`ObjId`]s.

mod commands;
mod registry;
mod session;

pub use registry::{CommandSpec, Registry};
pub use session::{Change, Session};

use meshling_model::ObjId;

/// Why a command failed. Frontends show the message; agents can branch on the variant.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command {0:?}")]
    UnknownCommand(String),
    #[error("invalid arguments: {0}")]
    BadArgs(String),
    #[error("no object with id {0}")]
    NotFound(ObjId),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Op(#[from] meshling_ops::OpError),
    #[error("{0}")]
    Rig(#[from] meshling_rig::RigError),
    #[error("{0}")]
    Io(String),
}

impl CommandError {
    /// A short stable code for agents.
    pub fn code(&self) -> &'static str {
        match self {
            CommandError::UnknownCommand(_) => "unknown-command",
            CommandError::BadArgs(_) => "bad-args",
            CommandError::NotFound(_) => "not-found",
            CommandError::Invalid(_) => "invalid",
            CommandError::Op(_) => "edit-rejected",
            CommandError::Rig(_) => "rig",
            CommandError::Io(_) => "io",
        }
    }
}

pub type CommandResult = Result<serde_json::Value, CommandError>;
