//! The deck files on disk and the build from them to the four init files.

use std::path::PathBuf;

use alias_deck_core::{
    compile, Bindings, CompileOptions, Compiled, Deck, Lock, Manifest, ManifestPack, PackDocument, Shell, Source,
    Target,
};

use crate::fsx::{read_optional, write_atomic};
use crate::{host_os, store, Error, Paths};

/// The user's manifest and lock. Either file may be missing, which reads as empty.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    pub manifest: Manifest,
    pub lock: Lock,
}

pub fn load_installed(paths: &Paths) -> Result<Installed, Error> {
    let parse_err =
        |path: PathBuf, src: String| move |errors| Error::Parse { origin: path.display().to_string(), src, errors };
    let manifest = match read_optional(&paths.manifest())? {
        None => Manifest::default(),
        Some(src) => Manifest::parse(&src).map_err(parse_err(paths.manifest(), src.clone()))?,
    };
    let lock = match read_optional(&paths.lock())? {
        None => Lock::default(),
        Some(src) => Lock::parse(&src).map_err(parse_err(paths.lock(), src.clone()))?,
    };
    Ok(Installed { manifest, lock })
}

/// The lock goes first: a lock entry with no manifest entry is ignored by every reader, and the
/// next write replaces it, so a crash between the two writes leaves nothing to clean up.
pub(crate) fn save_installed(paths: &Paths, installed: &Installed) -> Result<(), Error> {
    write_atomic(&paths.lock(), installed.lock.render().as_bytes())?;
    write_atomic(&paths.manifest(), installed.manifest.render().as_bytes())
}

pub fn load_bindings(paths: &Paths) -> Result<Bindings, Error> {
    match read_optional(&paths.secrets())? {
        None => Ok(Bindings::default()),
        Some(src) => Bindings::parse(&src).map_err(|errors| Error::Parse {
            origin: paths.secrets().display().to_string(),
            src: src.clone(),
            errors,
        }),
    }
}

pub(crate) fn parse_pack(origin: String, text: &str) -> Result<PackDocument, Error> {
    PackDocument::parse(text).map_err(|errors| Error::Parse { origin, src: text.to_owned(), errors })
}

/// A local pack is read live; a relative path in the manifest is relative to the manifest's
/// directory. A git pack is read only from the store, and only if the lock approved it.
pub(crate) fn load_pack(paths: &Paths, installed: &Installed, mp: &ManifestPack) -> Result<PackDocument, Error> {
    match &mp.source {
        Source::Local(spelled) => {
            let path = paths.resolve_local(spelled, &paths.config_dir());
            let text = std::fs::read_to_string(&path).map_err(Error::io(&path))?;
            parse_pack(path.display().to_string(), &text)
        }
        Source::Git { .. } => {
            let locked =
                installed.lock.get(&mp.source).ok_or_else(|| Error::NotApproved { source: mp.source.clone() })?;
            let text = store::get(paths, &locked.sha256)?;
            parse_pack(paths.store_pack(&locked.sha256).display().to_string(), &text)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    pub shell: Shell,
    pub path: PathBuf,
    pub compiled: Compiled,
}

#[derive(Debug, Clone)]
pub struct BuildReport {
    /// One per shell, in `Shell::ALL` order.
    pub outputs: Vec<Output>,
}

/// Compiles every enabled pack into all four init files, so switching shells needs no rebuild.
/// If any enabled pack fails to load, nothing is written: the init files from the last good
/// build hold only approved code, while a partial build would silently drop a pack.
pub fn build(paths: &Paths) -> Result<BuildReport, Error> {
    let installed = load_installed(paths)?;
    let bindings = load_bindings(paths)?;
    let mut packs = Vec::new();
    let mut failures = Vec::new();
    for mp in installed.manifest.packs().iter().filter(|p| p.enabled) {
        match load_pack(paths, &installed, mp) {
            Ok(doc) => packs.push(doc.pack().clone()),
            Err(e) => failures.push((mp.source.clone(), e)),
        }
    }
    if !failures.is_empty() {
        return Err(Error::Packs(failures));
    }
    let deck = Deck::new(packs);
    let mut outputs = Vec::new();
    for shell in Shell::ALL {
        let compiled = compile(&deck, &bindings, Target { shell, os: host_os() }, &CompileOptions::default());
        let path = paths.init(shell);
        write_atomic(&path, compiled.source.as_bytes())?;
        outputs.push(Output { shell, path, compiled });
    }
    Ok(BuildReport { outputs })
}
