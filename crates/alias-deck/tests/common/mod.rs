//! A throwaway world for each test: its own HOME, XDG dirs, and git fixture repositories, so no
//! test reads or writes the real user's files or touches the network.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use alias_deck::Paths;

pub struct World {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub paths: Paths,
}

impl World {
    pub fn new() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let root = real_path(tmp.path());
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let paths =
            Paths { config_home: home.join(".config"), data_home: home.join(".local/share"), zdotdir: None, home };
        World { _tmp: tmp, root, paths }
    }

    /// The environment a child process of this world runs with.
    pub fn envs(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.paths.home.clone()),
            ("XDG_CONFIG_HOME", self.paths.config_home.clone()),
            ("XDG_DATA_HOME", self.paths.data_home.clone()),
        ]
    }

    pub fn git(&self, repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.com", "-c", "commit.gpgsign=false"])
            .args(["-c", "tag.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .envs(self.envs())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    /// A repository holding `files`, committed. Returns its directory.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.root.join("repos").join(name);
        fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "-q"]);
        self.commit(&dir, files, "initial");
        dir
    }

    pub fn commit(&self, repo: &Path, files: &[(&str, &str)], message: &str) -> String {
        for (path, text) in files {
            let p = repo.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "-q", "-m", message]);
        self.git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    pub fn source(repo: &Path, path: &str) -> String {
        format!("git+{}//{path}", file_url(repo))
    }
}

/// The path with symlinks resolved, as macOS's `/var` is a link to `/private/var`. Windows gets
/// no `\\?\` prefix, which git and the shells do not accept.
pub fn real_path(p: &Path) -> PathBuf {
    let real = p.canonicalize().unwrap();
    match real.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) => PathBuf::from(plain),
        None => real,
    }
}

/// `file:///tmp/r` on Unix, `file:///C:/Users/r` on Windows.
pub fn file_url(p: &Path) -> String {
    let s = p.to_str().unwrap().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

pub fn pack(name: &str, body: &str) -> String {
    format!("format = 1\n[pack]\nname = \"{name}\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"test pack\"\n{body}")
}

/// Whether this run must exercise `name`: it is listed in `ALIAS_DECK_REQUIRE_SHELLS`
/// (comma-separated), or `ALIAS_DECK_REQUIRE_ALL_SHELLS=1` is set, as in the core harness.
pub fn shell_required(name: &str) -> bool {
    let all = std::env::var("ALIAS_DECK_REQUIRE_ALL_SHELLS").is_ok_and(|v| v == "1");
    all || std::env::var("ALIAS_DECK_REQUIRE_SHELLS").unwrap_or_default().split(',').any(|s| s.trim() == name)
}

/// A shell binary to run, or `None` when it is not installed and not required.
pub fn find_shell(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/bin"));
    }
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let found = dirs.into_iter().map(|d| d.join(&file)).find(|p| p.is_file() && runs(p));
    if found.is_none() {
        assert!(!shell_required(name), "{name} is required but not installed");
        eprintln!("skipped: {name} is not installed");
    }
    found
}

/// Windows ships `System32\bash.exe`, a WSL launcher that fails without a distribution.
fn runs(shell: &Path) -> bool {
    Command::new(shell).arg("--version").output().is_ok_and(|o| o.status.success())
}

impl World {
    /// Runs `script` in `shell` with this world's HOME and XDG dirs, and `extra_path` first on PATH.
    pub fn shell(&self, shell: &Path, args: &[&str], script: &str) -> std::process::Output {
        let path = std::env::join_paths([shell.parent().unwrap().to_owned()].into_iter().chain(
            std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default(),
        ))
        .unwrap();
        Command::new(shell)
            .args(args)
            .arg(script)
            .envs(self.envs())
            .env("PATH", path)
            .env_remove("ZDOTDIR")
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .output()
            .unwrap()
    }
}
