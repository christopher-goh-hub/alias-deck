//! `setup` against a temp HOME: one marked block however often it runs, and the rc file it
//! edits really loads the deck in the real shell.

mod common;

use std::fs;

use alias_deck::{approve, fetch, setup, BLOCK_START};
use alias_deck_core::{Shell, Source};
use common::{find_shell, pack, World};

fn world_with_alias() -> World {
    let w = World::new();
    let file = w.paths.home.join("mine.toml");
    fs::write(&file, pack("mine", "[groups.g.aliases]\nhello = \"echo hello\"\n")).unwrap();
    approve(&w.paths, &fetch(&w.paths, &Source::parse("~/mine.toml").unwrap()).unwrap()).unwrap();
    w
}

#[test]
fn setup_twice_leaves_one_block_and_keeps_the_users_lines() {
    let w = world_with_alias();
    let zshrc = w.paths.home.join(".zshrc");
    fs::write(&zshrc, "export MINE=1\n").unwrap();
    let first = setup(&w.paths, Shell::Zsh).unwrap();
    assert!(first[0].changed);
    assert_eq!(first[0].file, zshrc);
    let second = setup(&w.paths, Shell::Zsh).unwrap();
    assert!(!second[0].changed, "the second run changes nothing");
    let text = fs::read_to_string(&zshrc).unwrap();
    assert_eq!(text.matches(BLOCK_START).count(), 1, "{text}");
    assert!(text.starts_with("export MINE=1\n"));
    assert_eq!(text.lines().nth(second[0].line_number - 1), Some(second[0].line.as_str()));
}

#[test]
fn setup_follows_a_symlinked_rc_file() {
    let w = world_with_alias();
    let real = w.root.join("dotfiles-zshrc");
    fs::write(&real, "# mine\n").unwrap();
    std::os::unix::fs::symlink(&real, w.paths.home.join(".zshrc")).unwrap();
    setup(&w.paths, Shell::Zsh).unwrap();
    assert!(fs::symlink_metadata(w.paths.home.join(".zshrc")).unwrap().file_type().is_symlink());
    assert!(fs::read_to_string(&real).unwrap().contains(BLOCK_START));
}

#[test]
fn each_posix_shell_and_fish_load_the_deck_from_their_startup_file() {
    let w = world_with_alias();
    for (shell, bin, script) in [
        (Shell::Zsh, "zsh", "source $HOME/.zshrc; type hello"),
        (Shell::Bash, "bash", "shopt -s expand_aliases; source $HOME/.bashrc; type hello"),
        (Shell::Fish, "fish", "type hello"),
    ] {
        setup(&w.paths, shell).unwrap();
        setup(&w.paths, shell).unwrap();
        let Some(bin) = find_shell(bin) else { continue };
        let out = w.shell(&bin, &["-c"], script);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("hello"),
            "{shell}: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
