//! The domain model. Every type here is valid by construction: the only ways in are the
//! validating constructors below and `PackDocument::parse`, so lowering and the backends never
//! re-check names, NUL bytes, or argv shape.

use std::collections::BTreeMap;
use std::fmt;

/// Why a raw value was rejected at the boundary. `PackDocument::parse` attaches a byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidValue {
    ContainsNul,
    Empty,
    BadCommandName,
    BadEnvName,
    BadSlug,
    BadProgram,
    BadShorthand,
    /// `PathList` on a name that does not end in `PATH`.
    PathListName,
}

impl fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InvalidValue::ContainsNul => "contains a NUL byte, which no shell can carry",
            InvalidValue::Empty => "must not be empty",
            InvalidValue::BadCommandName => {
                "command names match [A-Za-z0-9_.][A-Za-z0-9_.+-]{0,63}, are not `.`, and are not all digits"
            }
            InvalidValue::BadEnvName => "env names match [A-Za-z_][A-Za-z0-9_]{0,127}",
            InvalidValue::BadSlug => "names of packs and groups match [a-z0-9][a-z0-9-]{0,63}",
            InvalidValue::BadProgram => "the program must be non-empty, must not start with `-`, and must not contain `=`",
            InvalidValue::BadShorthand => {
                "the string form allows only plain words ([A-Za-z0-9_./:=@%+,-], or `~` and `~/` at the start); use an array"
            }
            InvalidValue::PathListName => "prepend and append are allowed only on names ending in PATH",
        })
    }
}

/// A string with no NUL byte. No shell can carry NUL in argv or env, and each one truncates or
/// errors differently, so the boundary rejects it once.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Text(String);

impl Text {
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidValue> {
        let s = s.into();
        if s.contains('\0') {
            return Err(InvalidValue::ContainsNul);
        }
        Ok(Text(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn matches_grammar(s: &str, first: impl Fn(u8) -> bool, rest: impl Fn(u8) -> bool, max: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= max && first(b[0]) && b[1..].iter().all(|&c| rest(c))
}

/// A command name usable in all four shells. Whether it is reserved in one shell is a compile
/// outcome (`OmitReason::ReservedName`), not a parse error, so one pack stays valid everywhere.
/// The grammar excludes every shell metacharacter, so a backend may print it bare.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandName(String);

impl CommandName {
    pub fn new(s: &str) -> Result<Self, InvalidValue> {
        let ok = matches_grammar(
            s,
            |c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.',
            |c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c),
            64,
        ) && s != "."
            && !s.bytes().all(|c| c.is_ascii_digit());
        if ok {
            Ok(CommandName(s.to_owned()))
        } else {
            Err(InvalidValue::BadCommandName)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Safe bare in `export N=`, `set -gx N`, and `$env:N`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvName(String);

impl EnvName {
    pub fn new(s: &str) -> Result<Self, InvalidValue> {
        let ok = matches_grammar(
            s,
            |c| c.is_ascii_alphabetic() || c == b'_',
            |c| c.is_ascii_alphanumeric() || c == b'_',
            128,
        );
        if ok {
            Ok(EnvName(s.to_owned()))
        } else {
            Err(InvalidValue::BadEnvName)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// fish treats only variables named `*PATH` as lists, so only those take `PathList`.
    pub fn is_path_list(&self) -> bool {
        self.0.ends_with("PATH")
    }
}

/// Names packs and groups. The only free-form-looking field allowed into generated comments.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slug(String);

impl Slug {
    pub fn new(s: &str) -> Result<Self, InvalidValue> {
        let lower = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
        if matches_grammar(s, lower, |c| lower(c) || c == b'-', 64) {
            Ok(Slug(s.to_owned()))
        } else {
            Err(InvalidValue::BadSlug)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One shell word whose meaning is the same in every shell. Never shell syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Word {
    Literal(Text),
    /// `$HOME` at runtime followed by this suffix, which is empty or starts with `/`.
    Home(Text),
}

impl Word {
    /// The format's only expansion rule: exactly `~`, or a leading `~/`, means the home directory.
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        if s == "~" || s.starts_with("~/") {
            Ok(Word::Home(Text::new(&s[1..])?))
        } else {
            Ok(Word::Literal(Text::new(s)?))
        }
    }
}

/// A program plus fixed leading arguments. Aliases are data, not code, so lint can read the
/// program and every shell passes the same argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv {
    program: Word,
    args: Vec<Word>,
}

impl Argv {
    /// bash and zsh would read `A=b` in command position as an assignment, and a leading `-`
    /// would be an option to `command` or `&`.
    pub fn new(program: Word, args: Vec<Word>) -> Result<Self, InvalidValue> {
        if let Word::Literal(t) = &program {
            let p = t.as_str();
            if p.is_empty() || p.starts_with('-') || p.contains('=') {
                return Err(InvalidValue::BadProgram);
            }
        }
        Ok(Argv { program, args })
    }

    /// The string form `"git status -sb"`: plain words split on spaces. Anything a shell would
    /// read differently needs the array form.
    pub fn parse_shorthand(s: &str) -> Result<Self, InvalidValue> {
        let plain = |c: char| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c);
        let mut words = Vec::new();
        for w in s.split(' ').filter(|w| !w.is_empty()) {
            let tail = if w == "~" || w.starts_with("~/") { &w[1..] } else { w };
            if !tail.chars().all(plain) {
                return Err(InvalidValue::BadShorthand);
            }
            words.push(Word::parse(w)?);
        }
        let mut words = words.into_iter();
        let program = words.next().ok_or(InvalidValue::Empty)?;
        Argv::new(program, words.collect())
    }

    pub fn program(&self) -> &Word {
        &self.program
    }

    pub fn args(&self) -> &[Word] {
        &self.args
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: CommandName,
    pub argv: Argv,
}

/// The languages an in-shell body can be written in. `ALL` order is the format's key order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dialect {
    Posix,
    Bash,
    Zsh,
    Fish,
    Pwsh,
}

impl Dialect {
    pub const ALL: [Dialect; 5] = [Dialect::Posix, Dialect::Bash, Dialect::Zsh, Dialect::Fish, Dialect::Pwsh];

    /// The `shell.<key>` name in a pack. The parser derives its accepted keys from this.
    pub fn key(self) -> &'static str {
        match self {
            Dialect::Posix => "posix",
            Dialect::Bash => "bash",
            Dialect::Zsh => "zsh",
            Dialect::Fish => "fish",
            Dialect::Pwsh => "pwsh",
        }
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// At least one body. Which one a shell runs is that shell's dialect preference list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InShell(BTreeMap<Dialect, Text>);

impl InShell {
    pub fn new(bodies: BTreeMap<Dialect, Text>) -> Result<Self, InvalidValue> {
        if bodies.is_empty() {
            return Err(InvalidValue::Empty);
        }
        Ok(InShell(bodies))
    }

    pub fn get(&self, d: Dialect) -> Option<&Text> {
        self.0.get(&d)
    }

    pub fn dialects(&self) -> impl Iterator<Item = Dialect> + '_ {
        self.0.keys().copied()
    }
}

/// The two execution models. The author picks by what the function needs, not by shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionBody {
    /// POSIX sh, run as `/bin/sh -c BODY NAME ARGS...` from every shell. It cannot change the
    /// caller's directory or variables.
    Script(Text),
    /// Runs inside the user's shell, so it can `cd` or export.
    InShell(InShell),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: CommandName,
    pub description: Option<Text>,
    pub body: FunctionBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    Set(Word),
    /// Ensure-present: an entry already in the list stays where it is, so sourcing twice is a no-op.
    PathList {
        prepend: Vec<Word>,
        append: Vec<Word>,
    },
    /// The pack names a need. The value comes from the user's `Bindings` and never appears in a
    /// pack, the model, or the compiled file. `purpose` is shown when the user is asked to bind it.
    Secret {
        purpose: Text,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvEntry {
    name: EnvName,
    value: EnvValue,
}

impl EnvEntry {
    pub fn new(name: EnvName, value: EnvValue) -> Result<Self, InvalidValue> {
        if let EnvValue::PathList { prepend, append } = &value {
            if !name.is_path_list() {
                return Err(InvalidValue::PathListName);
            }
            // An empty element means the current directory to every shell that reads PATH.
            if prepend.iter().chain(append).any(|w| matches!(w, Word::Literal(t) if t.as_str().is_empty())) {
                return Err(InvalidValue::Empty);
            }
        }
        Ok(EnvEntry { name, value })
    }

    pub fn name(&self) -> &EnvName {
        &self.name
    }

    pub fn value(&self) -> &EnvValue {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Alias(Alias),
    Function(Function),
    Env(EnvEntry),
}

impl Entry {
    pub fn name(&self) -> EntryName {
        match self {
            Entry::Alias(a) => EntryName::Command(a.name.clone()),
            Entry::Function(f) => EntryName::Command(f.name.clone()),
            Entry::Env(e) => EntryName::Env(e.name.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

impl Os {
    pub const ALL: [Os; 3] = [Os::MacOs, Os::Linux, Os::Windows];

    pub fn key(self) -> &'static str {
        match self {
            Os::MacOs => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        }
    }

    pub fn from_key(s: &str) -> Option<Os> {
        Os::ALL.into_iter().find(|o| o.key() == s)
    }

    fn bit(self) -> u8 {
        match self {
            Os::MacOs => 1,
            Os::Linux => 2,
            Os::Windows => 4,
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OsSet(u8);

impl OsSet {
    pub const ALL: OsSet = OsSet(0b111);
    pub const NONE: OsSet = OsSet(0);

    pub fn contains(self, os: Os) -> bool {
        self.0 & os.bit() != 0
    }

    pub fn with(self, os: Os) -> OsSet {
        OsSet(self.0 | os.bit())
    }

    pub fn intersect(self, other: OsSet) -> OsSet {
        OsSet(self.0 & other.0)
    }

    pub fn iter(self) -> impl Iterator<Item = Os> {
        Os::ALL.into_iter().filter(move |o| self.contains(*o))
    }
}

impl fmt::Display for OsSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<&str> = self.iter().map(Os::key).collect();
        if keys.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&keys.join(", "))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMeta {
    pub name: Slug,
    pub version: semver::Version,
    pub license: Text,
    pub description: Text,
    /// Commands the pack expects on PATH. Metadata only; compile ignores it.
    pub requires: Vec<CommandName>,
    pub os: OsSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: Slug,
    pub description: Option<Text>,
    /// Already intersected with the pack's set.
    pub os: OsSet,
    /// env, then aliases, then functions, each in file order.
    pub entries: Vec<Entry>,
}

/// A parsed pack. Fields are private: a `Pack` only comes from `PackDocument`, so no caller holds
/// a model that disagrees with the file a GUI would write back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    meta: PackMeta,
    groups: Vec<Group>,
}

impl Pack {
    pub(crate) fn from_parts(meta: PackMeta, groups: Vec<Group>) -> Self {
        Pack { meta, groups }
    }

    pub fn meta(&self) -> &PackMeta {
        &self.meta
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Group, &Entry)> {
        self.groups.iter().flat_map(|g| g.entries.iter().map(move |e| (g, e)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryName {
    Command(CommandName),
    Env(EnvName),
}

impl fmt::Display for EntryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntryName::Command(n) => f.write_str(n.as_str()),
            EntryName::Env(n) => f.write_str(n.as_str()),
        }
    }
}

/// Stable address of one entry across parse, compile, and lint: `devkit/git/gs`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryRef {
    pub pack: Slug,
    pub group: Slug,
    pub name: EntryName,
}

impl fmt::Display for EntryRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.pack.as_str(), self.group.as_str(), self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_follow_the_portable_grammar() {
        for ok in ["gs", "..", "a.b", "a+b", "git-up", "_x", "1x", ".x"] {
            assert!(CommandName::new(ok).is_ok(), "{ok:?} should be accepted");
        }
        let long = "a".repeat(65);
        for bad in ["", ".", "123", "-x", "+x", "a b", "a/b", "a=b", "a,b", "@x", "é", long.as_str()] {
            assert_eq!(CommandName::new(bad), Err(InvalidValue::BadCommandName), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn env_names_and_slugs() {
        assert!(EnvName::new("_A1").is_ok());
        assert!(EnvName::new("1A").is_err());
        assert!(EnvName::new("A-B").is_err());
        assert!(Slug::new("dev-kit2").is_ok());
        assert!(Slug::new("Dev").is_err());
        assert!(Slug::new("-x").is_err());
    }

    #[test]
    fn text_rejects_nul() {
        assert_eq!(Text::new("a\0b"), Err(InvalidValue::ContainsNul));
    }

    #[test]
    fn tilde_is_home_only_alone_or_before_a_slash() {
        let t = |s: &str| Text::new(s).unwrap();
        assert_eq!(Word::parse("~").unwrap(), Word::Home(t("")));
        assert_eq!(Word::parse("~/").unwrap(), Word::Home(t("/")));
        assert_eq!(Word::parse("~/go").unwrap(), Word::Home(t("/go")));
        assert_eq!(Word::parse("~root").unwrap(), Word::Literal(t("~root")));
        assert_eq!(Word::parse("a~/b").unwrap(), Word::Literal(t("a~/b")));
    }

    #[test]
    fn argv_program_shape() {
        let lit = |s: &str| Word::parse(s).unwrap();
        assert!(Argv::new(lit("git"), vec![]).is_ok());
        assert!(Argv::new(lit("~/bin/x"), vec![]).is_ok());
        for bad in ["", "-x", "A=b"] {
            assert_eq!(Argv::new(lit(bad), vec![]), Err(InvalidValue::BadProgram), "program {bad:?}");
        }
    }

    #[test]
    fn shorthand_allows_only_plain_words() {
        let a = Argv::parse_shorthand("git  log --pretty=%h a,b x@y ~/d").unwrap();
        assert_eq!(a.program(), &Word::parse("git").unwrap());
        assert_eq!(a.args().len(), 5);
        assert_eq!(a.args()[4], Word::Home(Text::new("/d").unwrap()));
        for bad in ["echo $HOME", "a|b", "it's", "x ~root", "a\tb", "x *"] {
            assert_eq!(Argv::parse_shorthand(bad), Err(InvalidValue::BadShorthand), "{bad:?}");
        }
        assert_eq!(Argv::parse_shorthand("  "), Err(InvalidValue::Empty));
    }

    #[test]
    fn path_lists_only_on_path_names_and_never_empty() {
        let list = |w: &str| EnvValue::PathList { prepend: vec![Word::parse(w).unwrap()], append: vec![] };
        assert!(EnvEntry::new(EnvName::new("MANPATH").unwrap(), list("/x")).is_ok());
        assert_eq!(EnvEntry::new(EnvName::new("EDITOR").unwrap(), list("/x")), Err(InvalidValue::PathListName));
        assert_eq!(EnvEntry::new(EnvName::new("PATH").unwrap(), list("")), Err(InvalidValue::Empty));
    }
}
