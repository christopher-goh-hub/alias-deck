use std::fmt;
use std::io;
use std::path::PathBuf;

use alias_deck_core::{ParseError, Sha256, Slug, Source};

#[derive(Debug)]
pub enum Error {
    /// `$HOME` is unset or not absolute, so no deck location can be derived.
    NoHome,
    Io {
        path: PathBuf,
        error: io::Error,
    },
    /// `origin` is a file path or a source spelling; `src` is kept for line and column numbers.
    Parse {
        origin: String,
        src: String,
        errors: Vec<ParseError>,
    },
    /// A child process (git, pwsh) failed or could not start.
    Command {
        command: String,
        detail: String,
    },
    /// The stored bytes no longer hash to what the user approved.
    Tampered {
        path: PathBuf,
        expected: Sha256,
        actual: Sha256,
    },
    /// A git pack in the manifest with no lock entry: nobody approved its bytes.
    NotApproved {
        source: Source,
    },
    NotInDeck(String),
    NameTaken {
        name: Slug,
        source: Source,
    },
    /// Some enabled packs could not be loaded, so nothing was built.
    Packs(Vec<(Source, Error)>),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
        let path = path.into();
        move |error| Error::Io { path, error }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoHome => f.write_str("$HOME (or %USERPROFILE% on Windows) is not set to an absolute path"),
            Error::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Error::Parse { origin, src, errors } => {
                for (i, e) in errors.iter().enumerate() {
                    let (line, col) = e.line_col(src);
                    if i > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{origin}:{line}:{col}: {e}")?;
                }
                Ok(())
            }
            Error::Command { command, detail } => write!(f, "{command} failed: {}", detail.trim_end()),
            Error::Tampered { path, expected, actual } => write!(
                f,
                "{} does not match what you approved (expected sha256 {expected}, found {actual}); \
                 run `alias-deck update` to review the pack again",
                path.display()
            ),
            Error::NotApproved { source } => {
                write!(f, "{source} is in your deck but was never approved; run `alias-deck update` to review it")
            }
            Error::NotInDeck(what) => write!(f, "no pack named or sourced `{what}` in your deck"),
            Error::NameTaken { name, source } => {
                write!(f, "a pack named `{}` is already in your deck from {source}", name.as_str())
            }
            Error::Packs(failures) => {
                f.write_str("nothing was built, because these packs could not be loaded:")?;
                for (source, e) in failures {
                    write!(f, "\n  {source}: {}", e.to_string().replace('\n', "\n    "))?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}
