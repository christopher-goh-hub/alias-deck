//! Policy decisions of `compile`, observed through its report and source.

use alias_deck_core::{
    compile, Bindings, CompileOptions, Compiled, Deck, Dialect, FishAliasStyle, Note, OmitReason, Os, Outcome,
    PackDocument, Shell, Target,
};

fn pack(name: &str, body: &str) -> alias_deck_core::Pack {
    let src = format!(
        "format = 1\n[pack]\nname = \"{name}\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"d\"\n{body}"
    );
    PackDocument::parse(&src).unwrap_or_else(|e| panic!("{e:?}")).pack().clone()
}

fn build(packs: Vec<alias_deck_core::Pack>, bindings: &str, shell: Shell, os: Os) -> Compiled {
    let b = Bindings::parse(bindings).unwrap();
    compile(&Deck::new(packs), &b, Target { shell, os }, &CompileOptions::default())
}

fn outcome(c: &Compiled, entry: &str) -> Outcome {
    c.entries
        .iter()
        .find(|r| r.entry.to_string() == entry)
        .unwrap_or_else(|| panic!("no report for {entry}"))
        .outcome
        .clone()
}

#[test]
fn later_pack_wins_and_the_loser_is_reported() {
    let shared = pack("shared", "[groups.g.aliases]\ngs = \"git status\"\n[groups.g.env]\nEDITOR = \"vi\"\n");
    let mine = pack("mine", "[groups.me.aliases]\ngs = \"git status -sb\"\n[groups.me.env]\nEDITOR = \"nvim\"\n");
    for shell in Shell::ALL {
        let c = build(vec![shared.clone(), mine.clone()], "", shell, Os::Linux);
        assert!(
            matches!(outcome(&c, "shared/g/gs"), Outcome::Omitted(OmitReason::ShadowedBy(r)) if r.to_string() == "mine/me/gs"),
            "{shell}: shared gs is shadowed by mine"
        );
        assert!(matches!(outcome(&c, "shared/g/EDITOR"), Outcome::Omitted(OmitReason::ShadowedBy(_))));
        assert_eq!(outcome(&c, "mine/me/gs"), Outcome::Emitted);
        assert!(c.source.contains("-sb") && !c.source.contains("vi\n"), "{shell}: only the winner is emitted");
    }
}

#[test]
fn os_variants_of_one_name_coexist_because_the_gate_runs_first() {
    let p = pack(
        "clip",
        "[groups.mac]\nos = [\"macos\"]\naliases.copy = \"pbcopy\"\n[groups.lin]\nos = [\"linux\"]\naliases.copy = \"xclip\"\n",
    );
    let mac = build(vec![p.clone()], "", Shell::Bash, Os::MacOs);
    assert_eq!(outcome(&mac, "clip/mac/copy"), Outcome::Emitted);
    assert!(matches!(outcome(&mac, "clip/lin/copy"), Outcome::Omitted(OmitReason::OsCondition { .. })));
    let lin = build(vec![p], "", Shell::Bash, Os::Linux);
    assert_eq!(outcome(&lin, "clip/lin/copy"), Outcome::Emitted);
}

#[test]
fn path_lists_compose_across_packs() {
    let a = pack("a", "[groups.g.env]\nPATH = { prepend = [\"/a\"] }\n");
    let b = pack("b", "[groups.g.env]\nPATH = { prepend = [\"/b\"] }\n");
    let c = build(vec![a, b], "", Shell::Bash, Os::Linux);
    assert!(c.entries.iter().all(|r| r.outcome == Outcome::Emitted));
    assert!(c.source.contains("*:/a:*") && c.source.contains("*:/b:*"));
}

#[test]
fn pwsh_names_collide_case_insensitively() {
    let p = pack("p", "[groups.a.aliases]\ngs = \"git status\"\n[groups.b.aliases]\nGS = \"git show\"\n");
    let pw = build(vec![p.clone()], "", Shell::Pwsh, Os::Linux);
    assert!(matches!(outcome(&pw, "p/a/gs"), Outcome::Omitted(OmitReason::ShadowedBy(_))));
    let sh = build(vec![p], "", Shell::Bash, Os::Linux);
    assert_eq!(outcome(&sh, "p/a/gs"), Outcome::Emitted, "bash names are case-sensitive");
}

#[test]
fn reserved_names_are_per_shell() {
    let p = pack("p", "[groups.g.aliases]\nend = \"git log\"\nstring = \"git show\"\n");
    let expect =
        [(Shell::Bash, false, false), (Shell::Zsh, true, false), (Shell::Fish, true, true), (Shell::Pwsh, true, false)];
    for (shell, end, string) in expect {
        let c = build(vec![p.clone()], "", shell, Os::Linux);
        assert_eq!(outcome(&c, "p/g/end").is_omitted(), end, "{shell}: end");
        assert_eq!(outcome(&c, "p/g/string").is_omitted(), string, "{shell}: string");
    }
}

#[test]
fn names_the_compiled_file_calls_are_reserved() {
    let p = pack(
        "p",
        "[groups.g.aliases]\nunalias = \"git a\"\nexport = \"git b\"\ncontains = \"git c\"\nJoin-Path = \"git d\"\n",
    );
    let expect = [
        (Shell::Bash, ["unalias", "export"].as_slice()),
        (Shell::Zsh, &["unalias", "export"]),
        (Shell::Fish, &["contains"]),
        (Shell::Pwsh, &["Join-Path"]),
    ];
    for (shell, reserved) in expect {
        let c = build(vec![p.clone()], "", shell, Os::Linux);
        for r in &c.entries {
            let name = r.entry.name.to_string();
            let want = reserved.contains(&name.as_str());
            assert_eq!(r.outcome.is_omitted(), want, "{shell}: {name} reserved = {want}");
        }
    }
}

#[test]
fn in_shell_bodies_pick_the_most_specific_dialect() {
    let p = pack("p", "[groups.g.functions.f]\nshell.posix = \"echo posix\"\nshell.zsh = \"echo zsh\"\n");
    assert!(build(vec![p.clone()], "", Shell::Bash, Os::Linux).source.contains("eval 'echo posix'"));
    let zsh = build(vec![p.clone()], "", Shell::Zsh, Os::Linux).source;
    assert!(zsh.contains("eval 'echo zsh'") && !zsh.contains("emulate"), "a zsh body runs without emulate");
    let fish = build(vec![p], "", Shell::Fish, Os::Linux);
    assert_eq!(
        outcome(&fish, "p/g/f"),
        Outcome::Omitted(OmitReason::NoBodyFor { shell: Shell::Fish, available: vec![Dialect::Posix, Dialect::Zsh] })
    );
}

#[test]
fn windows_omits_what_has_no_windows_form() {
    let p = pack(
        "p",
        "[groups.g.functions.f]\nscript = \"echo hi\"\n[groups.g.env]\nT = { secret = \"t\" }\nU = { secret = \"u\" }\nPATH = { append = [\"C:\\\\tools;x\"] }\n",
    );
    let c = build(
        vec![p],
        "T = { keychain = { service = \"s\", account = \"a\" } }\nU = { command = [\"op\"] }",
        Shell::Pwsh,
        Os::Windows,
    );
    assert_eq!(outcome(&c, "p/g/f"), Outcome::Omitted(OmitReason::NeedsPosixSh));
    assert_eq!(outcome(&c, "p/g/T"), Outcome::Omitted(OmitReason::NoSecretLookup { os: Os::Windows }));
    assert_eq!(outcome(&c, "p/g/U"), Outcome::Emitted, "a user command works anywhere");
    assert_eq!(outcome(&c, "p/g/PATH"), Outcome::Omitted(OmitReason::SeparatorInPathEntry { separator: ';' }));
}

#[test]
fn separator_in_a_path_entry_is_omitted_not_split() {
    let p = pack("p", "[groups.g.env]\nPATH = { prepend = [\"/a:/b\"] }\n");
    for shell in Shell::ALL {
        let c = build(vec![p.clone()], "", shell, Os::Linux);
        assert_eq!(outcome(&c, "p/g/PATH"), Outcome::Omitted(OmitReason::SeparatorInPathEntry { separator: ':' }));
        assert!(!c.source.contains("/a:/b"), "{shell}");
    }
}

#[test]
fn secrets_run_first_then_env_then_commands_whatever_the_file_order() {
    let p = pack(
        "p",
        "[groups.a.aliases]\nx = \"git\"\n[groups.b.env]\nPATH = { prepend = [\"/evil\"] }\nT = { secret = \"t\" }\n",
    );
    for shell in Shell::ALL {
        let s = build(vec![p.clone()], "T = { command = [\"lookup-tool\"] }", shell, Os::Linux).source;
        let (secret, path, alias) = (s.find("lookup-tool").unwrap(), s.find("/evil").unwrap(), s.find(" x").unwrap());
        assert!(secret < path && path < alias, "{shell}: secret, then PATH, then commands\n{s}");
    }
}

#[test]
fn unbound_secret_is_a_to_do_and_never_emitted() {
    let with = pack("p", "[groups.g.env]\nE = \"1\"\nT = { secret = \"t\" }\n");
    let without = pack("p", "[groups.g.env]\nE = \"1\"\n");
    for shell in Shell::ALL {
        let c = build(vec![with.clone()], "", shell, Os::MacOs);
        assert_eq!(outcome(&c, "p/g/T"), Outcome::Omitted(OmitReason::SecretUnbound));
        let bare = build(vec![without.clone()], "", shell, Os::MacOs);
        assert_eq!(c.source, bare.source, "{shell}: an unbound secret leaves no trace in the source");
    }
}

#[test]
fn pwsh_notes_argument_rewriting_only_where_a_wrapper_exists() {
    let p = pack("p", "[groups.g.aliases]\ng = \"git\"\ngs = \"git status\"\ngrep = \"grep\"\n");
    let c = build(vec![p.clone()], "", Shell::Pwsh, Os::Linux);
    let notes = |e: &str| c.entries.iter().find(|r| r.entry.to_string() == e).unwrap().notes.clone();
    assert_eq!(notes("p/g/g"), [], "Set-Alias keeps native argument passing");
    assert_eq!(notes("p/g/gs"), [Note::PwshArgRewriting]);
    assert_eq!(notes("p/g/grep"), [Note::PwshArgRewriting], "a self-reference cannot be a Set-Alias");
    assert!(c.source.contains("Set-Alias -Name g -Value git\n"));
    assert!(build(vec![p], "", Shell::Fish, Os::Linux).entries.iter().all(|r| r.notes.is_empty()));
}

#[test]
fn fish_abbr_style_is_a_compile_option() {
    let p = pack("p", "[groups.g.aliases]\ngs = \"git status -sb\"\n");
    let deck = Deck::new(vec![p]);
    let opts = CompileOptions { fish_aliases: FishAliasStyle::Abbr };
    let c = compile(&deck, &Bindings::default(), Target { shell: Shell::Fish, os: Os::Linux }, &opts);
    assert!(c.source.contains("abbr --add -- gs 'git status -sb'\n"), "{}", c.source);
}

#[test]
fn every_entry_gets_exactly_one_report_in_deck_order() {
    let a = pack("a", "[groups.g.aliases]\nx = \"git\"\ny = \"git\"\n");
    let b = pack("b", "[groups.g.aliases]\nx = \"git\"\n[groups.g.env]\nE = \"1\"\n");
    let c = build(vec![a, b], "", Shell::Zsh, Os::Linux);
    let order: Vec<String> = c.entries.iter().map(|r| r.entry.to_string()).collect();
    assert_eq!(order, ["a/g/x", "a/g/y", "b/g/E", "b/g/x"]);
}
