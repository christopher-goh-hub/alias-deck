//! alias-deck core: one pack format, one typed model, four shells.

mod document;
mod model;
mod secrets;

pub use document::{PackDocument, ParseError, ParseErrorKind};
pub use model::{
    Alias, Argv, CommandName, Dialect, Entry, EntryName, EntryRef, EnvEntry, EnvName, EnvValue, Function, FunctionBody,
    Group, InShell, InvalidValue, Os, OsSet, Pack, PackMeta, Slug, Text, Word,
};
pub use secrets::{Bindings, SecretSource};
