//! alias-deck core: one pack format, one typed model, four shells.
//!
//! ```
//! use alias_deck_core::{compile, Bindings, CompileOptions, Deck, Os, PackDocument, Shell, Target};
//!
//! let src = "format = 1\n[pack]\nname = \"demo\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\n\
//!            description = \"demo\"\n[groups.git.aliases]\ngs = \"git status -sb\"\n";
//! let doc = PackDocument::parse(src).expect("valid pack");
//! let deck = Deck::new(vec![doc.pack().clone()]);
//! let target = Target { shell: Shell::Fish, os: Os::Linux };
//! let out = compile(&deck, &Bindings::default(), target, &CompileOptions::default());
//! assert!(out.source.contains("function gs --wraps 'git status -sb'"));
//! assert!(out.entries.iter().all(|r| !r.outcome.is_omitted()));
//! ```
//!
//! The writer and the backends are private, so no caller can emit shell text except through
//! `compile`, and each shell's quoting rules have exactly one home.

mod compile;
mod diff;
mod document;
mod lint;
mod lock;
mod manifest;
mod model;
mod secrets;
mod shells;
mod source;
mod writer;

pub use compile::{
    compile, CompileOptions, Compiled, Deck, EntryReport, FishAliasStyle, Note, OmitReason, Outcome, Shell, Target,
};
pub use diff::{diff_packs, EntryChange, Located, PackDiff};
pub use document::{PackDocument, ParseError, ParseErrorKind};
pub use lint::{lint, Finding, Rule, Severity};
pub use lock::{CommitId, Lock, LockedPack, Sha256};
pub use manifest::{Manifest, ManifestPack};
pub use model::{
    Alias, Argv, CommandName, Dialect, Entry, EntryName, EntryRef, EnvEntry, EnvName, EnvValue, Function, FunctionBody,
    Group, InShell, InvalidValue, Os, OsSet, Pack, PackMeta, Slug, Text, Word,
};
pub use secrets::{Bindings, SecretSource};
pub use source::{GitRef, GitUrl, RepoPath, Source, SourceError};
