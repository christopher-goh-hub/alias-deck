//! Where a pack comes from. A raw string never travels past this boundary: every spelling is
//! parsed into a `Source` whose parts are each valid on their own, so a URL can never become a git
//! flag and a repository path can never climb out of the repository.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    Empty,
    /// Starts with `-`, so a tool could read it as an option.
    OptionLike,
    ContainsNul,
    Unrecognized,
    BadUrl,
    BadGithub,
    BadRepoPath,
    BadRef,
    /// A local file is read live, so a ref has nothing to pin.
    RefOnLocal,
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SourceError::Empty => "the source is empty",
            SourceError::OptionLike => "a source must not start with `-`",
            SourceError::ContainsNul => "the source contains a NUL byte",
            SourceError::Unrecognized => {
                "expected ./file.toml, /abs/file.toml, ~/file.toml, github:owner/repo[//path], or git+URL[//path]"
            }
            SourceError::BadUrl => "git URLs must be https://, ssh://, file://, or git@host:path, with no spaces",
            SourceError::BadGithub => "expected github:owner/repo, where owner and repo are plain names",
            SourceError::BadRepoPath => {
                "the path inside the repository must be relative, with no empty, `.`, or `..` parts"
            }
            SourceError::BadRef => "a ref is a branch, tag, or commit name such as `v2` or `main`",
            SourceError::RefOnLocal => "a local file is read live, so it takes no ref",
        })
    }
}

/// A remote git URL in one of the four accepted forms. Never starts with `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitUrl(String);

impl GitUrl {
    pub fn new(s: &str) -> Result<GitUrl, SourceError> {
        if s.starts_with('-') {
            return Err(SourceError::OptionLike);
        }
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(SourceError::BadUrl);
        }
        let rest = ["https://", "ssh://", "file://"].iter().find_map(|p| s.strip_prefix(p));
        let ok = match rest {
            Some(rest) => !rest.is_empty(),
            None => s
                .strip_prefix("git@")
                .and_then(|r| r.split_once(':'))
                .is_some_and(|(host, path)| !host.is_empty() && !host.contains('/') && !path.is_empty()),
        };
        if ok {
            Ok(GitUrl(s.to_owned()))
        } else {
            Err(SourceError::BadUrl)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A file path inside a repository: relative, `/`-separated, with no empty, `.`, or `..` part.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoPath(String);

impl RepoPath {
    pub const DEFAULT: &'static str = "pack.toml";

    pub fn new(s: &str) -> Result<RepoPath, SourceError> {
        let ok = !s.is_empty()
            && s.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
            && !s.contains(['\\', '\0'])
            && !s.chars().any(char::is_control);
        if ok {
            Ok(RepoPath(s.to_owned()))
        } else {
            Err(SourceError::BadRepoPath)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A branch, tag, or commit name. The grammar is a strict subset of git's, and it never starts
/// with `-`, so it is safe as a git argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitRef(String);

impl GitRef {
    pub fn new(s: &str) -> Result<GitRef, SourceError> {
        let ok = !s.is_empty()
            && s.len() <= 255
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))
            && !s.starts_with(['-', '/', '.'])
            && !s.ends_with(['/', '.'])
            && !s.ends_with(".lock")
            && !s.contains("..")
            && !s.contains("//");
        if ok {
            Ok(GitRef(s.to_owned()))
        } else {
            Err(SourceError::BadRef)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A full commit id, which `git ls-remote` cannot resolve and needs no resolving.
    pub fn is_commit_id(&self) -> bool {
        matches!(self.0.len(), 40 | 64) && self.0.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    }
}

impl fmt::Display for GitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// A file on this machine, as spelled: absolute, relative, or starting with `~/`. Read at
    /// every build. You own it, so it is not pinned.
    Local(PathBuf),
    /// A file at a path inside a git repository. Pinned through the lock to a commit and a
    /// content hash.
    Git { url: GitUrl, path: RepoPath, reference: Option<GitRef> },
}

impl Source {
    /// The spellings a manifest holds. A ref comes from the manifest's `ref` key, not the string.
    pub fn parse(s: &str) -> Result<Source, SourceError> {
        if s.is_empty() {
            return Err(SourceError::Empty);
        }
        if s.starts_with('-') {
            return Err(SourceError::OptionLike);
        }
        if s.contains('\0') {
            return Err(SourceError::ContainsNul);
        }
        if let Some(rest) = s.strip_prefix("github:") {
            let (repo, path) = split_path(rest);
            let (owner, name) = repo.split_once('/').ok_or(SourceError::BadGithub)?;
            let plain = |p: &str| {
                !p.is_empty()
                    && !p.starts_with(['-', '.'])
                    && p.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            };
            if !plain(owner) || !plain(name) {
                return Err(SourceError::BadGithub);
            }
            let url = GitUrl::new(&format!("https://github.com/{owner}/{name}.git"))?;
            return Ok(Source::Git { url, path: RepoPath::new(path.unwrap_or(RepoPath::DEFAULT))?, reference: None });
        }
        if let Some(rest) = s.strip_prefix("git+") {
            let after_scheme = rest.find("://").map_or(0, |i| i + 3);
            let (url, path) = match rest[after_scheme..].find("//") {
                Some(i) => (&rest[..after_scheme + i], Some(&rest[after_scheme + i + 2..])),
                None => (rest, None),
            };
            let url = GitUrl::new(url)?;
            return Ok(Source::Git { url, path: RepoPath::new(path.unwrap_or(RepoPath::DEFAULT))?, reference: None });
        }
        if ["/", "./", "../", "~/"].iter().any(|p| s.starts_with(p)) {
            return Ok(Source::Local(PathBuf::from(s)));
        }
        Err(SourceError::Unrecognized)
    }

    /// The command-line spelling, which may end in `@ref` for a git source: `github:a/b@v2`.
    pub fn parse_with_ref(s: &str) -> Result<Source, SourceError> {
        if let Some((base, r)) = s.rsplit_once('@') {
            if let (Ok(source @ Source::Git { .. }), Ok(r)) = (Source::parse(base), GitRef::new(r)) {
                return Ok(source.with_ref(Some(r)));
            }
        }
        Source::parse(s)
    }

    pub fn with_ref(self, reference: Option<GitRef>) -> Source {
        match self {
            Source::Git { url, path, .. } => Source::Git { url, path, reference },
            local => local,
        }
    }

    pub fn reference(&self) -> Option<&GitRef> {
        match self {
            Source::Git { reference, .. } => reference.as_ref(),
            Source::Local(_) => None,
        }
    }

    /// Two sources name the same pack when they differ at most in ref.
    pub fn same_pack(&self, other: &Source) -> bool {
        self.to_string() == other.to_string()
    }
}

fn split_path(s: &str) -> (&str, Option<&str>) {
    match s.split_once("//") {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    }
}

/// The canonical spelling, without the ref. It round-trips through `Source::parse` and is the
/// identity the manifest and the lock agree on.
impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Local(p) => write!(f, "{}", p.display()),
            Source::Git { url, path, .. } => {
                let github = url
                    .as_str()
                    .strip_prefix("https://github.com/")
                    .and_then(|r| r.strip_suffix(".git"))
                    .filter(|r| r.matches('/').count() == 1);
                match github {
                    Some(repo) => write!(f, "github:{repo}")?,
                    None => write!(f, "git+{}", url.as_str())?,
                }
                if path.as_str() != RepoPath::DEFAULT {
                    write!(f, "//{}", path.as_str())?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(s: &str) -> (String, String, Option<String>) {
        match Source::parse_with_ref(s).unwrap_or_else(|e| panic!("{s}: {e}")) {
            Source::Git { url, path, reference } => {
                (url.as_str().to_owned(), path.as_str().to_owned(), reference.map(|r| r.as_str().to_owned()))
            }
            other => panic!("{s} parsed as {other:?}"),
        }
    }

    #[test]
    fn accepted_spellings() {
        assert_eq!(
            git("github:alice/dotpacks"),
            ("https://github.com/alice/dotpacks.git".into(), "pack.toml".into(), None)
        );
        assert_eq!(git("github:alice/dotpacks//git.toml").1, "git.toml");
        assert_eq!(git("github:a/b@v2").2.as_deref(), Some("v2"));
        assert_eq!(git("github:a/b//dir/x.toml@feature/y").2.as_deref(), Some("feature/y"));
        assert_eq!(git("git+https://host/r.git//x.toml"), ("https://host/r.git".into(), "x.toml".into(), None));
        assert_eq!(git("git+file:///tmp/r//x.toml"), ("file:///tmp/r".into(), "x.toml".into(), None));
        assert_eq!(
            git("git+git@host.example:team/r.git//p.toml"),
            ("git@host.example:team/r.git".into(), "p.toml".into(), None)
        );
        for local in ["./x.toml", "../x.toml", "/abs/x.toml", "~/x.toml"] {
            assert_eq!(Source::parse(local), Ok(Source::Local(PathBuf::from(local))));
        }
    }

    #[test]
    fn display_is_canonical_and_round_trips() {
        for s in [
            "github:alice/dotpacks",
            "github:a/b//dir/x.toml",
            "git+file:///tmp/r//x.toml",
            "git+ssh://h/r.git",
            "~/x.toml",
        ] {
            let parsed = Source::parse(s).unwrap();
            assert_eq!(parsed.to_string(), s);
            assert_eq!(Source::parse(&parsed.to_string()).unwrap(), parsed);
        }
        assert_eq!(Source::parse("github:a/b//pack.toml").unwrap().to_string(), "github:a/b");
        assert_eq!(Source::parse("git+https://github.com/a/b.git").unwrap().to_string(), "github:a/b");
        assert!(Source::parse_with_ref("github:a/b@v1").unwrap().same_pack(&Source::parse("github:a/b").unwrap()));
    }

    #[test]
    fn rejects_flags_traversal_and_unknown_schemes() {
        let err = |s: &str| Source::parse_with_ref(s).unwrap_err();
        assert_eq!(err("--upload-pack=evil"), SourceError::OptionLike);
        assert_eq!(err("git+-oProxyCommand=x//p.toml"), SourceError::OptionLike);
        assert_eq!(err("git+http://host/r.git"), SourceError::BadUrl);
        assert_eq!(err("git+https://host/r .git"), SourceError::BadUrl);
        assert_eq!(err("github:a/b//../etc/passwd"), SourceError::BadRepoPath);
        assert_eq!(err("git+file:///tmp/r///abs.toml"), SourceError::BadRepoPath);
        assert_eq!(err("github:a/b//x//y.toml"), SourceError::BadRepoPath);
        assert_eq!(err("github:-a/b"), SourceError::BadGithub);
        assert_eq!(err("github:a"), SourceError::BadGithub);
        assert_eq!(err("x.toml"), SourceError::Unrecognized);
        assert_eq!(err(""), SourceError::Empty);
        assert_eq!(Source::parse_with_ref("./x.toml@v2").unwrap(), Source::Local(PathBuf::from("./x.toml@v2")));
    }

    #[test]
    fn ref_grammar() {
        for ok in ["v2", "main", "feature/x", "1.2.3", "3f1c0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e"] {
            assert!(GitRef::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "/x", "x/", "a..b", "a//b", "x.lock", "a b", "a:b", "a@{1}", ".x"] {
            assert_eq!(GitRef::new(bad), Err(SourceError::BadRef), "{bad}");
        }
        assert!(GitRef::new(&"a".repeat(40)).is_ok_and(|r| r.is_commit_id()));
        assert!(!GitRef::new("v2").unwrap().is_commit_id());
    }
}
