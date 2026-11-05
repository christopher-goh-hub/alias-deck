//! Registries end to end: git+file fixture indexes in a temp world, searched and installed from
//! by name through the real binary. No test reaches the official registry or the network.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use alias_deck::load_registries;
use alias_deck_core::{Lock, Shell};
use common::{pack, World};

const TOOLS: &str = "[groups.g.aliases]\nhello = \"echo hello\"\n";

fn cli(w: &World, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alias-deck"))
        .args(args)
        .envs(w.envs())
        .env_remove("SHELL")
        .stdin(Stdio::null())
        .current_dir(&w.root)
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
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

fn entry(name: &str, source: &str, description: &str, tags: &str) -> String {
    format!("[[packs]]\nname = \"{name}\"\nsource = \"{source}\"\ndescription = \"{description}\"\ntags = [{tags}]\n")
}

/// A repository holding `tools.toml` and an index that lists it, and a deck that consults it.
fn world() -> (World, std::path::PathBuf, String) {
    let w = World::new();
    let packs = w.repo("packs", &[("tools.toml", &pack("tools", TOOLS))]);
    let pack_source = World::source(&packs, "tools.toml");
    let index = format!("format = 1\n{}", entry("tools", &pack_source, "Greets you", "\"greeting\", \"demo\""));
    let reg = w.repo("registry", &[("index.toml", &index)]);
    consult(&w, &[&reg]);
    (w, reg, pack_source)
}

fn consult(w: &World, registries: &[&Path]) {
    let mut deck = String::from("format = 1\n");
    for r in registries {
        deck.push_str(&format!("\n[[registries]]\nsource = \"{}\"\n", World::source(r, "index.toml")));
    }
    fs::create_dir_all(w.paths.config_dir()).unwrap();
    fs::write(w.paths.manifest(), deck).unwrap();
}

#[test]
fn search_matches_name_description_and_tags() {
    let (w, _, pack_source) = world();
    let out = ok(&cli(&w, &["search", "greet"]));
    assert!(out.starts_with("tools  Greets you\n       tags: greeting, demo\n"), "{out}");
    assert!(out.contains(&format!("       source: {pack_source}\n")), "{out}");
    assert!(out.contains("From git+file://") && out.contains("fetched just now"), "{out}");
    assert!(ok(&cli(&w, &["search", "DEMO"])).starts_with("tools  "), "tags match too, in any case");
    assert!(ok(&cli(&w, &["search", "docker"])).starts_with("No packs match `docker`."));
}

#[test]
fn add_by_name_installs_the_listed_source_and_the_lock_does_not_depend_on_the_registry() {
    let (w, reg, pack_source) = world();
    let out = cli(&w, &["add", "tools"]);
    assert_eq!(out.status.code(), Some(1), "no terminal and no --yes: preview only");
    let preview = text(&out.stdout);
    assert!(preview.contains("listed in: git+file://") && preview.contains("A listing is not a review"), "{preview}");
    assert!(preview.contains("alias hello = echo hello"), "{preview}");

    ok(&cli(&w, &["add", "tools", "--yes"]));
    let lock = Lock::parse(&fs::read_to_string(w.paths.lock()).unwrap()).unwrap();
    assert_eq!(lock.packs()[0].source().to_string(), pack_source, "the lock pins the resolved source");
    assert!(fs::read_to_string(w.paths.manifest()).unwrap().contains(&format!("source = \"{pack_source}\"")));
    assert!(fs::read_to_string(w.paths.init(Shell::Zsh)).unwrap().contains("hello"));

    fs::remove_dir_all(&reg).unwrap();
    assert!(ok(&cli(&w, &["update"])).contains(": up to date"), "update never asks the registry");
    assert!(ok(&cli(&w, &["add", "tools"])).contains("already in your deck"), "a fresh cache resolves the name");
}

#[test]
fn an_unknown_name_names_the_registries_it_searched() {
    let (w, _, _) = world();
    let out = cli(&w, &["add", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("no registry lists a pack named `nope` (searched: git+file://"), "{stderr}");
    assert!(!w.paths.lock().exists());
}

#[test]
fn a_later_registry_wins_a_name_both_list() {
    let w = World::new();
    let first = w.repo("first", &[("p.toml", &pack("tools", TOOLS))]);
    let second = w.repo("second", &[("p.toml", &pack("tools", "[groups.g.aliases]\nhi = \"echo hi\"\n"))]);
    let reg_a = w.repo(
        "a",
        &[("index.toml", &format!("format = 1\n{}", entry("tools", &World::source(&first, "p.toml"), "first", "")))],
    );
    let reg_b = w.repo(
        "b",
        &[("index.toml", &format!("format = 1\n{}", entry("tools", &World::source(&second, "p.toml"), "second", "")))],
    );
    consult(&w, &[&reg_a, &reg_b]);
    let out = ok(&cli(&w, &["search"]));
    assert!(out.starts_with("tools  second\n"), "{out}");
    assert_eq!(out.matches("tools  ").count(), 1, "one line per name: {out}");
}

#[test]
fn the_cache_serves_young_indexes_and_stands_in_when_a_refresh_fails() {
    let (w, reg, _) = world();
    let fetched = load_registries(&w.paths, false).unwrap();
    assert!(fetched.failed.is_empty() && fetched.loaded[0].stale.is_none());
    let commit = fetched.loaded[0].commit.clone();

    let moved = w.root.join("registry-moved");
    fs::rename(&reg, &moved).unwrap();
    let cached = load_registries(&w.paths, false).unwrap();
    assert!(cached.loaded[0].stale.is_none(), "a young cache is used without fetching");
    assert_eq!(cached.loaded[0].commit, commit);

    let refreshed = load_registries(&w.paths, true).unwrap();
    assert!(refreshed.loaded[0].stale.is_some(), "a failed refresh falls back to the cache");
    let out = cli(&w, &["search", "--refresh"]);
    assert!(ok(&out).starts_with("tools  "));
    assert!(text(&out.stderr).contains("note: using the copy of git+file://"), "{}", text(&out.stderr));

    fs::remove_dir_all(w.paths.data_dir().join("registries")).unwrap();
    let out = cli(&w, &["search"]);
    assert_eq!(out.status.code(), Some(1), "no cache and no network is an error");
    assert!(text(&out.stderr).contains("cannot read registry git+file://"), "{}", text(&out.stderr));
}
