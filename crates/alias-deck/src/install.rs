//! Add, update, remove, and list. Each change is two steps: `fetch` or `check_update` read and
//! lint without writing anything, the caller shows the preview and asks, then `approve` writes
//! the store, the lock, and the manifest and builds.

use std::env;
use std::path::PathBuf;

use alias_deck_core::{
    lint, CommandName, CommitId, Entry, EntryName, EnvName, EnvValue, Finding, LockedPack, ManifestPack, PackDocument,
    Sha256, Slug, Source,
};

use crate::build::{load_installed, load_pack, parse_pack, save_installed};
use crate::{build, git, load_bindings, store, BuildReport, Error, Installed, Paths};

/// A pack read and linted but not approved. Nothing on disk has changed yet.
pub struct Candidate {
    /// With the ref it was resolved from, if any.
    pub source: Source,
    /// The resolved commit, for git packs.
    pub commit: Option<CommitId>,
    pub sha256: Sha256,
    pub text: String,
    pub doc: PackDocument,
    pub findings: Vec<Finding>,
}

pub fn fetch(paths: &Paths, source: &Source) -> Result<Candidate, Error> {
    let (commit, text, origin) = match source {
        Source::Local(spelled) => {
            let path = paths.resolve_local(spelled, &paths.config_dir());
            let text = std::fs::read_to_string(&path).map_err(Error::io(&path))?;
            (None, text, path.display().to_string())
        }
        Source::Git { url, path, reference } => {
            let commit = git::resolve(paths, url, reference.as_ref())?;
            let bytes = git::fetch_file(paths, url, &commit, path)?;
            let text = String::from_utf8(bytes).map_err(|_| Error::Command {
                command: format!("git cat-file blob {}:{}", commit, path.as_str()),
                detail: "the pack is not UTF-8 text".into(),
            })?;
            (Some(commit), text, source.to_string())
        }
    };
    let doc = parse_pack(origin, &text)?;
    let findings = lint(doc.pack());
    Ok(Candidate { source: source.clone(), commit, sha256: store::sha256(text.as_bytes()), text, doc, findings })
}

/// A name the candidate shares with another enabled pack. Later packs in the manifest win.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    pub name: EntryName,
    pub other: Slug,
    pub winner: Slug,
}

/// What the preview needs beyond the pack itself: facts about this machine and this deck.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub clashes: Vec<Clash>,
    /// Commands the pack defines that already exist on this machine's `PATH`.
    pub on_path: Vec<(CommandName, PathBuf)>,
    /// Each secret the pack asks for, and whether the user's secrets file binds it.
    pub secrets: Vec<(EnvName, bool)>,
}

/// Names a pack defines that can clash. PATH lists compose across packs, so they never clash.
fn clash_names(doc: &PackDocument) -> Vec<EntryName> {
    let mut names = Vec::new();
    for (_, e) in doc.pack().entries() {
        if matches!(e, Entry::Env(env) if matches!(env.value(), EnvValue::PathList { .. })) {
            continue;
        }
        if !names.contains(&e.name()) {
            names.push(e.name());
        }
    }
    names
}

/// Fails if another pack in the deck already has the candidate's name, because `remove <name>`
/// and the entry references in every report would become ambiguous.
pub fn context(paths: &Paths, installed: &Installed, cand: &Candidate) -> Result<Context, Error> {
    let name = &cand.doc.pack().meta().name;
    let packs = installed.manifest.packs();
    let position = packs.iter().position(|p| p.source.same_pack(&cand.source)).unwrap_or(packs.len());
    let mut clashes = Vec::new();
    let mine = clash_names(&cand.doc);
    for (i, mp) in packs.iter().enumerate().filter(|(_, p)| !p.source.same_pack(&cand.source)) {
        let Ok(other) = load_pack(paths, installed, mp) else { continue };
        let other_name = &other.pack().meta().name;
        if other_name == name {
            return Err(Error::NameTaken { name: name.clone(), source: mp.source.clone() });
        }
        if !mp.enabled {
            continue;
        }
        let winner = if i > position { other_name } else { name };
        for n in clash_names(&other).into_iter().filter(|n| mine.contains(n)) {
            clashes.push(Clash { name: n, other: other_name.clone(), winner: winner.clone() });
        }
    }

    let search: Vec<PathBuf> = env::var_os("PATH").map(|p| env::split_paths(&p).collect()).unwrap_or_default();
    let mut on_path = Vec::new();
    for n in &mine {
        let EntryName::Command(cmd) = n else { continue };
        if let Some(found) = search.iter().map(|d| d.join(cmd.as_str())).find(|p| is_executable(p)) {
            on_path.push((cmd.clone(), found));
        }
    }

    let bindings = load_bindings(paths)?;
    let secrets = cand
        .doc
        .pack()
        .entries()
        .filter_map(|(_, e)| match e {
            Entry::Env(env) if matches!(env.value(), EnvValue::Secret { .. }) => {
                Some((env.name().clone(), bindings.get(env.name()).is_some()))
            }
            _ => None,
        })
        .collect();
    Ok(Context { clashes, on_path, secrets })
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &std::path::Path) -> bool {
    p.is_file()
}

/// Writes the approved bytes to the store, pins them in the lock, lists the pack in the manifest
/// (keeping its place and enabled flag if it is already there), and builds.
pub fn approve(paths: &Paths, cand: &Candidate) -> Result<BuildReport, Error> {
    let mut installed = load_installed(paths)?;
    if let (Source::Git { url, path, .. }, Some(commit)) = (&cand.source, &cand.commit) {
        let sha256 = store::put(paths, &cand.text)?;
        let meta = cand.doc.pack().meta();
        installed.lock.insert(LockedPack {
            url: url.clone(),
            path: path.clone(),
            commit: commit.clone(),
            sha256,
            name: meta.name.clone(),
            version: meta.version.clone(),
        });
    }
    installed.manifest.upsert(&cand.source);
    save_installed(paths, &installed)?;
    build(paths)
}

pub enum UpdateCheck {
    /// A local file is read live, so there is nothing to pin or update.
    Local,
    /// The ref resolves to the same bytes the user approved.
    UpToDate,
    /// New bytes to review. `old` is `None` for a git pack that was never approved.
    Changed { old: Option<Box<PackDocument>>, new: Box<Candidate> },
}

pub fn check_update(paths: &Paths, installed: &Installed, mp: &ManifestPack) -> Result<UpdateCheck, Error> {
    if let Source::Local(_) = mp.source {
        return Ok(UpdateCheck::Local);
    }
    let new = fetch(paths, &mp.source)?;
    match installed.lock.get(&mp.source) {
        None => Ok(UpdateCheck::Changed { old: None, new: Box::new(new) }),
        Some(locked) if locked.sha256 == new.sha256 => Ok(UpdateCheck::UpToDate),
        Some(locked) => {
            let text = store::get(paths, &locked.sha256)?;
            let old = parse_pack(paths.store_pack(&locked.sha256).display().to_string(), &text)?;
            Ok(UpdateCheck::Changed { old: Some(Box::new(old)), new: Box::new(new) })
        }
    }
}

pub struct Removed {
    pub source: Source,
    pub report: BuildReport,
}

/// The manifest entry a user means by `query`: a source spelling, or a pack name.
pub fn find_pack(paths: &Paths, installed: &Installed, query: &str) -> Result<ManifestPack, Error> {
    let packs = installed.manifest.packs();
    let by_source = Source::parse_with_ref(query).ok().and_then(|s| installed.manifest.find(&s));
    let by_name = || {
        packs.iter().find(|mp| match installed.lock.get(&mp.source) {
            Some(locked) => locked.name.as_str() == query,
            None => load_pack(paths, installed, mp).is_ok_and(|d| d.pack().meta().name.as_str() == query),
        })
    };
    by_source.or_else(by_name).cloned().ok_or_else(|| Error::NotInDeck(query.to_owned()))
}

/// Store bytes are left alone: they are addressed by content, so a later re-add may use them.
pub fn remove(paths: &Paths, query: &str) -> Result<Removed, Error> {
    let mut installed = load_installed(paths)?;
    let source = find_pack(paths, &installed, query)?.source;
    installed.manifest.remove(&source);
    installed.lock.remove(&source);
    save_installed(paths, &installed)?;
    Ok(Removed { source, report: build(paths)? })
}

pub struct ListedPack {
    pub source: Source,
    pub enabled: bool,
    pub locked: Option<LockedPack>,
    pub pack: Result<PackDocument, Error>,
}

/// Every pack in precedence order, including ones that fail to load, so a broken pack shows up
/// with its reason instead of disappearing.
pub fn list(paths: &Paths) -> Result<Vec<ListedPack>, Error> {
    let installed = load_installed(paths)?;
    Ok(installed
        .manifest
        .packs()
        .iter()
        .map(|mp| ListedPack {
            source: mp.source.clone(),
            enabled: mp.enabled,
            locked: installed.lock.get(&mp.source).cloned(),
            pack: load_pack(paths, &installed, mp),
        })
        .collect())
}
