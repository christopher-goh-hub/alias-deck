//! The registry's CI gate. Every pack `registry/index.toml` lists from this repository must parse,
//! carry the name its entry gives, have no Danger finding, compile for every shell and OS with
//! nothing left out except by its own OS list or for lack of /bin/sh, and load cleanly in each
//! installed shell, where its functions do what they say.
//!
//! A pack in another repository cannot be fetched hermetically, so it is listed as skipped and a
//! maintainer runs `alias-deck lint` on it before merging.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use alias_deck::host_os;
use alias_deck_core::{
    compile, lint, Bindings, CompileOptions, Compiled, Deck, OmitReason, Os, Outcome, Pack, PackDocument,
    RegistryIndex, Severity, Shell, Source, Target,
};
use common::{find_shell, real_path};

const THIS_REPO: &str = "https://github.com/christopher-goh-hub/alias-deck.git";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Each listed pack that lives in this repository, parsed, with its entry's name checked.
fn starter_packs() -> Vec<Pack> {
    let index_path = root().join("registry/index.toml");
    let index = RegistryIndex::parse(&fs::read_to_string(&index_path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e:?}", index_path.display()));
    let mut packs = Vec::new();
    for entry in index.entries() {
        let Source::Git { url, path, .. } = &entry.source else { unreachable!("an index lists only git sources") };
        if url.as_str() != THIS_REPO {
            eprintln!(
                "skipped {}: lives in {}; a maintainer runs `alias-deck lint` on it",
                entry.name.as_str(),
                url.as_str()
            );
            continue;
        }
        let file = root().join(path.as_str());
        let src = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let doc = PackDocument::parse(&src).unwrap_or_else(|e| panic!("{}: {e:?}", file.display()));
        assert_eq!(doc.pack().meta().name, entry.name, "{}: [pack].name must equal the index name", file.display());
        packs.push(doc.pack().clone());
    }
    assert!(!packs.is_empty(), "the index lists no packs from this repository");
    packs
}

fn compiled(packs: &[Pack], shell: Shell, os: Os) -> Compiled {
    compile(&Deck::new(packs.to_vec()), &Bindings::default(), Target { shell, os }, &CompileOptions::default())
}

#[test]
fn starter_packs_have_no_danger_findings() {
    for pack in starter_packs() {
        let danger: Vec<String> =
            lint(&pack).into_iter().filter(|f| f.severity == Severity::Danger).map(|f| f.to_string()).collect();
        assert!(danger.is_empty(), "{}: {danger:?}", pack.meta().name.as_str());
    }
}

/// Together, so a name two starter packs both define shows up as a shadowing omission.
#[test]
fn starter_packs_compile_everywhere_with_nothing_missing() {
    let packs = starter_packs();
    for os in Os::ALL {
        for shell in Shell::ALL {
            for r in &compiled(&packs, shell, os).entries {
                if let Outcome::Omitted(reason) = &r.outcome {
                    let expected = matches!(reason, OmitReason::OsCondition { .. })
                        || (os == Os::Windows && *reason == OmitReason::NeedsPosixSh);
                    assert!(expected, "{} for {shell} on {os}: omitted: {reason}", r.entry);
                }
            }
        }
    }
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}

fn fixture() -> (tempfile::TempDir, Fixture) {
    let tmp = tempfile::tempdir().unwrap();
    let root = real_path(tmp.path());
    let repo = root.join("repo");
    for d in [root.join("a/b/c"), repo.join("sub"), root.join("arc"), root.join("out")] {
        fs::create_dir_all(d).unwrap();
    }
    let git = Command::new("git").args(["init", "-q"]).arg(&repo).env("HOME", &root).output().unwrap();
    assert!(git.status.success(), "{}", String::from_utf8_lossy(&git.stderr));
    fs::write(root.join("arc/hello.txt"), "packed\n").unwrap();
    if cfg!(unix) {
        let tar = Command::new("tar")
            .arg("-czf")
            .arg(root.join("arc.tgz"))
            .arg("-C")
            .arg(root.join("arc"))
            .arg("hello.txt")
            .output()
            .unwrap();
        assert!(tar.status.success());
    }
    (tmp, Fixture { root, repo })
}

/// The script each shell runs after sourcing: `MISSING name` for every emitted name the shell
/// does not know, then one line per function check. Paths arrive in env vars, so no quoting.
fn script(shell: Shell, names: &[String], functional: bool) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    let list = quoted.join(" ");
    let (defined, checks) = match shell {
        Shell::Bash | Shell::Zsh => (
            format!("for n in {list}; do type \"$n\" >/dev/null 2>&1 || echo \"MISSING $n\"; done"),
            "cd \"$ROOT/a/b/c\" && up 2 && echo \"UP $PWD\"\n\
             cd \"$ROOT\" && mkcd 'new dir' && echo \"MKCD $PWD\"\n\
             cdtmp && echo \"TMP $PWD\"\n\
             cd \"$REPO/sub\" && groot && echo \"GROOT $PWD\"\n\
             up x 2>/dev/null || echo \"UPBAD $?\"\n\
             if [ -n \"$EXTRACT\" ]; then cd \"$ROOT/out\" && extract ../arc.tgz && echo \"EXTRACT $(cat hello.txt)\"; fi",
        ),
        Shell::Fish => (
            format!("for n in {list}; type -q -- $n; or echo \"MISSING $n\"; end"),
            "cd \"$ROOT/a/b/c\"; and up 2; and echo \"UP $PWD\"\n\
             cd \"$ROOT\"; and mkcd 'new dir'; and echo \"MKCD $PWD\"\n\
             cdtmp; and echo \"TMP $PWD\"\n\
             cd \"$REPO/sub\"; and groot; and echo \"GROOT $PWD\"\n\
             up x 2>/dev/null; or echo \"UPBAD $status\"\n\
             if test -n \"$EXTRACT\"; cd \"$ROOT/out\"; and extract ../arc.tgz; and echo \"EXTRACT \"(cat hello.txt); end",
        ),
        Shell::Pwsh => (
            format!(
                "foreach ($n in @({})) {{ if (-not (Get-Command -Name $n -ErrorAction Ignore)) {{ \"MISSING $n\" }} }}",
                quoted.join(", ")
            ),
            "Set-Location -LiteralPath \"$env:ROOT/a/b/c\"; up 2; \"UP $PWD\"\n\
             Set-Location -LiteralPath $env:ROOT; mkcd 'new dir'; \"MKCD $PWD\"\n\
             cdtmp; \"TMP $PWD\"\n\
             Set-Location -LiteralPath \"$env:REPO/sub\"; groot; \"GROOT $PWD\"\n\
             up x 2>$null; \"UPBAD 2\"",
        ),
    };
    let checks = if functional { checks } else { "" };
    format!("{defined}\n{checks}\n")
}

/// The lines a script prints when every function works, given where the fixture lives.
fn expected(fx: &Fixture, extract: bool) -> Vec<String> {
    let mut want = vec![
        format!("UP {}", fx.root.join("a").display()),
        format!("MKCD {}", fx.root.join("new dir").display()),
        format!("GROOT {}", fx.repo.display()),
        "UPBAD 2".to_owned(),
    ];
    if extract {
        want.push("EXTRACT packed".to_owned());
    }
    want
}

#[test]
fn starter_packs_load_cleanly_and_work_in_each_installed_shell() {
    let packs = starter_packs();
    let os = host_os();
    let (_tmp, fx) = fixture();
    let bash_3 = Some(PathBuf::from("/bin/bash")).filter(|p| cfg!(target_os = "macos") && p.exists());
    let shells: Vec<(Shell, PathBuf)> = [
        (Shell::Bash, bash_3),
        (Shell::Bash, find_shell("bash")),
        (Shell::Zsh, find_shell("zsh")),
        (Shell::Fish, find_shell("fish")),
        (Shell::Pwsh, find_shell("pwsh")),
    ]
    .into_iter()
    .filter_map(|(s, p)| Some((s, p?)))
    .collect();
    for (shell, bin) in shells {
        let out = compiled(&packs, shell, os);
        let init = fx.root.join(format!("init.{}", shell.extension()));
        fs::write(&init, &out.source).unwrap();
        let names: Vec<String> = out
            .entries
            .iter()
            .filter(|r| r.outcome == Outcome::Emitted && !r.entry.name.to_string().contains(char::is_uppercase))
            .map(|r| r.entry.name.to_string())
            .collect();
        // Git Bash prints POSIX spellings of Windows paths, so on Windows only definitions are checked.
        let functional = !(cfg!(windows) && shell == Shell::Bash);
        let extract = cfg!(unix) && shell != Shell::Pwsh;
        let body = script(shell, &names, functional);
        let (args, program): (Vec<&str>, String) = match shell {
            Shell::Bash => {
                (vec!["--norc", "--noprofile", "-c"], format!("shopt -s expand_aliases\nsource \"$INIT\"\n{body}"))
            }
            Shell::Zsh => (vec!["-f", "-c"], format!("source \"$INIT\"\n{body}")),
            Shell::Fish => (vec!["--no-config", "-c"], format!("source \"$INIT\"\n{body}")),
            Shell::Pwsh => (vec!["-NoProfile", "-NonInteractive", "-Command"], format!(". $env:INIT\n{body}")),
        };
        let run = Command::new(&bin)
            .args(&args)
            .arg(&program)
            .env("HOME", &fx.root)
            .env("INIT", &init)
            .env("ROOT", &fx.root)
            .env("REPO", &fx.repo)
            .env("EXTRACT", if extract { "1" } else { "" })
            .env_remove("ZDOTDIR")
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .current_dir(&fx.root)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
        let label = format!("{shell} at {}", bin.display());
        assert!(
            run.status.success() && stderr.is_empty(),
            "{label}: exit {:?}\n{stdout}\nstderr:\n{stderr}",
            run.status
        );
        let lines: Vec<&str> = stdout.lines().collect();
        assert!(!lines.iter().any(|l| l.starts_with("MISSING")), "{label}: {stdout}");
        if functional {
            for want in expected(&fx, extract) {
                assert!(lines.contains(&want.as_str()), "{label}: expected `{want}` in\n{stdout}");
            }
            let tmp = lines.iter().find_map(|l| l.strip_prefix("TMP ")).unwrap_or_else(|| panic!("{label}: {stdout}"));
            assert!(Path::new(tmp).is_dir() && Path::new(tmp) != fx.root, "{label}: cdtmp went to {tmp}");
            let _ = fs::remove_dir_all(fx.root.join("new dir"));
            let _ = fs::remove_file(fx.root.join("out/hello.txt"));
        }
    }
}
