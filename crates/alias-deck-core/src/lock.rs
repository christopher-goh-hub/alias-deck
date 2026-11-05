//! The lockfile (`deck.lock`): for each git pack, the commit and the content hash of the bytes the
//! user approved. Machine-written and deterministic. Local packs have no entry, because the user
//! owns them and they are read live.

use std::fmt;
use std::ops::Range;

use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use crate::document::{span, Cx, ParseError, ParseErrorKind};
use crate::model::Slug;
use crate::source::{GitUrl, RepoPath, Source};

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// A full git object id: 40 hex digits for SHA-1 repositories, 64 for SHA-256 ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    pub fn new(s: &str) -> Option<CommitId> {
        (matches!(s.len(), 40 | 64) && is_lower_hex(s)).then(|| CommitId(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        &self.0[..12]
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The SHA-256 of a pack's bytes, as 64 lowercase hex digits. Hashing is the caller's job, so
/// the core stays free of IO and crypto crates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sha256(String);

impl Sha256 {
    pub fn new(s: &str) -> Option<Sha256> {
        (s.len() == 64 && is_lower_hex(s)).then(|| Sha256(s.to_owned()))
    }

    pub fn from_digest(digest: [u8; 32]) -> Sha256 {
        Sha256(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One approved git pack. It holds the git parts directly, so a lock entry for a local file
/// cannot exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPack {
    pub url: GitUrl,
    pub path: RepoPath,
    pub commit: CommitId,
    pub sha256: Sha256,
    pub name: Slug,
    pub version: semver::Version,
}

impl LockedPack {
    pub fn source(&self) -> Source {
        Source::Git { url: self.url.clone(), path: self.path.clone(), reference: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Sorted by source, so the rendered file depends only on its contents.
    packs: Vec<LockedPack>,
}

impl Lock {
    pub fn parse(src: &str) -> Result<Lock, Vec<ParseError>> {
        let doc = Cx::parse_toml(src)?;
        let mut cx = Cx::new();
        let whole = 0..src.len();
        let root = doc.as_table();
        cx.only_keys(root, &["format", "packs"], &whole);
        cx.format_one(root, &whole);
        let mut lock = Lock::default();
        if let Some(item) = root.get("packs") {
            match item.as_array_of_tables() {
                None => cx.error(span(item, &whole), ParseErrorKind::WrongType { expected: "[[packs]] tables" }),
                Some(tables) => {
                    for t in tables.iter() {
                        let at = t.span().unwrap_or_else(|| whole.clone());
                        if let Some(p) = parse_locked(&mut cx, t, &at) {
                            if lock.get(&p.source()).is_some() {
                                cx.error(at, ParseErrorKind::DuplicateSource(p.source().to_string()));
                            }
                            lock.insert(p);
                        }
                    }
                }
            }
        }
        cx.finish(Some(lock))
    }

    pub fn packs(&self) -> &[LockedPack] {
        &self.packs
    }

    pub fn get(&self, source: &Source) -> Option<&LockedPack> {
        self.packs.iter().find(|p| p.source().same_pack(source))
    }

    /// Replaces the entry for the same source, if any.
    pub fn insert(&mut self, pack: LockedPack) {
        self.remove(&pack.source());
        self.packs.push(pack);
        self.packs.sort_by_key(|p| p.source().to_string());
    }

    pub fn remove(&mut self, source: &Source) -> bool {
        let before = self.packs.len();
        self.packs.retain(|p| !p.source().same_pack(source));
        self.packs.len() != before
    }

    pub fn render(&self) -> String {
        let mut doc = DocumentMut::new();
        doc["format"] = value(1);
        let mut tables = ArrayOfTables::new();
        for p in &self.packs {
            let mut t = Table::new();
            t["source"] = value(p.source().to_string());
            t["commit"] = value(p.commit.as_str());
            t["sha256"] = value(p.sha256.as_str());
            t["name"] = value(p.name.as_str());
            t["version"] = value(p.version.to_string());
            tables.push(t);
        }
        if !tables.is_empty() {
            doc.insert("packs", Item::ArrayOfTables(tables));
        }
        doc.to_string()
    }
}

fn parse_locked(cx: &mut Cx, t: &Table, at: &Range<usize>) -> Option<LockedPack> {
    const KEYS: [&str; 5] = ["source", "commit", "sha256", "name", "version"];
    cx.only_keys(t, &KEYS, at);
    let mut field = |key: &'static str| -> Option<(&Item, &str)> {
        let Some(item) = t.get(key) else {
            cx.error(at.clone(), ParseErrorKind::MissingKey(key));
            return None;
        };
        cx.str(item, at).map(|s| (item, s))
    };
    let (source, commit, sha256, name, version) =
        (field("source"), field("commit"), field("sha256"), field("name"), field("version"));
    let mut fail = |item: &Item, kind: ParseErrorKind| cx.error(span(item, at), kind);
    let git = source.and_then(|(item, s)| match Source::parse(s) {
        Ok(Source::Git { url, path, .. }) => Some((url, path)),
        Ok(Source::Local(_)) => {
            fail(item, ParseErrorKind::WrongType { expected: "a git source" });
            None
        }
        Err(e) => {
            fail(item, ParseErrorKind::BadSource(e));
            None
        }
    });
    let commit = commit.and_then(|(item, s)| {
        let c = CommitId::new(s);
        if c.is_none() {
            fail(item, ParseErrorKind::BadDigest { expected: "a full commit id (40 or 64 lowercase hex digits)" });
        }
        c
    });
    let sha256 = sha256.and_then(|(item, s)| {
        let h = Sha256::new(s);
        if h.is_none() {
            fail(item, ParseErrorKind::BadDigest { expected: "a SHA-256 digest (64 lowercase hex digits)" });
        }
        h
    });
    let name = name.and_then(|(item, s)| Slug::new(s).map_err(|e| fail(item, ParseErrorKind::Invalid(e))).ok());
    let version = version.and_then(|(item, s)| {
        semver::Version::parse(s).map_err(|e| fail(item, ParseErrorKind::BadVersion(e.to_string()))).ok()
    });
    let (url, path) = git?;
    Some(LockedPack { url, path, commit: commit?, sha256: sha256?, name: name?, version: version? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked(source: &str, n: u8) -> LockedPack {
        let Source::Git { url, path, .. } = Source::parse(source).unwrap() else { panic!("git source expected") };
        LockedPack {
            url,
            path,
            commit: CommitId::new(&format!("{n:x}").repeat(40)).unwrap(),
            sha256: Sha256::from_digest([n; 32]),
            name: Slug::new("p").unwrap(),
            version: semver::Version::new(1, 0, n.into()),
        }
    }

    #[test]
    fn renders_sorted_and_round_trips() {
        let mut lock = Lock::default();
        lock.insert(locked("github:z/z", 1));
        lock.insert(locked("github:a/a//x.toml", 2));
        let out = lock.render();
        assert_eq!(
            out,
            format!(
                "format = 1\n\n[[packs]]\nsource = \"github:a/a//x.toml\"\ncommit = \"{}\"\nsha256 = \"{}\"\nname = \"p\"\nversion = \"1.0.2\"\n\n[[packs]]\nsource = \"github:z/z\"\ncommit = \"{}\"\nsha256 = \"{}\"\nname = \"p\"\nversion = \"1.0.1\"\n",
                "2".repeat(40),
                "02".repeat(32),
                "1".repeat(40),
                "01".repeat(32)
            )
        );
        assert_eq!(Lock::parse(&out).unwrap(), lock);
    }

    #[test]
    fn insert_replaces_and_remove_drops() {
        let mut lock = Lock::default();
        lock.insert(locked("github:a/a", 1));
        lock.insert(locked("github:a/a", 2));
        assert_eq!(lock.packs().len(), 1);
        assert_eq!(lock.get(&Source::parse_with_ref("github:a/a@v9").unwrap()).unwrap().version.patch, 2);
        assert!(lock.remove(&Source::parse("github:a/a").unwrap()));
        assert_eq!(lock.render(), "format = 1\n");
    }

    #[test]
    fn rejects_short_digests_and_local_sources() {
        let kinds = |src: &str| Lock::parse(src).unwrap_err().into_iter().map(|e| e.kind).collect::<Vec<_>>();
        let good = locked("github:a/a", 1);
        let entry = |source: &str, commit: &str| {
            format!("format = 1\n[[packs]]\nsource = \"{source}\"\ncommit = \"{commit}\"\nsha256 = \"{}\"\nname = \"p\"\nversion = \"1.0.0\"\n", good.sha256)
        };
        assert!(Lock::parse(&entry("github:a/a", good.commit.as_str())).is_ok());
        assert!(matches!(kinds(&entry("github:a/a", "abc"))[..], [ParseErrorKind::BadDigest { .. }]));
        assert!(matches!(kinds(&entry("./x.toml", good.commit.as_str()))[..], [ParseErrorKind::WrongType { .. }]));
    }
}
