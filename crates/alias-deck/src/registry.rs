//! Registry indexes, fetched through the same git path as packs and cached under the data dir
//! with the commit they came from. A registry only turns a name into a source; installing from
//! that source is the usual fetch, preview, and approval.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use alias_deck_core::{CommitId, ParseError, RegistryEntry, RegistryIndex, Source};

use crate::build::load_installed;
use crate::fsx::{read_optional, write_atomic};
use crate::{git, store, Error, Paths};

/// How old a cached index may be before `search` and `add <name>` fetch it again.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// One registry's index, from the network or from the cache.
#[derive(Debug, Clone)]
pub struct Registry {
    /// With the ref it was resolved from.
    pub source: Source,
    pub commit: CommitId,
    pub index: RegistryIndex,
    /// How long ago this index was fetched. Zero when it was fetched just now.
    pub age: Duration,
    /// Set when a fetch failed and this is the older cached copy: why the fetch failed.
    pub stale: Option<String>,
}

/// An entry and the registry that listed it.
#[derive(Debug, Clone, Copy)]
pub struct Found<'a> {
    pub registry: &'a Registry,
    pub entry: &'a RegistryEntry,
}

/// Every registry the deck consults, in precedence order, with those that could not be read.
#[derive(Debug)]
pub struct Registries {
    pub loaded: Vec<Registry>,
    pub failed: Vec<(Source, Error)>,
}

impl Registries {
    /// The last registry that lists `name` wins, as later packs do.
    pub fn find(&self, name: &str) -> Option<Found<'_>> {
        self.loaded.iter().rev().find_map(|r| r.index.get(name).map(|entry| Found { registry: r, entry }))
    }

    /// Entries matching `term`, one per name (the winner), sorted by name.
    pub fn search(&self, term: &str) -> Vec<Found<'_>> {
        let mut names: Vec<&str> =
            self.loaded.iter().flat_map(|r| r.index.entries()).map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names.into_iter().filter_map(|n| self.find(n)).filter(|f| f.entry.matches(term)).collect()
    }
}

/// Reads each registry the manifest names. A cached index younger than `MAX_AGE` is used as is
/// unless `refresh`; otherwise the index is fetched, and if that fails the cache stands in.
pub fn load_registries(paths: &Paths, refresh: bool) -> Result<Registries, Error> {
    let sources = load_installed(paths)?.manifest.registries();
    let mut out = Registries { loaded: Vec::new(), failed: Vec::new() };
    for source in sources {
        match load(paths, &source, refresh) {
            Ok(r) => out.loaded.push(r),
            Err(e) => out.failed.push((source, e)),
        }
    }
    Ok(out)
}

fn load(paths: &Paths, source: &Source, refresh: bool) -> Result<Registry, Error> {
    let cached = read_cache(paths, source)?;
    if let Some(r) = cached.as_ref().filter(|r| !refresh && r.age < MAX_AGE) {
        return Ok(r.clone());
    }
    match fetch(paths, source) {
        Ok((commit, text)) => {
            let index = parse(source.to_string(), &text)?;
            let header = format!("# alias-deck cache of {source}, ref {}, commit {commit}\n", reference(source));
            write_atomic(&cache_path(paths, source), format!("{header}{text}").as_bytes())?;
            Ok(Registry { source: source.clone(), commit, index, age: Duration::ZERO, stale: None })
        }
        Err(e) => match cached {
            Some(r) => Ok(Registry { stale: Some(e.to_string()), ..r }),
            None => Err(e),
        },
    }
}

fn fetch(paths: &Paths, source: &Source) -> Result<(CommitId, String), Error> {
    let Source::Git { url, path, reference } = source else { unreachable!("the manifest accepts only git registries") };
    let commit = git::resolve(paths, url, reference.as_ref())?;
    let bytes = git::fetch_file(paths, url, &commit, path)?;
    let text = String::from_utf8(bytes).map_err(|_| Error::Command {
        command: format!("git cat-file blob {commit}:{}", path.as_str()),
        detail: "the registry index is not UTF-8 text".into(),
    })?;
    Ok((commit, text))
}

fn parse(origin: String, text: &str) -> Result<RegistryIndex, Error> {
    RegistryIndex::parse(text).map_err(|errors: Vec<ParseError>| Error::Parse { origin, src: text.to_owned(), errors })
}

fn reference(source: &Source) -> String {
    source.reference().map_or_else(|| "HEAD".to_owned(), |r| r.to_string())
}

/// One file per registry and ref, named by a hash of both, so any source spelling is a safe name.
fn cache_path(paths: &Paths, source: &Source) -> PathBuf {
    let key = store::sha256(format!("{source}@{}", reference(source)).as_bytes());
    paths.data_dir().join("registries").join(format!("{key}.toml"))
}

/// The cache is the index text after one comment line that records the commit. A cache that no
/// longer parses reads as missing, so the next fetch replaces it.
fn read_cache(paths: &Paths, source: &Source) -> Result<Option<Registry>, Error> {
    let path = cache_path(paths, source);
    let Some(text) = read_optional(&path)? else { return Ok(None) };
    let commit = text.lines().next().and_then(|l| l.rsplit(' ').next()).and_then(CommitId::new);
    let (Some(commit), Ok(index)) = (commit, RegistryIndex::parse(&text)) else { return Ok(None) };
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).map_err(Error::io(&path))?;
    let age = SystemTime::now().duration_since(modified).unwrap_or(Duration::ZERO);
    Ok(Some(Registry { source: source.clone(), commit, index, age, stale: None }))
}
