//! The parse boundary. `PackDocument` owns the TOML document and the `Pack` derived from it, so
//! there is one source of truth: a later editor changes the document and re-derives the pack.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use toml_edit::{ImDocument, Item, TableLike};

use crate::model::{
    Alias, Argv, CommandName, Dialect, Entry, EnvEntry, EnvName, EnvValue, Function, FunctionBody, Group, InShell,
    InvalidValue, Os, OsSet, Pack, PackMeta, Slug, Text, Word,
};
use crate::source::SourceError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Byte range in the source, for CLI carets and GUI underlines.
    pub span: Range<usize>,
    pub kind: ParseErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    Toml(String),
    UnsupportedFormat,
    MissingKey(&'static str),
    UnknownKey(String),
    WrongType {
        expected: &'static str,
    },
    Invalid(InvalidValue),
    BadVersion(String),
    UnknownOs(String),
    /// A function needs exactly one of `script` or `shell.*`.
    FunctionBodyShape,
    /// A secret binding needs exactly one of `keychain`, `file`, or `command`.
    SecretSourceShape,
    /// A `file` binding must be absolute or start with `~/`, so it never depends on the cwd.
    RelativeSecretPath,
    BadSource(SourceError),
    /// The manifest lists one pack twice.
    DuplicateSource(String),
    /// A registry index lists one pack name twice.
    DuplicateName(String),
    BadDigest {
        expected: &'static str,
    },
}

impl ParseError {
    /// 1-based line and column of the span start, for `file:line:col` messages.
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        let before = &src[..self.span.start.min(src.len())];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, col)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ParseErrorKind::Toml(m) => write!(f, "invalid TOML: {}", m.trim_end()),
            ParseErrorKind::UnsupportedFormat => f.write_str("expected `format = 1`"),
            ParseErrorKind::MissingKey(k) => write!(f, "missing key `{k}`"),
            ParseErrorKind::UnknownKey(k) => write!(f, "unknown key `{k}`"),
            ParseErrorKind::WrongType { expected } => write!(f, "expected {expected}"),
            ParseErrorKind::Invalid(why) => write!(f, "invalid value: {why}"),
            ParseErrorKind::BadVersion(m) => write!(f, "invalid semver version: {m}"),
            ParseErrorKind::UnknownOs(o) => write!(f, "unknown os `{o}` (expected macos, linux, or windows)"),
            ParseErrorKind::FunctionBodyShape => f.write_str("a function needs exactly one of `script` or `shell.*`"),
            ParseErrorKind::SecretSourceShape => {
                f.write_str("a binding needs exactly one of `keychain`, `file`, or `command`")
            }
            ParseErrorKind::RelativeSecretPath => f.write_str("a secret file path must be absolute or start with `~/`"),
            ParseErrorKind::BadSource(why) => write!(f, "invalid source: {why}"),
            ParseErrorKind::DuplicateSource(s) | ParseErrorKind::DuplicateName(s) => write!(f, "`{s}` is listed twice"),
            ParseErrorKind::BadDigest { expected } => write!(f, "expected {expected}"),
        }
    }
}

/// Collects every error in one pass, so a GUI can mark them all at once.
pub(crate) struct Cx {
    pub errors: Vec<ParseError>,
}

impl Cx {
    pub fn new() -> Cx {
        Cx { errors: Vec::new() }
    }

    pub fn error(&mut self, span: Range<usize>, kind: ParseErrorKind) {
        self.errors.push(ParseError { span, kind });
    }

    pub fn finish<T>(self, value: Option<T>) -> Result<T, Vec<ParseError>> {
        match value {
            Some(v) if self.errors.is_empty() => Ok(v),
            _ => Err(self.errors),
        }
    }

    pub fn parse_toml(src: &str) -> Result<ImDocument<&str>, Vec<ParseError>> {
        ImDocument::parse(src).map_err(|e| {
            vec![ParseError { span: e.span().unwrap_or(0..0), kind: ParseErrorKind::Toml(e.message().to_owned()) }]
        })
    }

    pub fn table<'a>(&mut self, item: &'a Item, at: &Range<usize>) -> Option<&'a dyn TableLike> {
        let t = item.as_table_like();
        if t.is_none() {
            self.error(span(item, at), ParseErrorKind::WrongType { expected: "a table" });
        }
        t
    }

    /// Reports every key not in `allowed`, at the key's own span.
    pub fn only_keys(&mut self, t: &dyn TableLike, allowed: &[&str], at: &Range<usize>) {
        for (k, _) in t.iter() {
            if !allowed.contains(&k) {
                let at = t.key(k).and_then(|k| k.span()).unwrap_or_else(|| at.clone());
                self.error(at, ParseErrorKind::UnknownKey(k.to_owned()));
            }
        }
    }

    pub fn str<'a>(&mut self, item: &'a Item, at: &Range<usize>) -> Option<&'a str> {
        let s = item.as_str();
        if s.is_none() {
            self.error(span(item, at), ParseErrorKind::WrongType { expected: "a string" });
        }
        s
    }

    pub fn text(&mut self, item: &Item, at: &Range<usize>) -> Option<Text> {
        let s = self.str(item, at)?;
        self.valid(Text::new(s), item, at)
    }

    pub fn strings<'a>(&mut self, item: &'a Item, at: &Range<usize>) -> Option<Vec<(&'a str, Range<usize>)>> {
        let at = span(item, at);
        let Some(arr) = item.as_array() else {
            self.error(at, ParseErrorKind::WrongType { expected: "an array of strings" });
            return None;
        };
        let mut out = Vec::new();
        for v in arr.iter() {
            let vs = v.span().unwrap_or_else(|| at.clone());
            match v.as_str() {
                Some(s) => out.push((s, vs)),
                None => self.error(vs, ParseErrorKind::WrongType { expected: "a string" }),
            }
        }
        (out.len() == arr.len()).then_some(out)
    }

    pub fn words(&mut self, item: &Item, at: &Range<usize>) -> Option<Vec<Word>> {
        let strings = self.strings(item, at)?;
        let mut words = Vec::new();
        for (s, sp) in strings {
            match Word::parse(s) {
                Ok(w) => words.push(w),
                Err(e) => self.error(sp, ParseErrorKind::Invalid(e)),
            }
        }
        Some(words)
    }

    /// The array form of an argv. The first word is the program.
    pub fn argv(&mut self, item: &Item, at: &Range<usize>) -> Option<Argv> {
        let mut words = self.words(item, at)?.into_iter();
        let Some(program) = words.next() else {
            self.error(span(item, at), ParseErrorKind::Invalid(InvalidValue::Empty));
            return None;
        };
        self.valid(Argv::new(program, words.collect()), item, at)
    }

    /// Every file alias-deck reads starts with `format = 1`, so a later format fails loudly.
    pub fn format_one(&mut self, root: &dyn TableLike, whole: &Range<usize>) {
        match root.get("format") {
            Some(f) if f.as_integer() == Some(1) => {}
            Some(f) => self.error(span(f, whole), ParseErrorKind::UnsupportedFormat),
            None => self.error(0..0, ParseErrorKind::UnsupportedFormat),
        }
    }

    pub fn valid<T>(&mut self, r: Result<T, InvalidValue>, item: &Item, at: &Range<usize>) -> Option<T> {
        r.map_err(|e| self.error(span(item, at), ParseErrorKind::Invalid(e))).ok()
    }

    pub fn key_valid<T>(
        &mut self,
        r: Result<T, InvalidValue>,
        t: &dyn TableLike,
        key: &str,
        at: &Range<usize>,
    ) -> Option<T> {
        let at = t.key(key).and_then(|k| k.span()).unwrap_or_else(|| at.clone());
        r.map_err(|e| self.error(at, ParseErrorKind::Invalid(e))).ok()
    }
}

pub(crate) fn span(item: &Item, fallback: &Range<usize>) -> Range<usize> {
    item.span().unwrap_or_else(|| fallback.clone())
}

pub struct PackDocument {
    doc: toml_edit::DocumentMut,
    pack: Pack,
}

impl PackDocument {
    /// `Err` holds every problem found, each with a byte span.
    pub fn parse(src: &str) -> Result<Self, Vec<ParseError>> {
        let doc = Cx::parse_toml(src)?;
        let mut cx = Cx::new();
        let pack = parse_pack(&mut cx, doc.as_table(), &(0..src.len()));
        let pack = cx.finish(pack)?;
        Ok(PackDocument { doc: doc.into_mut(), pack })
    }

    pub fn pack(&self) -> &Pack {
        &self.pack
    }

    /// Byte-identical to the parsed source.
    pub fn render(&self) -> String {
        self.doc.to_string()
    }
}

fn parse_pack(cx: &mut Cx, root: &dyn TableLike, whole: &Range<usize>) -> Option<Pack> {
    cx.only_keys(root, &["format", "pack", "groups"], whole);
    cx.format_one(root, whole);
    let meta = match root.get("pack") {
        Some(item) => cx.table(item, whole).and_then(|t| parse_meta(cx, t, &span(item, whole))),
        None => {
            cx.error(0..0, ParseErrorKind::MissingKey("pack"));
            None
        }
    };
    let pack_os = meta.as_ref().map_or(OsSet::ALL, |m: &PackMeta| m.os);
    let mut groups = Vec::new();
    if let Some(item) = root.get("groups") {
        if let Some(t) = cx.table(item, whole) {
            for (name, g) in t.iter() {
                let at = span(g, whole);
                let name = cx.key_valid(Slug::new(name), t, name, &at);
                let group = cx.table(g, &at).map(|gt| parse_group(cx, gt, pack_os, &at));
                if let (Some(name), Some(Some((description, os, entries)))) = (name, group) {
                    groups.push(Group { name, description, os, entries });
                }
            }
        }
    }
    Some(Pack::from_parts(meta?, groups))
}

fn parse_meta(cx: &mut Cx, t: &dyn TableLike, at: &Range<usize>) -> Option<PackMeta> {
    cx.only_keys(t, &["name", "version", "license", "description", "requires", "os"], at);
    let required = |cx: &mut Cx, key: &'static str| {
        let v = t.get(key);
        if v.is_none() {
            cx.error(at.clone(), ParseErrorKind::MissingKey(key));
        }
        v
    };
    let name = required(cx, "name").and_then(|i| {
        let s = cx.str(i, at)?;
        cx.valid(Slug::new(s), i, at)
    });
    let version = required(cx, "version").and_then(|i| {
        let s = cx.str(i, at)?;
        semver::Version::parse(s).map_err(|e| cx.error(span(i, at), ParseErrorKind::BadVersion(e.to_string()))).ok()
    });
    let license = required(cx, "license").and_then(|i| non_empty_text(cx, i, at));
    let description = required(cx, "description").and_then(|i| cx.text(i, at));
    let requires = match t.get("requires") {
        None => Some(Vec::new()),
        Some(i) => cx.strings(i, at).map(|names| {
            names
                .into_iter()
                .filter_map(|(s, sp)| CommandName::new(s).map_err(|e| cx.error(sp, ParseErrorKind::Invalid(e))).ok())
                .collect()
        }),
    };
    let os = match t.get("os") {
        None => Some(OsSet::ALL),
        Some(i) => parse_os(cx, i, at),
    };
    Some(PackMeta {
        name: name?,
        version: version?,
        license: license?,
        description: description?,
        requires: requires?,
        os: os?,
    })
}

fn non_empty_text(cx: &mut Cx, item: &Item, at: &Range<usize>) -> Option<Text> {
    let t = cx.text(item, at)?;
    if t.as_str().trim().is_empty() {
        cx.error(span(item, at), ParseErrorKind::Invalid(InvalidValue::Empty));
        return None;
    }
    Some(t)
}

fn parse_os(cx: &mut Cx, item: &Item, at: &Range<usize>) -> Option<OsSet> {
    let mut set = OsSet::NONE;
    for (s, sp) in cx.strings(item, at)? {
        match Os::from_key(s) {
            Some(os) => set = set.with(os),
            None => cx.error(sp, ParseErrorKind::UnknownOs(s.to_owned())),
        }
    }
    Some(set)
}

type GroupParts = (Option<Text>, OsSet, Vec<Entry>);

fn parse_group(cx: &mut Cx, t: &dyn TableLike, pack_os: OsSet, at: &Range<usize>) -> Option<GroupParts> {
    cx.only_keys(t, &["description", "os", "env", "aliases", "functions"], at);
    let description = t.get("description").map(|i| cx.text(i, at));
    let os = t.get("os").map(|i| parse_os(cx, i, at));
    let mut entries = Vec::new();
    if let Some(item) = t.get("env") {
        let at = span(item, at);
        if let Some(env) = cx.table(item, &at) {
            for (name, v) in env.iter() {
                let name = cx.key_valid(EnvName::new(name), env, name, &at);
                let value = parse_env_value(cx, v, &at);
                if let (Some(name), Some(value)) = (name, value) {
                    if let Some(e) = cx.valid(EnvEntry::new(name, value), v, &at) {
                        entries.push(Entry::Env(e));
                    }
                }
            }
        }
    }
    if let Some(item) = t.get("aliases") {
        let at = span(item, at);
        if let Some(aliases) = cx.table(item, &at) {
            for (name, v) in aliases.iter() {
                let name = cx.key_valid(CommandName::new(name), aliases, name, &at);
                let argv = match v.as_str() {
                    Some(s) => cx.valid(Argv::parse_shorthand(s), v, &at),
                    None if v.is_array() => cx.argv(v, &at),
                    None => {
                        cx.error(
                            span(v, &at),
                            ParseErrorKind::WrongType { expected: "a string or an array of strings" },
                        );
                        None
                    }
                };
                if let (Some(name), Some(argv)) = (name, argv) {
                    entries.push(Entry::Alias(Alias { name, argv }));
                }
            }
        }
    }
    if let Some(item) = t.get("functions") {
        let at = span(item, at);
        if let Some(functions) = cx.table(item, &at) {
            for (name, v) in functions.iter() {
                let fat = span(v, &at);
                let name = cx.key_valid(CommandName::new(name), functions, name, &fat);
                let parts = cx.table(v, &fat).and_then(|ft| parse_function(cx, ft, &fat));
                if let (Some(name), Some((description, body))) = (name, parts) {
                    entries.push(Entry::Function(Function { name, description, body }));
                }
            }
        }
    }
    let os = match os {
        Some(Some(os)) => pack_os.intersect(os),
        Some(None) => return None,
        None => pack_os,
    };
    Some((description.flatten(), os, entries))
}

fn parse_env_value(cx: &mut Cx, v: &Item, at: &Range<usize>) -> Option<EnvValue> {
    if let Some(s) = v.as_str() {
        return cx.valid(Word::parse(s), v, at).map(EnvValue::Set);
    }
    let Some(t) = v.as_table_like() else {
        cx.error(
            span(v, at),
            ParseErrorKind::WrongType { expected: "a string, { prepend/append = [...] }, or { secret = \"...\" }" },
        );
        return None;
    };
    let at = span(v, at);
    if let Some(purpose) = t.get("secret") {
        cx.only_keys(t, &["secret"], &at);
        return non_empty_text(cx, purpose, &at).map(|purpose| EnvValue::Secret { purpose });
    }
    cx.only_keys(t, &["prepend", "append"], &at);
    let mut list = |key: &str| match t.get(key) {
        None => Some(Vec::new()),
        Some(i) => cx.words(i, &at),
    };
    let (prepend, append) = (list("prepend")?, list("append")?);
    if prepend.is_empty() && append.is_empty() {
        cx.error(at, ParseErrorKind::MissingKey("prepend` or `append"));
        return None;
    }
    Some(EnvValue::PathList { prepend, append })
}

fn parse_function(cx: &mut Cx, t: &dyn TableLike, at: &Range<usize>) -> Option<(Option<Text>, FunctionBody)> {
    cx.only_keys(t, &["description", "script", "shell"], at);
    let description = match t.get("description") {
        Some(i) => Some(cx.text(i, at)?),
        None => None,
    };
    let body = match (t.get("script"), t.get("shell")) {
        (Some(s), None) => FunctionBody::Script(cx.text(s, at)?),
        (None, Some(sh)) => {
            let sat = span(sh, at);
            let shells = cx.table(sh, &sat)?;
            let keys: Vec<&str> = Dialect::ALL.iter().map(|d| d.key()).collect();
            cx.only_keys(shells, &keys, &sat);
            let mut bodies = BTreeMap::new();
            for d in Dialect::ALL {
                if let Some(b) = shells.get(d.key()) {
                    bodies.insert(d, cx.text(b, &sat)?);
                }
            }
            FunctionBody::InShell(cx.valid(InShell::new(bodies), sh, &sat)?)
        }
        _ => {
            cx.error(at.clone(), ParseErrorKind::FunctionBodyShape);
            return None;
        }
    };
    Some((description, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EntryName;

    const MINIMAL: &str =
        "format = 1\n[pack]\nname = \"p\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"d\"\n";

    fn parse(extra: &str) -> Result<PackDocument, Vec<ParseError>> {
        PackDocument::parse(&format!("{MINIMAL}{extra}"))
    }

    fn kinds(extra: &str) -> Vec<ParseErrorKind> {
        parse(extra).err().expect("expected parse errors").into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn parses_every_entry_kind_in_a_fixed_order() {
        let doc = parse(
            r#"
[groups.g]
os = ["linux"]
[groups.g.functions.f]
script = "echo hi"
[groups.g.aliases]
gs = "git status"
[groups.g.env]
PATH = { prepend = ["~/bin"] }
TOKEN = { secret = "for tests" }
"#,
        )
        .unwrap();
        let names: Vec<String> = doc.pack().entries().map(|(_, e)| e.name().to_string()).collect();
        assert_eq!(names, ["PATH", "TOKEN", "gs", "f"], "env, then aliases, then functions");
        let g = &doc.pack().groups()[0];
        assert!(g.os.contains(Os::Linux) && !g.os.contains(Os::MacOs), "group os narrows the pack's set");
    }

    #[test]
    fn render_is_byte_identical() {
        let src = format!("{MINIMAL}# keep me\n[groups.a.aliases]\nx = \"y\"   # and me\n");
        assert_eq!(PackDocument::parse(&src).unwrap().render(), src);
    }

    #[test]
    fn unknown_keys_fail_instead_of_dropping_entries() {
        assert_eq!(
            kinds("[groups.g.functions.f]\nscirpt = \"x\"\n"),
            [ParseErrorKind::UnknownKey("scirpt".into()), ParseErrorKind::FunctionBodyShape]
        );
        assert_eq!(kinds("colour = 1\n"), [ParseErrorKind::UnknownKey("colour".into())]);
        assert_eq!(
            kinds("[groups.g.functions.f.shell]\nksh = \"x\"\n"),
            [ParseErrorKind::UnknownKey("ksh".into()), ParseErrorKind::Invalid(InvalidValue::Empty)]
        );
    }

    #[test]
    fn collects_every_error_with_a_span() {
        let src = format!("{MINIMAL}[groups.g.aliases]\nok = \"ls\"\n\"a b\" = \"ls\"\nx = \"echo $HOME\"\n");
        let errs = PackDocument::parse(&src).err().unwrap();
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert_eq!(&src[errs[0].span.clone()], "\"a b\"", "the bad name is underlined");
        assert_eq!(&src[errs[1].span.clone()], "\"echo $HOME\"", "the bad value is underlined");
        assert_eq!(errs[1].line_col(&src), (10, 5));
    }

    #[test]
    fn format_must_be_one() {
        let src = MINIMAL.replace("format = 1", "format = 2");
        let errs = PackDocument::parse(&src).err().unwrap();
        assert_eq!(errs[0].kind, ParseErrorKind::UnsupportedFormat);
    }

    #[test]
    fn function_body_shapes() {
        assert_eq!(
            kinds("[groups.g.functions.f]\nscript = \"a\"\nshell.posix = \"b\"\n"),
            [ParseErrorKind::FunctionBodyShape]
        );
        let doc =
            parse("[groups.g.functions.f]\nshell.fish = \"cd $argv\"\nshell.posix = \"cd \\\"$1\\\"\"\n").unwrap();
        let (_, Entry::Function(f)) = doc.pack().entries().next().unwrap() else { panic!("function expected") };
        let FunctionBody::InShell(b) = &f.body else { panic!("in-shell body expected") };
        assert_eq!(b.dialects().collect::<Vec<_>>(), [Dialect::Posix, Dialect::Fish]);
    }

    #[test]
    fn env_values() {
        assert_eq!(
            kinds("[groups.g.env]\nEDITOR = { prepend = [\"/x\"] }\n"),
            [ParseErrorKind::Invalid(InvalidValue::PathListName)]
        );
        assert_eq!(kinds("[groups.g.env]\nPATH = {}\n"), [ParseErrorKind::MissingKey("prepend` or `append")]);
        assert_eq!(kinds("[groups.g.env]\nT = { secret = \"\" }\n"), [ParseErrorKind::Invalid(InvalidValue::Empty)]);
        let doc = parse("[groups.g.env]\nGOPATH = \"~/go\"\n").unwrap();
        let (_, Entry::Env(e)) = doc.pack().entries().next().unwrap() else { panic!("env expected") };
        assert_eq!(e.name(), &EnvName::new("GOPATH").unwrap());
        assert_eq!(e.value(), &EnvValue::Set(Word::Home(Text::new("/go").unwrap())));
    }

    #[test]
    fn same_name_in_two_groups_is_allowed_for_os_variants() {
        let doc = parse("[groups.mac]\nos=[\"macos\"]\naliases.copy = \"pbcopy\"\n[groups.lin]\nos=[\"linux\"]\naliases.copy = [\"xclip\", \"-sel\", \"clip\"]\n").unwrap();
        let names: Vec<EntryName> = doc.pack().entries().map(|(_, e)| e.name()).collect();
        assert_eq!(names.len(), 2);
    }
}
