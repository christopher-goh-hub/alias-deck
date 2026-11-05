//! The only way compiled text is produced, and so the safety boundary.
//!
//! Pack bytes reach the output through two doors only:
//! 1. Quoted, through `arg`, `expr`, `text`, `body`, or `nested`.
//! 2. Bare, through `name`, which takes only validated names whose grammar excludes every
//!    shell metacharacter.
//!
//! Compiler syntax goes through `code`, which takes `&'static str`, so a runtime string cannot
//! become code. The quoting rules were measured byte for byte in bash 3.2, bash 5.3, zsh 5.9,
//! fish 4.9, and pwsh 7.6; `tests/quote_vectors.tsv` pins them and `tests/shells.rs` re-proves them.

use crate::model::{CommandName, EnvName, Slug, Text, Word};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quoting {
    /// bash and zsh.
    Posix,
    Fish,
    Pwsh,
}

/// A name safe to print bare in every shell. Sealed: only validated types implement it.
pub(crate) trait BareName {
    fn bare(&self) -> String;
}

impl BareName for CommandName {
    fn bare(&self) -> String {
        self.as_str().to_owned()
    }
}

impl BareName for EnvName {
    fn bare(&self) -> String {
        self.as_str().to_owned()
    }
}

impl BareName for Slug {
    fn bare(&self) -> String {
        self.as_str().to_owned()
    }
}

/// semver's grammar is `[0-9A-Za-z.+-]`.
impl BareName for semver::Version {
    fn bare(&self) -> String {
        self.to_string()
    }
}

pub(crate) struct Writer {
    q: Quoting,
    out: String,
}

impl Writer {
    pub(crate) fn new(q: Quoting) -> Writer {
        Writer { q, out: String::new() }
    }

    pub(crate) fn code(&mut self, s: &'static str) -> &mut Self {
        self.out.push_str(s);
        self
    }

    pub(crate) fn name(&mut self, n: &impl BareName) -> &mut Self {
        self.out.push_str(&n.bare());
        self
    }

    /// A word in argument position. It may print bare when that is provably the same bytes.
    pub(crate) fn arg(&mut self, w: &Word) -> &mut Self {
        self.word(w, Pos::Arg)
    }

    /// A word in expression position (the right side of an assignment). pwsh always quotes
    /// here, because `$env:X = -n` is a parse error.
    pub(crate) fn expr(&mut self, w: &Word) -> &mut Self {
        self.word(w, Pos::Expr)
    }

    /// Free text such as a description or a command name passed as data, as one literal.
    pub(crate) fn text(&mut self, t: &str) -> &mut Self {
        quote(self.q, t, Pos::Arg, &mut self.out);
        self
    }

    /// A function or script body, as one quoted literal. There is no verbatim variant: bodies
    /// run when called, never while the file is sourced.
    pub(crate) fn body(&mut self, t: &Text) -> &mut Self {
        quote(self.q, t.as_str(), Pos::Expr, &mut self.out);
        self
    }

    /// Renders `f` into a scratch writer and emits the result as one quoted literal. This builds
    /// a second quoting layer (bash `alias x='...'`, fish `--wraps '...'`) without hand-escaping.
    pub(crate) fn nested(&mut self, f: impl FnOnce(&mut Writer)) -> &mut Self {
        let mut inner = Writer::new(self.q);
        f(&mut inner);
        quote(self.q, &inner.out, Pos::Arg, &mut self.out);
        self
    }

    pub(crate) fn finish(self) -> String {
        self.out
    }

    fn word(&mut self, w: &Word, pos: Pos) -> &mut Self {
        match (w, self.q) {
            (Word::Literal(t), q) => quote(q, t.as_str(), pos, &mut self.out),
            (Word::Home(suffix), Quoting::Posix | Quoting::Fish) => {
                self.out.push_str("\"$HOME\"");
                if !suffix.as_str().is_empty() {
                    quote(self.q, suffix.as_str(), Pos::Arg, &mut self.out);
                }
            }
            (Word::Home(suffix), Quoting::Pwsh) => match suffix.as_str().strip_prefix('/') {
                None => self.out.push_str("$HOME"),
                Some(rest) => {
                    self.out.push_str("(Join-Path $HOME ");
                    quote(Quoting::Pwsh, rest, Pos::Expr, &mut self.out);
                    self.out.push(')');
                }
            },
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pos {
    Arg,
    Expr,
}

fn quote(q: Quoting, s: &str, pos: Pos, out: &mut String) {
    let quoted = match (q, pos) {
        (Quoting::Posix, _) => posix_quote(s),
        (Quoting::Fish, _) => fish_quote(s),
        (Quoting::Pwsh, Pos::Arg) => pwsh_arg(s),
        (Quoting::Pwsh, Pos::Expr) => pwsh_expr(s),
    };
    out.push_str(&quoted);
}

fn in_bare_set(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_./:@%+,=-".contains(&b))
}

/// `'...'` keeps every byte in bash and zsh; `'` is written `'\''`. UTF-8 never puts 0x27 inside
/// a multibyte sequence, so this holds in every locale. zsh EQUALS expands a leading `=cmd`, and
/// MAGIC_EQUAL_SUBST expands `==cmd` and `:=cmd` after an `=`, so those words are quoted.
pub(crate) fn posix_quote(s: &str) -> String {
    if in_bare_set(s) && !s.starts_with('=') && !s.contains("==") && !s.contains(":=") {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// fish single quotes keep every byte except `\` and `'`, which take a backslash. fish 4.9
/// still expands the word `%self` to its PID, so a leading `%` is quoted.
pub(crate) fn fish_quote(s: &str) -> String {
    if in_bare_set(s) && !s.starts_with('%') {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
}

/// pwsh ends a single-quoted string at `'` and at U+2018 to U+201B; doubling escapes each.
pub(crate) fn pwsh_expr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Bare only for `-{0,2}[A-Za-z][A-Za-z0-9_-]*`. A quoted `'-Force'` binds as a positional
/// string to a cmdlet, while pwsh would read bare `007` as a number, `a,b` as an array, and
/// `@x` as a splat, and drops a bare `--`.
pub(crate) fn pwsh_arg(s: &str) -> String {
    let rest = s.trim_start_matches('-');
    let mut chars = rest.chars();
    let bare = s.len() - rest.len() <= 2
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        s.to_owned()
    } else {
        pwsh_expr(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> String {
        let bytes = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn quoters_match_the_shell_verified_reference() {
        let vectors = include_str!("../tests/quote_vectors.tsv");
        let mut n = 0;
        for line in vectors.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<String> = line.split('\t').map(unhex).collect();
            let input = f[0].as_str();
            assert_eq!(posix_quote(input), f[1], "posix {input:?}");
            assert_eq!(fish_quote(input), f[2], "fish {input:?}");
            assert_eq!(pwsh_arg(input), f[3], "pwsh arg {input:?}");
            assert_eq!(pwsh_expr(input), f[4], "pwsh expr {input:?}");
            n += 1;
        }
        assert!(n > 400, "only {n} vectors loaded");
    }

    fn word(s: &str) -> Word {
        Word::parse(s).unwrap()
    }

    fn render(q: Quoting, f: impl FnOnce(&mut Writer)) -> String {
        let mut w = Writer::new(q);
        f(&mut w);
        w.finish()
    }

    #[test]
    fn home_words() {
        assert_eq!(render(Quoting::Posix, |w| drop(w.arg(&word("~")))), r#""$HOME""#);
        assert_eq!(render(Quoting::Fish, |w| drop(w.arg(&word("~/a b")))), r#""$HOME"'/a b'"#);
        assert_eq!(render(Quoting::Pwsh, |w| drop(w.expr(&word("~/go")))), "(Join-Path $HOME 'go')");
        assert_eq!(render(Quoting::Pwsh, |w| drop(w.arg(&word("~/")))), "(Join-Path $HOME '')");
        assert_eq!(render(Quoting::Pwsh, |w| drop(w.arg(&word("~")))), "$HOME");
    }

    #[test]
    fn pwsh_quotes_by_position() {
        assert_eq!(render(Quoting::Pwsh, |w| drop(w.arg(&word("-n")))), "-n");
        assert_eq!(render(Quoting::Pwsh, |w| drop(w.expr(&word("-n")))), "'-n'");
    }

    #[test]
    fn nested_builds_the_second_layer_mechanically() {
        let msg = "wip: don't merge! \"$USER\" `draft` \\o/ 100%";
        let posix = render(Quoting::Posix, |w| {
            w.code("alias x=").nested(|w| {
                w.arg(&word("git")).code(" ").arg(&word("-m")).code(" ").arg(&word(msg));
            });
        });
        assert_eq!(posix, r#"alias x='git -m '\''wip: don'\''\'\'''\''t merge! "$USER" `draft` \o/ 100%'\'''"#);
        let fish = render(Quoting::Fish, |w| drop(w.nested(|w| drop(w.arg(&word(msg))))));
        assert_eq!(fish, r#"'\'wip: don\\\'t merge! "$USER" `draft` \\\\o/ 100%\''"#);
    }
}
