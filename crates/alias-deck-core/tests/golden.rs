//! `examples/devkit.toml` compiled for every shell must equal `tests/golden/*` byte for byte.
//! Regenerate with `UPDATE_GOLDEN=1 cargo test --test golden`, then prove the new files in the
//! real shells with `cargo test --test shells`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use alias_deck_core::{compile, Bindings, CompileOptions, Deck, Os, PackDocument, Shell, Target};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn deck_and_bindings() -> (Deck, Bindings) {
    let src = fs::read_to_string(root().join("examples/devkit.toml")).unwrap();
    let doc = PackDocument::parse(&src).unwrap_or_else(|e| panic!("examples/devkit.toml: {e:?}"));
    let bindings = Bindings::parse(&fs::read_to_string(root().join("examples/secrets.toml")).unwrap()).unwrap();
    (Deck::new(vec![doc.pack().clone()]), bindings)
}

fn check(name: &str, actual: &str, mismatches: &mut Vec<String>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_default();
    if expected != actual {
        mismatches.push(format!("{name} differs from compile() output; rerun with UPDATE_GOLDEN=1 and review"));
    }
}

#[test]
fn devkit_matches_golden_files() {
    let (deck, bindings) = deck_and_bindings();
    let mut mismatches = Vec::new();
    for shell in Shell::ALL {
        let out = compile(&deck, &bindings, Target { shell, os: Os::MacOs }, &CompileOptions::default());
        check(&format!("devkit.{}", shell.extension()), &out.source, &mut mismatches);
    }

    let mut report = String::new();
    let targets = Shell::ALL
        .map(|shell| Target { shell, os: Os::MacOs })
        .into_iter()
        .chain([Target { shell: Shell::Bash, os: Os::Linux }, Target { shell: Shell::Pwsh, os: Os::Windows }]);
    for target in targets {
        let out = compile(&deck, &bindings, target, &CompileOptions::default());
        writeln!(report, "[{} on {}]", target.shell, target.os).unwrap();
        for r in &out.entries {
            writeln!(report, "  {r}").unwrap();
            for n in &r.notes {
                writeln!(report, "    note: {n}").unwrap();
            }
        }
    }
    check("report.txt", &report, &mut mismatches);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn compile_is_deterministic() {
    let (deck, bindings) = deck_and_bindings();
    for shell in Shell::ALL {
        let t = Target { shell, os: Os::Linux };
        let a = compile(&deck, &bindings, t, &CompileOptions::default());
        let b = compile(&deck, &bindings, t, &CompileOptions::default());
        assert_eq!(a, b, "two compiles for {shell} differ");
    }
}
