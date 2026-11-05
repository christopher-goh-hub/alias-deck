//! Wires the built init file into a shell's startup file, inside a marked block that a re-run
//! replaces in place.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use alias_deck_core::{Os, Shell};

use crate::fsx::{read_optional, write_atomic};
use crate::{host_os, Error, Paths};

pub const BLOCK_START: &str = "# >>> alias-deck >>>";
pub const BLOCK_END: &str = "# <<< alias-deck <<<";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupChange {
    pub file: PathBuf,
    /// 1-based line number of `line` in the file after the change.
    pub line_number: usize,
    pub line: String,
    /// `false` when the file already held exactly this block.
    pub changed: bool,
}

pub fn setup(paths: &Paths, shell: Shell) -> Result<Vec<SetupChange>, Error> {
    let init = paths.init(shell);
    let init = init.to_string_lossy();
    let line = match shell {
        Shell::Bash | Shell::Zsh => format!("[ -f {q} ] && . {q}", q = posix_quote(&init)),
        Shell::Fish => format!("test -f {q}; and source {q}", q = fish_quote(&init)),
        Shell::Pwsh => format!("if (Test-Path -LiteralPath {q}) {{ . {q} }}", q = pwsh_quote(&init)),
    };
    rc_files(paths, shell)?.into_iter().map(|file| upsert_block(&file, &line)).collect()
}

fn rc_files(paths: &Paths, shell: Shell) -> Result<Vec<PathBuf>, Error> {
    Ok(match shell {
        Shell::Bash => {
            let mut files = vec![paths.home.join(".bashrc")];
            // macOS Terminal starts login shells, which read .bash_profile and not .bashrc.
            let profile = paths.home.join(".bash_profile");
            if host_os() == Os::MacOs && read_optional(&profile)?.is_some_and(|s| !s.contains(".bashrc")) {
                files.push(profile);
            }
            files
        }
        Shell::Zsh => vec![paths.zdotdir.as_ref().unwrap_or(&paths.home).join(".zshrc")],
        Shell::Fish => vec![paths.config_home.join("fish/conf.d/alias-deck.fish")],
        Shell::Pwsh => vec![pwsh_profile(paths)?],
    })
}

fn pwsh_profile(paths: &Paths) -> Result<PathBuf, Error> {
    let command = "pwsh -NoProfile -NonInteractive -Command $PROFILE.CurrentUserAllHosts".to_owned();
    let out = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$PROFILE.CurrentUserAllHosts"])
        .env("HOME", &paths.home)
        .env("XDG_CONFIG_HOME", &paths.config_home)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::Command { command: command.clone(), detail: e.to_string() })?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || path.is_empty() {
        return Err(Error::Command { command, detail: String::from_utf8_lossy(&out.stderr).into_owned() });
    }
    Ok(PathBuf::from(path))
}

fn upsert_block(file: &Path, line: &str) -> Result<SetupChange, Error> {
    let old = read_optional(file)?.unwrap_or_default();
    let (new, line_number) = with_block(&old, line);
    let changed = new != old;
    if changed {
        write_atomic(file, new.as_bytes())?;
    }
    Ok(SetupChange { file: file.to_owned(), line_number, line: line.to_owned(), changed })
}

/// Replaces every complete marked block with one new block at the first one's place, or appends
/// it. A start marker with no end marker is left alone rather than eating the rest of the file.
fn with_block(old: &str, line: &str) -> (String, usize) {
    let lines: Vec<&str> = old.split_inclusive('\n').collect();
    let is = |l: &str, marker: &str| l.trim_end() == marker;
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is(lines[i], BLOCK_START) {
            if let Some(len) = lines[i..].iter().position(|l| is(l, BLOCK_END)) {
                blocks.push((i, i + len));
                i += len;
            }
        }
        i += 1;
    }
    let block = format!("{BLOCK_START}\n{line}\n{BLOCK_END}\n");
    let mut out = String::new();
    let mut at = None;
    for (n, l) in lines.iter().enumerate() {
        match blocks.iter().find(|(s, e)| (*s..=*e).contains(&n)) {
            Some((s, _)) if *s == n && at.is_none() => {
                at = Some(out.matches('\n').count());
                out.push_str(&block);
            }
            Some(_) => {}
            None => out.push_str(l),
        }
    }
    let at = at.unwrap_or_else(|| {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        let at = out.matches('\n').count();
        out.push_str(&block);
        at
    });
    (out, at + 2)
}

fn posix_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn fish_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn pwsh_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}'..='\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "[ -f '/x' ] && . '/x'";

    #[test]
    fn appends_once_and_replaces_in_place() {
        let (once, n) = with_block("export A=1", LINE);
        assert_eq!(once, format!("export A=1\n{BLOCK_START}\n{LINE}\n{BLOCK_END}\n"));
        assert_eq!(n, 3);
        assert_eq!(with_block(&once, LINE), (once.clone(), 3), "a re-run changes nothing");
        let edited = format!("{once}alias x=y\n");
        let (again, _) = with_block(&edited.replace(LINE, "old line"), LINE);
        assert_eq!(again, edited, "the block is replaced where it stands");
    }

    #[test]
    fn duplicate_blocks_collapse_and_an_unclosed_marker_is_kept() {
        let block = format!("{BLOCK_START}\nold\n{BLOCK_END}\n");
        let (out, n) = with_block(&format!("a\n{block}b\n{block}c\n"), LINE);
        assert_eq!(out, format!("a\n{BLOCK_START}\n{LINE}\n{BLOCK_END}\nb\nc\n"));
        assert_eq!(n, 3);
        let unclosed = format!("{BLOCK_START}\nmine\n");
        let (out, _) = with_block(&unclosed, LINE);
        assert!(out.starts_with(&unclosed), "{out}");
    }

    #[test]
    fn quoting_survives_hostile_paths() {
        assert_eq!(posix_quote("/a b/it's"), "'/a b/it'\\''s'");
        assert_eq!(fish_quote("/a\\b/it's"), "'/a\\\\b/it\\'s'");
        assert_eq!(pwsh_quote("/it's"), "'/it''s'");
    }
}
