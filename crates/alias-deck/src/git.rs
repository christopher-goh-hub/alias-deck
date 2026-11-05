//! Fetching through the `git` CLI, so the binary carries no HTTP or TLS stack and inherits the
//! user's SSH keys and credential helpers. Every URL goes after `--`, and the types already
//! refuse a URL, ref, or path that starts with `-`.

use std::path::Path;
use std::process::{Command, Stdio};

use alias_deck_core::{CommitId, GitRef, GitUrl, RepoPath};

use crate::{Error, Paths};

fn git(paths: &Paths, cwd: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, Error> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    // HOME and XDG_CONFIG_HOME pick git's config, so a caller's `Paths` decides it as well.
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("HOME", &paths.home)
        .env("XDG_CONFIG_HOME", &paths.config_home)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null());
    let command = format!("git {}", args.join(" "));
    let out = cmd.output().map_err(|e| Error::Command { command: command.clone(), detail: e.to_string() })?;
    if !out.status.success() {
        return Err(Error::Command { command, detail: String::from_utf8_lossy(&out.stderr).into_owned() });
    }
    Ok(out.stdout)
}

/// The commit a ref names right now. No ref means the remote's default branch.
pub fn resolve(paths: &Paths, url: &GitUrl, reference: Option<&GitRef>) -> Result<CommitId, Error> {
    if let Some(r) = reference.filter(|r| r.is_commit_id()) {
        return Ok(CommitId::new(r.as_str()).expect("is_commit_id checked the shape"));
    }
    let name = reference.map_or("HEAD", GitRef::as_str);
    let peeled = format!("{name}^{{}}");
    let out = git(paths, None, &["ls-remote", "--", url.as_str(), name, &peeled])?;
    let listing = String::from_utf8_lossy(&out);
    let refs: Vec<(&str, &str)> = listing.lines().filter_map(|l| l.split_once('\t')).collect();
    let wanted = if name == "HEAD" {
        vec!["HEAD".to_owned()]
    } else {
        vec![format!("refs/tags/{name}^{{}}"), format!("refs/tags/{name}"), format!("refs/heads/{name}")]
    };
    wanted.iter().find_map(|w| refs.iter().find(|(_, r)| r == w)).and_then(|(oid, _)| CommitId::new(oid)).ok_or_else(
        || Error::Command {
            command: format!("git ls-remote -- {} {name}", url.as_str()),
            detail: format!("no branch or tag named `{name}`"),
        },
    )
}

/// The bytes of one file at one commit, fetched shallowly into a throwaway repository.
pub fn fetch_file(paths: &Paths, url: &GitUrl, commit: &CommitId, path: &RepoPath) -> Result<Vec<u8>, Error> {
    let tmp = tempfile::tempdir().map_err(Error::io(std::env::temp_dir()))?;
    let dir = tmp.path();
    git(paths, Some(dir), &["init", "-q"])?;
    git(paths, Some(dir), &["fetch", "-q", "--depth", "1", "--no-tags", "--", url.as_str(), commit.as_str()])?;
    git(paths, Some(dir), &["cat-file", "blob", &format!("{}:{}", commit.as_str(), path.as_str())])
}
