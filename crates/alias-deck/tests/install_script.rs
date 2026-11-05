//! install.sh and install.ps1 against a fake release served from 127.0.0.1: the archive holds the
//! binary this build produced, and SHA256SUMS lists it. The happy path must install a binary that
//! runs; a SHA256SUMS that disagrees with the archive must install nothing.
//!
//! `cargo test --test install_script -- --nocapture` prints each script's output.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{fs, thread};

use common::{find_shell, real_path};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Serves the files in `dir` over plain HTTP on an ephemeral port, until the test exits.
fn serve(dir: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                header.clear();
            }
            let name = request.split(' ').nth(1).unwrap_or("/").trim_start_matches('/').to_owned();
            let body = (!name.contains("..") && !name.contains('/')).then(|| fs::read(dir.join(&name)).ok()).flatten();
            let mut stream = reader.into_inner();
            let _ = match body {
                Some(b) => stream
                    .write_all(
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len())
                            .as_bytes(),
                    )
                    .and_then(|_| stream.write_all(&b)),
                None => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
            };
        }
    });
    url
}

/// The target install.sh should pick on this machine, from how this test was compiled.
fn host_target() -> String {
    let os = if cfg!(target_os = "macos") { "apple-darwin" } else { "unknown-linux-musl" };
    format!("{}-{os}", std::env::consts::ARCH)
}

/// A release directory with one archive for `target`, a decoy for another target, and a
/// SHA256SUMS that lists both. `tamper` makes the listed checksum wrong for the real archive.
struct Release {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    work: PathBuf,
    archive: String,
}

fn release(archive_ext: &str, pack: impl Fn(&Path, &Path), tamper: bool) -> Release {
    let tmp = tempfile::tempdir().unwrap();
    let work = real_path(tmp.path());
    let dir = work.join("release");
    let stage = work.join("stage");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&stage).unwrap();
    let target = if archive_ext == "zip" { "x86_64-pc-windows-msvc".to_owned() } else { host_target() };
    let archive = format!("alias-deck-{VERSION}-{target}.{archive_ext}");
    pack(&stage, &dir.join(&archive));
    let decoy = format!("alias-deck-{VERSION}-riscv64gc-unknown-linux-musl.{archive_ext}");
    fs::write(dir.join(&decoy), b"not this one").unwrap();
    let mut sha = alias_deck::sha256(&fs::read(dir.join(&archive)).unwrap()).to_string();
    if tamper {
        sha = alias_deck::sha256(b"something else").to_string();
    }
    let decoy_sha = alias_deck::sha256(b"not this one");
    fs::write(dir.join("SHA256SUMS"), format!("{decoy_sha}  {decoy}\n{sha}  {archive}\n")).unwrap();
    Release { _tmp: tmp, dir, work, archive }
}

fn tarball(stage: &Path, archive: &Path) {
    fs::copy(env!("CARGO_BIN_EXE_alias-deck"), stage.join("alias-deck")).unwrap();
    fs::copy(root().join("LICENSE"), stage.join("LICENSE")).unwrap();
    let out = Command::new("tar")
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(stage)
        .args(["alias-deck", "LICENSE"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn show(label: &str, out: &Output) {
    println!("--- {label}: exit {:?}", out.status.code());
    print!("{}", String::from_utf8_lossy(&out.stdout));
    print!("{}", String::from_utf8_lossy(&out.stderr));
}

/// Every POSIX shell on this machine worth proving the script under: macOS's bash 3.2, the system
/// sh (dash on Debian and Ubuntu), and dash itself.
#[cfg(unix)]
fn posix_shells() -> Vec<PathBuf> {
    let mut shells: Vec<PathBuf> = vec![PathBuf::from("/bin/sh"), PathBuf::from("/bin/bash")];
    shells.extend(["/bin/dash", "/usr/bin/dash"].map(PathBuf::from));
    shells.retain(|s| s.exists());
    shells.dedup_by(|a, b| fs::canonicalize(a).ok() == fs::canonicalize(b).ok());
    shells
}

#[cfg(unix)]
fn run_sh(shell: &Path, rel: &Release, url: &str) -> (Output, PathBuf) {
    let dest = rel.work.join(format!("bin-{}", shell.display().to_string().replace('/', "_")));
    let out = Command::new(shell)
        .arg(root().join("install.sh"))
        .env("HOME", &rel.work)
        .env("ALIAS_DECK_INSTALL_DIR", &dest)
        .env("ALIAS_DECK_BASE_URL", url)
        .env_remove("ALIAS_DECK_VERSION")
        .output()
        .unwrap();
    (out, dest)
}

#[cfg(unix)]
#[test]
fn install_sh_installs_a_verified_binary_that_runs() {
    let rel = release("tar.gz", tarball, false);
    let url = serve(rel.dir.clone());
    for shell in posix_shells() {
        let (out, dest) = run_sh(&shell, &rel, &url);
        show(&format!("install.sh under {}", shell.display()), &out);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}: {}", shell.display(), String::from_utf8_lossy(&out.stderr));
        let bin = dest.join("alias-deck");
        assert!(stdout.starts_with(&format!("Installed alias-deck {VERSION} to {}", bin.display())), "{stdout}");
        assert!(stdout.contains("is not on your PATH") && stdout.contains("alias-deck add git-essentials"), "{stdout}");
        let version = Command::new(&bin).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&version.stdout), format!("alias-deck {VERSION}\n"));
        assert!(!dest.join(".alias-deck.new").exists());
    }
    assert!(rel.archive.ends_with(&format!("-{}.tar.gz", host_target())));
}

#[cfg(unix)]
#[test]
fn install_sh_refuses_an_archive_whose_checksum_does_not_match() {
    let rel = release("tar.gz", tarball, true);
    let url = serve(rel.dir.clone());
    for shell in posix_shells() {
        let (out, dest) = run_sh(&shell, &rel, &url);
        show(&format!("install.sh under {} with a tampered SHA256SUMS", shell.display()), &out);
        assert_eq!(out.status.code(), Some(1), "{}", shell.display());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(&format!("checksum mismatch for {}", rel.archive)), "{stderr}");
        assert!(stderr.contains("Nothing was installed."), "{stderr}");
        assert!(!dest.exists(), "{}: nothing is written on a mismatch", shell.display());
    }
}

fn zip_with(pwsh: &Path) -> impl Fn(&Path, &Path) + '_ {
    move |stage, archive| {
        fs::copy(env!("CARGO_BIN_EXE_alias-deck"), stage.join("alias-deck.exe")).unwrap();
        let out = Command::new(pwsh)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Compress-Archive -Path $env:STAGE -DestinationPath $env:ARCHIVE",
            ])
            .env("STAGE", stage.join("alias-deck.exe"))
            .env("ARCHIVE", archive)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
}

fn run_ps1(pwsh: &Path, rel: &Release, url: &str) -> (Output, PathBuf) {
    let dest = rel.work.join("bin");
    let out = Command::new(pwsh)
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(root().join("install.ps1"))
        .env("ALIAS_DECK_INSTALL_DIR", &dest)
        .env("ALIAS_DECK_BASE_URL", url)
        .env_remove("ALIAS_DECK_VERSION")
        .output()
        .unwrap();
    (out, dest)
}

/// The script targets Windows, so elsewhere the test checks the installed bytes instead of running
/// them; on Windows the script itself runs `alias-deck.exe --version`.
#[test]
fn install_ps1_installs_a_verified_binary() {
    let Some(pwsh) = find_shell("pwsh") else { return };
    let rel = release("zip", zip_with(&pwsh), false);
    let url = serve(rel.dir.clone());
    let (out, dest) = run_ps1(&pwsh, &rel, &url);
    show("install.ps1", &out);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let exe = dest.join("alias-deck.exe");
    assert_eq!(fs::read(&exe).unwrap(), fs::read(env!("CARGO_BIN_EXE_alias-deck")).unwrap());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let installed =
        if cfg!(windows) { format!("Installed alias-deck {VERSION} to") } else { "Installed alias-deck to".to_owned() };
    assert!(stdout.starts_with(&installed), "{stdout}");
    assert!(stdout.contains("is not on your PATH"), "{stdout}");
}

#[test]
fn install_ps1_refuses_an_archive_whose_checksum_does_not_match() {
    let Some(pwsh) = find_shell("pwsh") else { return };
    let rel = release("zip", zip_with(&pwsh), true);
    let url = serve(rel.dir.clone());
    let (out, dest) = run_ps1(&pwsh, &rel, &url);
    show("install.ps1 with a tampered SHA256SUMS", &out);
    assert!(!out.status.success());
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(all.contains("checksum mismatch") && all.contains("Nothing was installed."), "{all}");
    assert!(!dest.exists());
}
