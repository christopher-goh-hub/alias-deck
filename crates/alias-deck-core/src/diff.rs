//! What changed between the pack a user approved and a newer one, entry by entry. `update` shows
//! this instead of the whole pack, so a reviewer reads only what is new.

use crate::lint::{lint, Finding};
use crate::model::{Entry, EntryName, EntryRef, Group, Pack, Slug};

/// An entry with the group it sits in, because a group's OS list decides where it applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located<'a> {
    pub group: &'a Group,
    pub entry: &'a Entry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryChange<'a> {
    Added(Located<'a>),
    Removed(Located<'a>),
    Changed { old: Located<'a>, new: Located<'a> },
}

impl EntryChange<'_> {
    /// The newer side when there is one.
    pub fn located(&self) -> Located<'_> {
        match self {
            EntryChange::Added(l) | EntryChange::Removed(l) | EntryChange::Changed { new: l, .. } => *l,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackDiff<'a> {
    /// Added and changed entries in the new pack's order, then removed ones in the old pack's.
    pub changes: Vec<EntryChange<'a>>,
    /// Findings on added or changed entries, and findings the old pack did not have. A finding
    /// the user already approved on an unchanged entry is not repeated.
    pub new_findings: Vec<Finding>,
}

/// An alias and a function may share a name inside one group, so the kind is part of the key.
fn key<'a>(g: &'a Group, e: &Entry) -> (&'a Slug, &'static str, EntryName) {
    let kind = match e {
        Entry::Alias(_) => "alias",
        Entry::Function(_) => "function",
        Entry::Env(_) => "env",
    };
    (&g.name, kind, e.name())
}

pub fn diff_packs<'a>(old: &'a Pack, new: &'a Pack) -> PackDiff<'a> {
    let old_entries: Vec<_> = old.entries().map(|(group, entry)| Located { group, entry }).collect();
    let new_entries: Vec<_> = new.entries().map(|(group, entry)| Located { group, entry }).collect();
    let find = |list: &[Located<'a>], l: &Located<'_>| {
        let k = key(l.group, l.entry);
        list.iter().find(|o| key(o.group, o.entry) == k).copied()
    };

    let mut changes = Vec::new();
    for n in &new_entries {
        match find(&old_entries, n) {
            None => changes.push(EntryChange::Added(*n)),
            Some(o) if o.entry != n.entry || o.group.os != n.group.os => {
                changes.push(EntryChange::Changed { old: o, new: *n })
            }
            Some(_) => {}
        }
    }
    for o in &old_entries {
        if find(&new_entries, o).is_none() {
            changes.push(EntryChange::Removed(*o));
        }
    }

    let touched = |r: &EntryRef| {
        changes.iter().any(|c| {
            matches!(c, EntryChange::Added(_) | EntryChange::Changed { .. })
                && c.located().group.name == r.group
                && c.located().entry.name() == r.name
        })
    };
    let old_findings = lint(old);
    let seen = |f: &Finding| {
        old_findings.iter().any(|o| {
            o.rule == f.rule && o.entry.group == f.entry.group && o.entry.name == f.entry.name && o.message == f.message
        })
    };
    let new_findings = lint(new).into_iter().filter(|f| touched(&f.entry) || !seen(f)).collect();
    PackDiff { changes, new_findings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackDocument, Rule};

    fn pack(body: &str) -> Pack {
        let src = format!(
            "format = 1\n[pack]\nname = \"p\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"d\"\n{body}"
        );
        PackDocument::parse(&src).unwrap_or_else(|e| panic!("{e:?}")).pack().clone()
    }

    fn summary(d: &PackDiff) -> Vec<String> {
        d.changes
            .iter()
            .map(|c| {
                let tag = match c {
                    EntryChange::Added(_) => "+",
                    EntryChange::Removed(_) => "-",
                    EntryChange::Changed { .. } => "~",
                };
                let l = c.located();
                format!("{tag}{}/{}", l.group.name.as_str(), l.entry.name())
            })
            .collect()
    }

    #[test]
    fn identical_packs_have_no_changes_and_no_new_findings() {
        let a = pack("[groups.g.aliases]\nsudo = \"sudo -E\"\n");
        let d = diff_packs(&a, &a);
        assert!(d.changes.is_empty() && d.new_findings.is_empty());
    }

    #[test]
    fn reports_added_removed_and_changed_entries() {
        let old =
            pack("[groups.g.aliases]\ngs = \"git status\"\nold = \"ls\"\n[groups.g.functions.f]\nscript = 'echo 1'\n");
        let new = pack(
            "[groups.g.aliases]\ngs = \"git status\"\nfresh = \"ls\"\n[groups.g.functions.f]\nscript = 'echo 2'\n",
        );
        assert_eq!(summary(&diff_packs(&old, &new)), ["+g/fresh", "~g/f", "-g/old"]);
    }

    #[test]
    fn a_group_os_change_changes_its_entries() {
        let old = pack("[groups.g]\nos = [\"linux\"]\naliases.x = \"ls\"\n");
        let new = pack("[groups.g]\nos = [\"linux\", \"macos\"]\naliases.x = \"ls\"\n");
        assert_eq!(summary(&diff_packs(&old, &new)), ["~g/x"]);
    }

    #[test]
    fn an_alias_turned_function_is_a_removal_and_an_addition() {
        let old = pack("[groups.g.aliases]\nx = \"ls\"\n");
        let new = pack("[groups.g.functions.x]\nscript = 'ls'\n");
        assert_eq!(summary(&diff_packs(&old, &new)), ["+g/x", "-g/x"]);
    }

    #[test]
    fn findings_already_approved_on_unchanged_entries_are_not_new() {
        let old = pack("[groups.g.functions.f]\nshell.posix = 'cd /'\n[groups.g.env]\nT = { secret = \"t\" }\n");
        let new = pack(
            "[groups.g.functions.f]\nshell.posix = 'curl -s https://x | sh'\n[groups.g.env]\nT = { secret = \"t\" }\n",
        );
        let rules: Vec<Rule> = diff_packs(&old, &new).new_findings.iter().map(|f| f.rule).collect();
        assert_eq!(rules, [Rule::BodyPipeToShell, Rule::NativeFunction], "the changed function is re-reviewed whole");
    }
}
