//! The `alias-deck` binary end to end: git+file fixture repositories, a temp HOME and XDG dirs,
//! and the real shells sourcing what it builds.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use alias_deck::BLOCK_START;
use alias_deck_core::{Lock, Shell};
use common::{find_shell, pack, World};

const TOOLS: &str = "[groups.git.aliases]\nhello = \"echo hello\"\n\
[groups.git.functions.greet]\nscript = 'echo hi from greet'\n\
[groups.nav.functions.mkcd]\nshell.posix = 'mkdir -p -- \"$1\" && cd -- \"$1\"'\nshell.fish = 'mkdir -p -- $argv[1]; and cd -- $argv[1]'\n\
[groups.env.env]\nPATH = { prepend = [\"~/.tools/bin\"] }\n";

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Runs the binary in `w`'s world. stdin is empty and not a terminal, as over plain `ssh host cmd`.
fn cli(w: &World, args: &[&str]) -> Output {
    cli_with_path(w, args, None)
}

fn cli_with_path(w: &World, args: &[&str], extra_path: Option<&Path>) -> Output {
    let mut path: Vec<PathBuf> = extra_path.into_iter().map(Path::to_owned).collect();
    path.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    Command::new(env!("CARGO_BIN_EXE_alias-deck"))
        .args(args)
        .envs(w.envs())
        .env("PATH", std::env::join_paths(path).unwrap())
        .env_remove("ZDOTDIR")
        .env_remove("SHELL")
        .stdin(Stdio::null())
        .current_dir(&w.root)
        .output()
        .unwrap()
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    );
    text(&out.stdout)
}

fn lock(w: &World) -> Lock {
    Lock::parse(&fs::read_to_string(w.paths.lock()).unwrap()).unwrap()
}

/// A world with the tools pack installed from a fixture repository through `add --yes`.
fn installed() -> (World, PathBuf, String) {
    let w = World::new();
    let repo = w.repo("tools", &[("tools.toml", &pack("tools", TOOLS))]);
    let source = World::source(&repo, "tools.toml");
    ok(&cli(&w, &["add", &source, "--yes"]));
    (w, repo, source)
}

#[test]
fn add_yes_writes_the_lock_the_store_and_four_init_files() {
    let (w, repo, source) = installed();
    let lock = lock(&w);
    let [locked] = lock.packs() else { panic!("one lock entry expected") };
    assert_eq!(locked.source().to_string(), source);
    assert_eq!(locked.commit.as_str(), w.git(&repo, &["rev-parse", "HEAD"]).trim());
    assert_eq!(fs::read_to_string(w.paths.store_pack(&locked.sha256)).unwrap(), pack("tools", TOOLS));
    assert!(fs::read_to_string(w.paths.manifest()).unwrap().contains(&format!("source = \"{source}\"")));
    for shell in Shell::ALL {
        assert!(fs::read_to_string(w.paths.init(shell)).unwrap().contains("hello"), "init file for {shell}");
    }
}

#[test]
fn add_without_a_terminal_shows_the_preview_and_refuses_without_writing() {
    let w = World::new();
    let repo = w.repo("tools", &[("tools.toml", &pack("tools", TOOLS))]);
    let out = cli(&w, &["add", &World::source(&repo, "tools.toml")]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("function greet (script: runs in /bin/sh)\n") && stdout.contains("| echo hi from greet"),
        "{stdout}"
    );
    assert!(stdout.contains("env PATH: prepend ~/.tools/bin"), "{stdout}");
    assert!(stdout.contains("alias hello = echo hello"), "{stdout}");
    assert!(stdout.contains("info   native-function"), "{stdout}");
    assert!(text(&out.stderr).contains("stdin is not a terminal"), "{}", text(&out.stderr));
    assert!(!w.paths.manifest().exists() && !w.paths.lock().exists() && !w.paths.data_dir().exists());
}

#[test]
fn sourcing_the_built_init_file_defines_the_alias_in_zsh_and_bash() {
    let (w, _, _) = installed();
    let run = "eval hello; eval greet; alias hello";
    let mut shells: Vec<(PathBuf, Vec<String>, Shell, &str)> = Vec::new();
    if let Some(zsh) = find_shell("zsh") {
        let script = format!("source \"$1\"; {run}");
        shells.push((zsh, vec!["-f".into(), "-c".into(), script], Shell::Zsh, "hello='echo hello'\n"));
    }
    let bash_3 = Some(PathBuf::from("/bin/bash")).filter(|p| p.exists());
    for bash in [bash_3, find_shell("bash")].into_iter().flatten() {
        let script = format!("shopt -s expand_aliases; source \"$1\"; {run}");
        shells.push((bash, vec!["-c".into(), script], Shell::Bash, "alias hello='echo hello'\n"));
    }
    for (bin, args, shell, alias_line) in shells {
        let out = Command::new(&bin)
            .args(&args)
            .arg("sh")
            .arg(w.paths.init(shell))
            .envs(w.envs())
            .env_remove("BASH_ENV")
            .output()
            .unwrap();
        assert_eq!(ok(&out), format!("hello\nhi from greet\n{alias_line}"), "{}", bin.display());
    }
}

#[test]
fn update_shows_the_changed_function_body() {
    let (w, repo, _) = installed();
    let changed = TOOLS.replace("echo hi from greet", "echo hello from greet");
    w.commit(&repo, &[("tools.toml", &pack("tools", &changed))], "friendlier greet");
    let out = cli(&w, &["update"]);
    assert_eq!(out.status.code(), Some(1), "no terminal, no --yes: shown but not applied");
    let stdout = text(&out.stdout);
    let expected = "  ~ greet\n    old: function greet (script: runs in /bin/sh)\n             | echo hi from greet\n    new: function greet (script: runs in /bin/sh)\n             | echo hello from greet\n";
    assert!(stdout.contains(expected), "{stdout}");
    assert!(stdout.contains("New findings: none"), "{stdout}");

    ok(&cli(&w, &["update", "--yes"]));
    assert!(fs::read_to_string(w.paths.init(Shell::Bash)).unwrap().contains("echo hello from greet"));
    assert!(ok(&cli(&w, &["update"])).contains(": up to date"));
}

#[test]
fn update_yes_refuses_a_new_danger_finding_without_allow() {
    let (w, repo, _) = installed();
    let approved = lock(&w);
    let evil = format!("{TOOLS}LD_PRELOAD = \"/tmp/fast.so\"\n");
    w.commit(&repo, &[("tools.toml", &pack("tools", &evil))], "go faster");

    let out = cli(&w, &["update", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("danger env-loader-hijack  tools/env/LD_PRELOAD"), "{}", text(&out.stdout));
    assert!(text(&out.stderr).contains("re-run with --allow env-loader-hijack"), "{}", text(&out.stderr));
    assert_eq!(lock(&w), approved, "the lock still pins the approved bytes");
    assert!(!fs::read_to_string(w.paths.init(Shell::Zsh)).unwrap().contains("LD_PRELOAD"));

    ok(&cli(&w, &["update", "--yes", "--allow", "env-loader-hijack"]));
    assert!(fs::read_to_string(w.paths.init(Shell::Zsh)).unwrap().contains("LD_PRELOAD"));
}

#[test]
fn update_refuses_a_rename_onto_another_packs_name() {
    let (w, repo, _) = installed();
    let other = w.repo("other", &[("p.toml", &pack("other", ""))]);
    ok(&cli(&w, &["add", &World::source(&other, "p.toml"), "--yes"]));
    w.commit(&repo, &[("tools.toml", &pack("other", TOOLS))], "rename");
    let out = cli(&w, &["update", "tools", "--yes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("a pack named `other` is already in your deck"), "{}", text(&out.stderr));
}

#[test]
fn tampering_with_the_store_fails_the_build() {
    let (w, _, _) = installed();
    let good = fs::read(w.paths.init(Shell::Zsh)).unwrap();
    let store = w.paths.store_pack(&lock(&w).packs()[0].sha256);
    fs::write(&store, pack("tools", "[groups.git.aliases]\nhello = \"curl evil.example\"\n")).unwrap();
    let out = cli(&w, &["build"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("nothing was built") && stderr.contains("does not match what you approved"), "{stderr}");
    assert_eq!(fs::read(w.paths.init(Shell::Zsh)).unwrap(), good);
}

#[test]
fn a_local_source_change_is_picked_up_live_by_build() {
    let w = World::new();
    let file = w.root.join("mine.toml");
    fs::write(&file, pack("mine", "[groups.g.aliases]\nll = \"ls -l\"\n")).unwrap();
    ok(&cli(&w, &["add", "./mine.toml", "--yes"]));
    assert!(
        fs::read_to_string(w.paths.manifest()).unwrap().contains(&format!("source = \"{}\"", file.display())),
        "a relative path is stored absolute"
    );
    fs::write(&file, pack("mine", "[groups.g.aliases]\nll = \"ls -la\"\n")).unwrap();
    ok(&cli(&w, &["build"]));
    assert!(fs::read_to_string(w.paths.init(Shell::Bash)).unwrap().contains("alias ll='ls -la'"));
    assert!(ok(&cli(&w, &["update"])).contains("a local file, read live"));
}

#[test]
fn build_twice_writes_identical_bytes_and_prints_the_pwsh_note_once() {
    let (w, _, _) = installed();
    let before: Vec<Vec<u8>> = Shell::ALL.iter().map(|s| fs::read(w.paths.init(*s)).unwrap()).collect();
    let out = cli(&w, &["build"]);
    ok(&out);
    let after: Vec<Vec<u8>> = Shell::ALL.iter().map(|s| fs::read(w.paths.init(*s)).unwrap()).collect();
    assert_eq!(before, after);
    let stderr = text(&out.stderr);
    let notes: Vec<&str> = stderr.lines().filter(|l| l.contains("note:")).collect();
    assert_eq!(notes, ["alias-deck: note: pwsh rewrites some caller arguments (a bare --, a,b, -Dx.y, wildcards) before 2 wrapper functions see them: tools/git/hello, tools/git/greet"], "{stderr}");
    assert!(stderr.contains("alias-deck: tools/nav/mkcd: omitted in pwsh: no body for pwsh"), "{stderr}");
}

#[test]
fn setup_twice_leaves_one_block_and_zsh_sees_the_alias() {
    let (w, _, _) = installed();
    for _ in 0..2 {
        ok(&cli(&w, &["setup", "--shell", "zsh"]));
    }
    let zshrc = fs::read_to_string(w.paths.home.join(".zshrc")).unwrap();
    assert_eq!(zshrc.matches(BLOCK_START).count(), 1, "{zshrc}");
    let Some(zsh) = find_shell("zsh") else { return };
    let out = w.shell(&zsh, &["-c"], "source $HOME/.zshrc; type hello");
    assert_eq!(ok(&out), "hello is an alias for echo hello\n");
}

#[test]
fn setup_for_pwsh_edits_its_profile_once_and_pwsh_loads_the_deck() {
    let (w, _, _) = installed();
    let Some(pwsh) = find_shell("pwsh") else { return };
    let dir = pwsh.parent().unwrap();
    for _ in 0..2 {
        ok(&cli_with_path(&w, &["setup", "--shell", "pwsh"], Some(dir)));
    }
    let profile = w.paths.config_home.join("powershell/profile.ps1");
    let text = fs::read_to_string(&profile).unwrap();
    assert_eq!(text.matches(BLOCK_START).count(), 1, "{text}");
    let out = w.shell(&pwsh, &["-NoLogo", "-NonInteractive", "-Command"], "greet");
    assert_eq!(ok(&out), "hi from greet\n");
}

#[test]
fn remove_by_name_and_list() {
    let (w, _, source) = installed();
    let listing = ok(&cli(&w, &["list"]));
    assert!(listing.starts_with(&format!("1. tools 1.0.0  {source}  commit ")), "{listing}");
    assert!(listing.contains("1 alias, 2 functions, 1 env var"), "{listing}");
    ok(&cli(&w, &["remove", "tools"]));
    assert!(lock(&w).packs().is_empty());
    assert!(!fs::read_to_string(w.paths.init(Shell::Zsh)).unwrap().contains("hello"));
    assert_eq!(cli(&w, &["remove", "tools"]).status.code(), Some(1));
}

/// Opt-in, macOS only: a real keychain item reaches a variable in a zsh that sources the build.
/// alias-deck runs in the temp HOME as usual. Only the final `zsh -f` runs with the real HOME,
/// because `security` finds the login keychain through it; `-f` keeps zsh from reading any rc file.
#[test]
fn a_keychain_secret_reaches_a_sourced_zsh() {
    if std::env::var("ALIAS_DECK_TEST_KEYCHAIN").as_deref() != Ok("1") || !cfg!(target_os = "macos") {
        eprintln!("skipped: set ALIAS_DECK_TEST_KEYCHAIN=1 on macOS to run the keychain test");
        return;
    }
    struct Item(String);
    impl Drop for Item {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", &self.0, "-a", "alias-deck-test"])
                .output();
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let item = Item(format!("alias-deck-test-{}-{nanos}", std::process::id()));
    let value = "kc value with 'quotes' and $dollar";
    let added = Command::new("/usr/bin/security")
        .args(["add-generic-password", "-s", &item.0, "-a", "alias-deck-test", "-w", value])
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", text(&added.stderr));

    let w = World::new();
    fs::create_dir_all(w.paths.config_dir()).unwrap();
    fs::write(
        w.paths.secrets(),
        format!("DECK_KC_TOKEN = {{ keychain = {{ service = \"{}\", account = \"alias-deck-test\" }} }}\n", item.0),
    )
    .unwrap();
    let repo =
        w.repo("kc", &[("p.toml", &pack("kc", "[groups.g.env]\nDECK_KC_TOKEN = { secret = \"test token\" }\n"))]);
    let out = cli(&w, &["add", &World::source(&repo, "p.toml"), "--yes"]);
    assert!(ok(&out).contains("DECK_KC_TOKEN: bound in your secrets file"));

    let zsh = find_shell("zsh").expect("zsh ships with macOS");
    let out = Command::new(zsh)
        .args(["-f", "-c", "source \"$1\"; printf %s \"$DECK_KC_TOKEN\"", "zsh"])
        .arg(w.paths.init(Shell::Zsh))
        .output()
        .unwrap();
    assert_eq!(ok(&out), value);
    drop(item);
    let gone = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", &format!("alias-deck-test-{}-{nanos}", std::process::id())])
        .output()
        .unwrap();
    assert!(!gone.status.success(), "the throwaway item is deleted");
}
