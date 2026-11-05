//! The IO half of alias-deck: fetching packs, the content-addressed store, the deck files,
//! building init files, and wiring them into a shell. The CLI and the future GUI call these same
//! functions; `alias-deck-core` stays pure.
//!
//! Every function takes a `Paths`, so a caller (or a test) decides where the deck lives instead
//! of this crate reading `$HOME` behind its back.

mod approval;
mod build;
mod error;
mod fsx;
mod git;
mod install;
mod paths;
mod store;

pub use approval::{decide, Confirm, Decision};
pub use build::{build, load_bindings, load_installed, BuildReport, Installed, Output};
pub use error::Error;
pub use install::{
    approve, check_update, context, fetch, list, remove, Candidate, Clash, Context, ListedPack, Removed, UpdateCheck,
};
pub use paths::{host_os, Paths};
pub use store::sha256;
