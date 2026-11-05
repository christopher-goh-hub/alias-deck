//! Deck to shell source. Pure, total, and deterministic: no I/O, and the same inputs give the
//! same bytes.
//!
//! Policy lives here once, in `lower`. Syntax lives in `shells::*`. A backend never decides
//! whether an entry applies; it renders an `Item` that `lower` already accepted.

use std::collections::HashMap;
use std::fmt;

use crate::model::{
    Argv, CommandName, Dialect, Entry, EntryName, EntryRef, EnvName, EnvValue, FunctionBody, Os, OsSet, Pack, Text,
    Word,
};
use crate::secrets::Bindings;
use crate::shells::{self, Backend};
use crate::writer::Writer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shell {
    /// bash 3.2 through 5.x: one output, written to the 3.2 subset.
    Bash,
    Zsh,
    Fish,
    /// PowerShell 7.
    Pwsh,
}

impl Shell {
    pub const ALL: [Shell; 4] = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Pwsh];

    pub fn key(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Pwsh => "pwsh",
        }
    }

    pub fn from_key(s: &str) -> Option<Shell> {
        Shell::ALL.into_iter().find(|sh| sh.key() == s)
    }

    /// The conventional extension of a file this shell sources.
    pub fn extension(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Pwsh => "ps1",
        }
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub shell: Shell,
    pub os: Os,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FishAliasStyle {
    /// `function gs --wraps '...'`: the same semantics as the other shells.
    #[default]
    Function,
    /// `abbr --add gs '...'`: expands on the command line.
    Abbr,
}

/// User preferences, never pack fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompileOptions {
    pub fish_aliases: FishAliasStyle,
}

/// The packs a user has active, in precedence order. A later pack wins a name clash.
#[derive(Debug, Clone, Default)]
pub struct Deck {
    packs: Vec<Pack>,
}

impl Deck {
    pub fn new(packs: Vec<Pack>) -> Deck {
        Deck { packs }
    }

    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    pub target: Target,
    /// The file the user's shell sources. It holds no secret value and no pack text outside a
    /// quoted literal.
    pub source: String,
    /// One report per deck entry, in deck order, built before rendering. An entry missing from
    /// `source` is `Omitted` by construction.
    pub entries: Vec<EntryReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryReport {
    pub entry: EntryRef,
    pub outcome: Outcome,
    pub notes: Vec<Note>,
}

impl fmt::Display for EntryReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.outcome {
            Outcome::Emitted => write!(f, "{}: emitted", self.entry),
            Outcome::Omitted(r) => write!(f, "{}: omitted: {r}", self.entry),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Emitted,
    Omitted(OmitReason),
}

impl Outcome {
    pub fn is_omitted(&self) -> bool {
        matches!(self, Outcome::Omitted(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmitReason {
    OsCondition {
        allowed: OsSet,
    },
    /// A later entry with the same name wins.
    ShadowedBy(EntryRef),
    ReservedName {
        shell: Shell,
    },
    NoBodyFor {
        shell: Shell,
        available: Vec<Dialect>,
    },
    NeedsPosixSh,
    /// The pack needs this secret and the user has not bound it. A to-do, not a failure.
    SecretUnbound,
    /// The user's binding has no lookup template on this OS.
    NoSecretLookup {
        os: Os,
    },
    SeparatorInPathEntry {
        separator: char,
    },
}

impl fmt::Display for OmitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OmitReason::OsCondition { allowed } => write!(f, "only for {allowed}"),
            OmitReason::ShadowedBy(r) => write!(f, "shadowed by {r}"),
            OmitReason::ReservedName { shell } => write!(f, "the name is reserved in {shell}"),
            OmitReason::NoBodyFor { shell, available } => {
                let keys: Vec<&str> = available.iter().map(|d| d.key()).collect();
                write!(f, "no body for {shell} (pack has: {})", keys.join(", "))
            }
            OmitReason::NeedsPosixSh => f.write_str("script functions need /bin/sh, which Windows lacks"),
            OmitReason::SecretUnbound => f.write_str("secret is not bound; add it to your secrets file"),
            OmitReason::NoSecretLookup { os } => write!(f, "the bound secret source has no lookup on {os}"),
            OmitReason::SeparatorInPathEntry { separator } => {
                write!(f, "a path entry contains the list separator `{separator}`")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// pwsh parses the caller's words before a wrapper function sees them: a bare `--` is
    /// dropped, `a,b` becomes two arguments, `-Dx.y` splits, and wildcards glob.
    PwshArgRewriting,
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::PwshArgRewriting => f.write_str(
                "pwsh rewrites some caller arguments (a bare --, a,b, -Dx.y, wildcards) before this wrapper sees them",
            ),
        }
    }
}

/// One renderable unit with every policy decision already made.
pub(crate) enum Item<'a> {
    Secret {
        name: &'a EnvName,
        lookup: Argv,
    },
    SetEnv {
        name: &'a EnvName,
        value: &'a Word,
    },
    PathList {
        name: &'a EnvName,
        prepend: &'a [Word],
        append: &'a [Word],
    },
    /// `self_ref` is `grep = "grep --color=auto"`: fish and pwsh must call the program, not recurse.
    Alias {
        name: &'a CommandName,
        argv: &'a Argv,
        self_ref: bool,
    },
    ScriptFn {
        name: &'a CommandName,
        body: &'a Text,
        description: Option<&'a Text>,
    },
    InShellFn {
        name: &'a CommandName,
        dialect: Dialect,
        code: &'a Text,
        description: Option<&'a Text>,
    },
}

impl Item<'_> {
    /// Secrets run before any pack PATH edit could shadow the program a lookup calls.
    fn phase(&self) -> usize {
        match self {
            Item::Secret { .. } => 0,
            Item::SetEnv { .. } | Item::PathList { .. } => 1,
            Item::Alias { .. } | Item::ScriptFn { .. } | Item::InShellFn { .. } => 2,
        }
    }
}

pub fn compile(deck: &Deck, bindings: &Bindings, target: Target, options: &CompileOptions) -> Compiled {
    let backend = shells::backend(target.shell);
    let (entries, mut items) = lower(deck, bindings, target, backend);
    items.sort_by_key(Item::phase);

    let mut w = Writer::new(backend.quoting());
    let metas: Vec<_> = deck.packs().iter().map(Pack::meta).collect();
    backend.header(&mut w, target, &metas);
    let mut phase = None;
    for item in &items {
        if phase != Some(item.phase()) {
            phase = Some(item.phase());
            w.code("\n");
        }
        backend.render(&mut w, item, options);
    }
    Compiled { target, source: w.finish(), entries }
}

/// Decides every entry once: the OS gate, shadowing, reserved names, dialect choice,
/// self-reference, and secret binding.
fn lower<'d>(
    deck: &'d Deck,
    bindings: &Bindings,
    target: Target,
    backend: &dyn Backend,
) -> (Vec<EntryReport>, Vec<Item<'d>>) {
    let all: Vec<(&Pack, &crate::model::Group, &Entry)> =
        deck.packs().iter().flat_map(|p| p.entries().map(move |(g, e)| (p, g, e))).collect();
    let refer = |p: &Pack, g: &crate::model::Group, e: &Entry| EntryRef {
        pack: p.meta().name.clone(),
        group: g.name.clone(),
        name: e.name(),
    };
    // PATH lists compose across packs, so they never shadow or get shadowed.
    let shadow_key = |e: &Entry| match e {
        Entry::Env(env) if matches!(env.value(), EnvValue::PathList { .. }) => None,
        Entry::Env(env) => Some(format!("env:{}", env.name().as_str())),
        Entry::Alias(_) | Entry::Function(_) => match e.name() {
            EntryName::Command(n) => Some(format!("cmd:{}", backend.command_key(&n))),
            EntryName::Env(_) => None,
        },
    };
    let mut winner: HashMap<String, usize> = HashMap::new();
    for (i, (_, g, e)) in all.iter().enumerate() {
        if g.os.contains(target.os) {
            if let Some(k) = shadow_key(e) {
                winner.insert(k, i);
            }
        }
    }

    let mut reports = Vec::with_capacity(all.len());
    let mut items = Vec::new();
    for (i, &(p, g, e)) in all.iter().enumerate() {
        let decided = if !g.os.contains(target.os) {
            Err(OmitReason::OsCondition { allowed: g.os })
        } else if let Some(&w) = shadow_key(e).and_then(|k| winner.get(&k)).filter(|&&w| w != i) {
            let (wp, wg, we) = all[w];
            Err(OmitReason::ShadowedBy(refer(wp, wg, we)))
        } else {
            lower_entry(e, bindings, target, backend)
        };
        let (outcome, notes) = match decided {
            Ok(item) => {
                let notes = backend.notes(&item);
                items.push(item);
                (Outcome::Emitted, notes)
            }
            Err(reason) => (Outcome::Omitted(reason), Vec::new()),
        };
        reports.push(EntryReport { entry: refer(p, g, e), outcome, notes });
    }
    (reports, items)
}

fn lower_entry<'d>(
    e: &'d Entry,
    bindings: &Bindings,
    target: Target,
    backend: &dyn Backend,
) -> Result<Item<'d>, OmitReason> {
    let reserved = |name: &CommandName| {
        if backend.is_reserved(name) {
            Err(OmitReason::ReservedName { shell: target.shell })
        } else {
            Ok(())
        }
    };
    match e {
        Entry::Env(env) => {
            let name = env.name();
            match env.value() {
                EnvValue::Set(value) => Ok(Item::SetEnv { name, value }),
                EnvValue::PathList { prepend, append } => {
                    let separator = backend.list_separator(target.os);
                    let has_sep = |w: &Word| match w {
                        Word::Literal(t) | Word::Home(t) => t.as_str().contains(separator),
                    };
                    if prepend.iter().chain(append).any(has_sep) {
                        return Err(OmitReason::SeparatorInPathEntry { separator });
                    }
                    Ok(Item::PathList { name, prepend, append })
                }
                EnvValue::Secret { .. } => {
                    let source = bindings.get(name).ok_or(OmitReason::SecretUnbound)?;
                    let lookup = source.lookup(target.os).ok_or(OmitReason::NoSecretLookup { os: target.os })?;
                    Ok(Item::Secret { name, lookup })
                }
            }
        }
        Entry::Alias(a) => {
            reserved(&a.name)?;
            let self_ref = matches!(a.argv.program(), Word::Literal(t) if t.as_str() == a.name.as_str());
            Ok(Item::Alias { name: &a.name, argv: &a.argv, self_ref })
        }
        Entry::Function(f) => {
            reserved(&f.name)?;
            let description = f.description.as_ref();
            match &f.body {
                FunctionBody::Script(_) if target.os == Os::Windows => Err(OmitReason::NeedsPosixSh),
                FunctionBody::Script(body) => Ok(Item::ScriptFn { name: &f.name, body, description }),
                FunctionBody::InShell(bodies) => {
                    let found = backend.dialects().iter().find_map(|&d| bodies.get(d).map(|code| (d, code)));
                    let (dialect, code) = found.ok_or_else(|| OmitReason::NoBodyFor {
                        shell: target.shell,
                        available: bodies.dialects().collect(),
                    })?;
                    Ok(Item::InShellFn { name: &f.name, dialect, code, description })
                }
            }
        }
    }
}
