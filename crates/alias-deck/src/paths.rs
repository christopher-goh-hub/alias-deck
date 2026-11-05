use std::env;
use std::path::{Path, PathBuf};

use alias_deck_core::{Os, Sha256, Shell};

use crate::Error;

/// Where the deck lives. Derived once from the environment, or built directly by a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`, default `~/.config`. fish's `conf.d` lives under it too.
    pub config_home: PathBuf,
    /// `$XDG_DATA_HOME`, default `~/.local/share`.
    pub data_home: PathBuf,
    /// `$ZDOTDIR`, where zsh reads `.zshrc` instead of the home directory.
    pub zdotdir: Option<PathBuf>,
}

impl Paths {
    /// The XDG spec says to ignore a relative value, so one never points the deck at the cwd.
    pub fn from_env() -> Result<Paths, Error> {
        let abs = |var: &str| env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
        let home = abs("HOME").ok_or(Error::NoHome)?;
        Ok(Paths {
            config_home: abs("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            data_home: abs("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
            zdotdir: abs("ZDOTDIR"),
            home,
        })
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_home.join("alias-deck")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_home.join("alias-deck")
    }

    pub fn manifest(&self) -> PathBuf {
        self.config_dir().join("deck.toml")
    }

    pub fn lock(&self) -> PathBuf {
        self.config_dir().join("deck.lock")
    }

    pub fn secrets(&self) -> PathBuf {
        self.config_dir().join("secrets.toml")
    }

    pub fn store_pack(&self, sha: &Sha256) -> PathBuf {
        self.data_dir().join("packs").join(format!("{sha}.toml"))
    }

    pub fn init(&self, shell: Shell) -> PathBuf {
        self.data_dir().join(format!("init.{}", shell.extension()))
    }

    /// A local source as the user spelled it: `~/` is the home directory, and a relative path is
    /// relative to `base`.
    pub fn resolve_local(&self, spelled: &Path, base: &Path) -> PathBuf {
        match spelled.strip_prefix("~") {
            Ok(rest) => self.home.join(rest),
            Err(_) => base.join(spelled),
        }
    }
}

pub fn host_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    }
}
