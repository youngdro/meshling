//! Commands by name, with JSON Schemas for their arguments.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{CommandError, CommandResult, Session};

/// What a frontend needs to offer a command: a menu entry, a CLI subcommand, an MCP tool.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    /// Dotted name, e.g. `part.create`.
    pub name: &'static str,
    pub summary: &'static str,
    /// Whether the command changes the project (and so adds an undo step).
    pub mutates: bool,
    /// JSON Schema of the argument object.
    pub input_schema: Value,
}

type Handler = Box<dyn Fn(&mut Session, Value) -> CommandResult + Send + Sync>;

pub struct Registry {
    commands: BTreeMap<&'static str, (CommandSpec, Handler)>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.commands.keys()).finish()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::standard()
    }
}

impl Registry {
    pub fn empty() -> Self {
        Self { commands: BTreeMap::new() }
    }

    /// Every built-in command.
    pub fn standard() -> Self {
        let mut r = Self::empty();
        crate::commands::register(&mut r);
        r
    }

    /// Add a command taking arguments `A` (deserialized from JSON; unknown fields rejected by
    /// the argument type).
    pub fn add<A, F>(&mut self, name: &'static str, summary: &'static str, mutates: bool, run: F)
    where
        A: DeserializeOwned + JsonSchema,
        F: Fn(&mut Session, A) -> CommandResult + Send + Sync + 'static,
    {
        let input_schema = serde_json::to_value(schemars::schema_for!(A)).unwrap_or(Value::Null);
        let handler: Handler = Box::new(move |session, args| {
            let args = if args.is_null() { Value::Object(Default::default()) } else { args };
            let args: A = serde_json::from_value(args).map_err(|e| CommandError::BadArgs(e.to_string()))?;
            run(session, args)
        });
        self.commands.insert(name, (CommandSpec { name, summary, mutates, input_schema }, handler));
    }

    /// Specs in name order.
    pub fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.commands.values().map(|(spec, _)| spec)
    }

    pub fn spec(&self, name: &str) -> Option<&CommandSpec> {
        self.commands.get(name).map(|(spec, _)| spec)
    }

    /// Run command `name` with JSON `args` (an object, or null for none).
    pub fn run(&self, session: &mut Session, name: &str, args: Value) -> CommandResult {
        let (_, handler) = self.commands.get(name).ok_or_else(|| CommandError::UnknownCommand(name.to_string()))?;
        handler(session, args)
    }
}
