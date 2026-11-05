//! Text for people. Every string that came from a pack goes through `visible` first, so a
//! hostile pack cannot hide a line of its preview behind terminal escape codes or bidi controls.

use std::fmt::Write as _;

use std::time::Duration;

use alias_deck::{Candidate, Context, ListedPack, Registries, Registry};
use alias_deck_core::{
    Argv, CommitId, Compiled, Entry, EntryChange, EnvValue, Finding, FunctionBody, Group, Note, OsSet, Outcome,
    PackDiff, PackDocument, Severity, Shell, Word,
};

/// Escapes every control character except newline and tab, and the Unicode controls that
/// reorder or hide text.
pub fn visible(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let hidden =
            matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
        if (c.is_control() && c != '\n' && c != '\t') || hidden {
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

fn word(w: &Word) -> String {
    visible(&w.to_string())
}

fn is_plain(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./:=@%+,-~".contains(c))
}

/// `git status -sb` when every word is plain, else a quoted list, so spaces and quotes inside one
/// argument stay visible.
fn argv(a: &Argv) -> String {
    let words: Vec<String> = std::iter::once(a.program()).chain(a.args()).map(|w| w.to_string()).collect();
    if words.iter().all(|w| is_plain(w)) {
        words.join(" ")
    } else {
        let quoted: Vec<String> = words.iter().map(|w| visible(&format!("{w:?}"))).collect();
        format!("[{}]", quoted.join(", "))
    }
}

fn os_tag(g: &Group) -> String {
    if g.os == OsSet::ALL {
        String::new()
    } else {
        format!(" [{} only]", g.os)
    }
}

fn body_lines(out: &mut String, indent: &str, text: &str) {
    for line in visible(text).lines() {
        let _ = writeln!(out, "{indent}| {line}");
    }
}

fn path_list(prepend: &[Word], append: &[Word]) -> String {
    let join = |ws: &[Word]| ws.iter().map(word).collect::<Vec<_>>().join(", ");
    match (prepend.is_empty(), append.is_empty()) {
        (false, false) => format!("prepend {}; append {}", join(prepend), join(append)),
        (false, true) => format!("prepend {}", join(prepend)),
        _ => format!("append {}", join(append)),
    }
}

/// One entry in full: the whole body for a function.
fn entry(out: &mut String, indent: &str, g: &Group, e: &Entry) {
    let os = os_tag(g);
    match e {
        Entry::Alias(a) => {
            let _ = writeln!(out, "{indent}alias {} = {}{os}", a.name.as_str(), argv(&a.argv));
        }
        Entry::Env(env) => {
            let name = env.name().as_str();
            let _ = match env.value() {
                EnvValue::Set(w) => writeln!(out, "{indent}env {name} = {}{os}", word(w)),
                EnvValue::PathList { prepend, append } => {
                    writeln!(out, "{indent}env {name}: {}{os}", path_list(prepend, append))
                }
                EnvValue::Secret { purpose } => {
                    writeln!(out, "{indent}env {name}: secret, \"{}\"{os}", visible(purpose.as_str()))
                }
            };
        }
        Entry::Function(f) => {
            let kind = match f.body {
                FunctionBody::Script(_) => "script: runs in /bin/sh",
                FunctionBody::InShell(_) => "native: runs inside your shell",
            };
            let _ = writeln!(out, "{indent}function {} ({kind}){os}", f.name.as_str());
            let inner = format!("{indent}    ");
            if let Some(d) = &f.description {
                let _ = writeln!(out, "{inner}{}", visible(d.as_str()));
            }
            match &f.body {
                FunctionBody::Script(t) => body_lines(out, &inner, t.as_str()),
                FunctionBody::InShell(bodies) => {
                    for d in bodies.dialects() {
                        let _ = writeln!(out, "{inner}{d}:");
                        body_lines(out, &inner, bodies.get(d).map_or("", |t| t.as_str()));
                    }
                }
            }
        }
    }
}

fn header(out: &mut String, doc: &PackDocument) {
    let m = doc.pack().meta();
    let _ = writeln!(out, "Pack {} {}, license {}", m.name.as_str(), m.version, visible(m.license.as_str()));
    let _ = writeln!(out, "  {}", visible(m.description.as_str()));
}

fn source_lines(out: &mut String, cand: &Candidate) {
    let reference = cand.source.reference().map(|r| format!(" (ref {r})")).unwrap_or_default();
    let _ = writeln!(out, "  source: {}{reference}", visible(&cand.source.to_string()));
    match &cand.commit {
        Some(c) => {
            let _ = writeln!(out, "  commit: {c}");
        }
        None => {
            let _ = writeln!(out, "  a local file: read live at every build, never pinned");
        }
    }
    let _ = writeln!(out, "  sha256: {}", cand.sha256);
}

/// Everything the pack adds, before anything is written. `via` is the registry that turned the
/// name the user typed into this source.
pub fn preview(cand: &Candidate, ctx: &Context, via: Option<&Registry>) -> String {
    let mut out = String::new();
    header(&mut out, &cand.doc);
    source_lines(&mut out, cand);
    if let Some(r) = via {
        let _ = writeln!(
            out,
            "  listed in: {} (commit {}). A listing is not a review: read what follows.",
            visible(&r.source.to_string()),
            r.commit.short()
        );
    }
    let pack = cand.doc.pack();
    let section = |out: &mut String, title: &str, keep: fn(&Entry) -> bool| {
        let entries: Vec<(&Group, &Entry)> = pack.entries().filter(|(_, e)| keep(e)).collect();
        if entries.is_empty() {
            return;
        }
        let _ = writeln!(out, "\n{title} ({})", entries.len());
        for (g, e) in entries {
            entry(out, "  ", g, e);
        }
    };
    section(&mut out, "Functions", |e| matches!(e, Entry::Function(_)));
    section(&mut out, "Environment and PATH", |e| matches!(e, Entry::Env(_)));
    if !ctx.secrets.is_empty() {
        let _ = writeln!(out, "\nSecrets");
        for (name, bound) in &ctx.secrets {
            let status = if *bound { "bound in your secrets file" } else { "not bound: left out until you bind it" };
            let _ = writeln!(out, "  {}: {status}", name.as_str());
        }
    }
    section(&mut out, "Aliases", |e| matches!(e, Entry::Alias(_)));
    if !ctx.on_path.is_empty() || !ctx.clashes.is_empty() {
        let _ = writeln!(out, "\nShadowing");
        for (name, path) in &ctx.on_path {
            let _ = writeln!(out, "  {}: replaces the command at {}", name.as_str(), path.display());
        }
        for c in &ctx.clashes {
            let _ =
                writeln!(out, "  {}: also defined by pack {}; {} wins", c.name, c.other.as_str(), c.winner.as_str());
        }
    }
    findings_section(&mut out, &cand.findings);
    out
}

/// `3 hours ago`, rounded down to the largest whole unit.
fn ago(d: Duration) -> String {
    let s = d.as_secs();
    let (n, unit) = match s {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (s / 60, "minute"),
        3600..=86_399 => (s / 3600, "hour"),
        _ => (s / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Registries that could not be read, or that came from an old cache because a fetch failed.
pub fn registry_notes(registries: &Registries) -> String {
    let mut out = String::new();
    for (source, e) in &registries.failed {
        let _ = writeln!(out, "alias-deck: cannot read registry {source}: {}", e.to_string().replace('\n', "; "));
    }
    for r in registries.loaded.iter().filter(|r| r.stale.is_some()) {
        let why = r.stale.as_deref().unwrap_or_default().replace('\n', "; ");
        let _ = writeln!(
            out,
            "alias-deck: note: using the copy of {} cached {} (commit {}), because fetching it failed: {why}",
            r.source,
            ago(r.age),
            r.commit.short()
        );
    }
    out
}

/// Each matching pack with its description, tags, and source, then where the list came from.
pub fn search(registries: &Registries, term: &str) -> String {
    let mut out = String::new();
    let found = registries.search(term);
    if found.is_empty() {
        let _ = writeln!(out, "No packs match `{}`.", visible(term));
    }
    let width = found.iter().map(|f| f.entry.name.as_str().len()).max().unwrap_or(0);
    for f in &found {
        let e = f.entry;
        let _ = writeln!(out, "{:<width$}  {}", e.name.as_str(), visible(e.description.as_str()));
        let indent = " ".repeat(width + 2);
        if !e.tags.is_empty() {
            let tags: Vec<&str> = e.tags.iter().map(|t| t.as_str()).collect();
            let _ = writeln!(out, "{indent}tags: {}", tags.join(", "));
        }
        let _ = writeln!(out, "{indent}source: {}", visible(&e.source.to_string()));
    }
    out.push('\n');
    for r in &registries.loaded {
        let when = if r.age.is_zero() { "fetched just now".to_owned() } else { format!("fetched {}", ago(r.age)) };
        let _ = writeln!(out, "From {} at commit {}, {when}.", visible(&r.source.to_string()), r.commit.short());
    }
    if !found.is_empty() {
        let _ =
            writeln!(out, "Install one with `alias-deck add <name>`. You see everything it adds before it installs.");
    }
    out
}

fn findings_section(out: &mut String, list: &[Finding]) {
    if list.is_empty() {
        let _ = writeln!(out, "\nFindings: none");
        return;
    }
    let _ =
        writeln!(out, "\nFindings. Body rules are text heuristics: they point at code to read, not proof it is safe");
    out.push_str(&findings("  ", list));
}

/// Findings grouped by severity, most severe first.
pub fn findings(prefix: &str, findings: &[Finding]) -> String {
    let mut out = String::new();
    for severity in [Severity::Danger, Severity::Warn, Severity::Info] {
        for f in findings.iter().filter(|f| f.severity == severity) {
            let _ =
                writeln!(out, "{prefix}{:<6} {:<18} {}: {}", severity.key(), f.rule.id(), f.entry, visible(&f.message));
        }
    }
    out
}

/// The changes since the approved version: changed functions show both bodies in full.
pub fn update(old: &PackDocument, new: &Candidate, old_commit: Option<&CommitId>, diff: &PackDiff) -> String {
    let mut out = String::new();
    let (o, n) = (old.pack().meta(), new.doc.pack().meta());
    if o.name == n.name {
        let _ = writeln!(out, "Update {} {} -> {}", n.name.as_str(), o.version, n.version);
    } else {
        let _ =
            writeln!(out, "Update {} {} -> {} {} (renamed)", o.name.as_str(), o.version, n.name.as_str(), n.version);
    }
    let _ = writeln!(out, "  source: {}", visible(&new.source.to_string()));
    if let (Some(a), Some(b)) = (old_commit, &new.commit) {
        let _ = writeln!(out, "  commit: {a} -> {b}");
    }
    let _ = writeln!(out, "  sha256: {}", new.sha256);
    if diff.changes.is_empty() {
        let _ = writeln!(out, "\nNo entry changed; only the pack's metadata or formatting did.");
    } else {
        let _ = writeln!(out, "\nChanges ({})", diff.changes.len());
    }
    for change in &diff.changes {
        match change {
            EntryChange::Added(l) => prefixed(&mut out, "  + ", "    ", &entry_text(l.group, l.entry)),
            EntryChange::Removed(l) => prefixed(&mut out, "  - ", "    ", &entry_text(l.group, l.entry)),
            EntryChange::Changed { old, new } => {
                let _ = writeln!(out, "  ~ {}", new.entry.name());
                prefixed(&mut out, "    old: ", "         ", &entry_text(old.group, old.entry));
                prefixed(&mut out, "    new: ", "         ", &entry_text(new.group, new.entry));
            }
        }
    }
    if diff.new_findings.is_empty() {
        let _ = writeln!(out, "\nNew findings: none");
    } else {
        let _ = writeln!(
            out,
            "\nNew findings. Body rules are text heuristics: they point at code to read, not proof it is safe"
        );
        out.push_str(&findings("  ", &diff.new_findings));
    }
    out
}

fn entry_text(g: &Group, e: &Entry) -> String {
    let mut s = String::new();
    entry(&mut s, "", g, e);
    s
}

fn prefixed(out: &mut String, first: &str, rest: &str, text: &str) {
    for (i, line) in text.lines().enumerate() {
        let _ = writeln!(out, "{}{line}", if i == 0 { first } else { rest });
    }
}

/// Omitted entries once each, naming the shells that left them out, then one line per note
/// kind naming every entry it applies to.
pub fn compile_report(outputs: &[&Compiled]) -> String {
    let mut omitted: Vec<(String, String, Vec<Shell>)> = Vec::new();
    let mut notes: Vec<(Note, Vec<String>)> = Vec::new();
    for c in outputs {
        for r in &c.entries {
            if let Outcome::Omitted(reason) = &r.outcome {
                let (entry, why) = (r.entry.to_string(), reason.to_string());
                match omitted.iter_mut().find(|(e, w, _)| *e == entry && *w == why) {
                    Some((_, _, shells)) => shells.push(c.target.shell),
                    None => omitted.push((entry, why, vec![c.target.shell])),
                }
            }
            for n in &r.notes {
                let entry = r.entry.to_string();
                match notes.iter_mut().find(|(k, _)| k == n) {
                    Some((_, refs)) if refs.contains(&entry) => {}
                    Some((_, refs)) => refs.push(entry),
                    None => notes.push((*n, vec![entry])),
                }
            }
        }
    }
    let mut out = String::new();
    for (entry, why, shells) in omitted {
        if outputs.len() == 1 {
            let _ = writeln!(out, "alias-deck: {entry}: omitted: {why}");
        } else {
            let shells: Vec<&str> = shells.iter().map(|s| s.key()).collect();
            let _ = writeln!(out, "alias-deck: {entry}: omitted in {}: {why}", shells.join(", "));
        }
    }
    for (note, refs) in notes {
        let summary = match note {
            Note::PwshArgRewriting => format!(
                "pwsh rewrites some caller arguments (a bare --, a,b, -Dx.y, wildcards) before {} wrapper function{} see{} them",
                refs.len(),
                if refs.len() == 1 { "" } else { "s" },
                if refs.len() == 1 { "s" } else { "" },
            ),
        };
        let _ = writeln!(out, "alias-deck: note: {summary}: {}", refs.join(", "));
    }
    out
}

pub fn list(packs: &[ListedPack]) -> String {
    let mut out = String::new();
    for (i, p) in packs.iter().enumerate() {
        let n = i + 1;
        let flag = if p.enabled { "" } else { "  (disabled)" };
        let pin = match (&p.locked, p.source.reference()) {
            (Some(l), Some(r)) => format!("ref {r}, commit {}", l.commit.short()),
            (Some(l), None) => format!("commit {}", l.commit.short()),
            (None, _) if matches!(p.source, alias_deck_core::Source::Local(_)) => "local, read live".to_owned(),
            (None, _) => "not approved".to_owned(),
        };
        match &p.pack {
            Ok(doc) => {
                let m = doc.pack().meta();
                let count = |f: fn(&Entry) -> bool| doc.pack().entries().filter(|(_, e)| f(e)).count();
                let counts = [
                    plural(count(|e| matches!(e, Entry::Alias(_))), "alias", "aliases"),
                    plural(count(|e| matches!(e, Entry::Function(_))), "function", "functions"),
                    plural(count(|e| matches!(e, Entry::Env(_))), "env var", "env vars"),
                ];
                let _ = writeln!(
                    out,
                    "{n}. {} {}  {}  {pin}  {}{flag}",
                    m.name.as_str(),
                    m.version,
                    p.source,
                    counts.join(", ")
                );
            }
            Err(e) => {
                let _ =
                    writeln!(out, "{n}. {}  {pin}  cannot load: {}{flag}", p.source, e.to_string().replace('\n', "; "));
            }
        }
    }
    out
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_codes_and_bidi_controls_are_shown_not_obeyed() {
        assert_eq!(visible("ok\tline\nnext"), "ok\tline\nnext");
        assert_eq!(visible("evil\x1b[2K\rsafe"), "evil\\u{1b}[2K\\u{d}safe");
        assert_eq!(visible("a\u{202e}b\u{200b}"), "a\\u{202e}b\\u{200b}");
    }
}
