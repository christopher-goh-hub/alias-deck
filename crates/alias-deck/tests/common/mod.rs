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
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let paths = Paths { config_home: home.join(".config"), data_home: home.join(".local/share"), home };
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
        format!("git+file://{}//{path}", repo.display())
    }
}

pub fn pack(name: &str, body: &str) -> String {
    format!("format = 1\n[pack]\nname = \"{name}\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"test pack\"\n{body}")
}
