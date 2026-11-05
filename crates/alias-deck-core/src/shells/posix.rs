//! bash (3.2 and later) and zsh.

use super::{header_comments, Backend};
use crate::compile::{CompileOptions, Item, Target};
use crate::model::{CommandName, Dialect, PackMeta, Word};
use crate::writer::{Quoting, Writer};

pub(crate) struct Posix {
    zsh: bool,
}

pub(crate) static BASH: Posix = Posix { zsh: false };
pub(crate) static ZSH: Posix = Posix { zsh: true };

#[rustfmt::skip]
const KEYWORDS: &[&str] = &[
    "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in", "select", "then",
    "time", "until", "while",
];
#[rustfmt::skip]
const ZSH_KEYWORDS: &[&str] = &[
    "declare", "end", "export", "float", "foreach", "integer", "local", "nocorrect", "noglob", "readonly",
    "repeat", "typeset",
];

impl Backend for Posix {
    fn quoting(&self) -> Quoting {
        Quoting::Posix
    }

    fn dialects(&self) -> &'static [Dialect] {
        if self.zsh {
            &[Dialect::Zsh, Dialect::Posix]
        } else {
            &[Dialect::Bash, Dialect::Posix]
        }
    }

    fn is_reserved(&self, name: &CommandName) -> bool {
        KEYWORDS.contains(&name.as_str()) || (self.zsh && ZSH_KEYWORDS.contains(&name.as_str()))
    }

    fn header(&self, w: &mut Writer, target: Target, packs: &[&PackMeta]) {
        header_comments(w, target, packs);
    }

    fn render(&self, w: &mut Writer, item: &Item<'_>, _options: &CompileOptions) {
        match item {
            Item::Secret { name, lookup } => {
                w.code("if __alias_deck_v=$(").arg(lookup.program());
                for a in lookup.args() {
                    w.code(" ").arg(a);
                }
                w.code(" 2>/dev/null) && [ -n \"$__alias_deck_v\" ]; then export ").name(*name);
                w.code("=\"$__alias_deck_v\"; fi\nunset __alias_deck_v\n");
            }
            Item::SetEnv { name, value } => {
                w.code("export ").name(*name).code("=").expr(value).code("\n");
            }
            Item::PathList { name, prepend, append } => {
                for word in prepend.iter().rev() {
                    path_case(w, name, word);
                    w.code(") ;; *) ").name(*name).code("=").arg(word).code("\"${").name(*name).code(":+:$");
                    w.name(*name).code("}\" ;; esac\n");
                }
                for word in append.iter() {
                    path_case(w, name, word);
                    w.code(") ;; *) ").name(*name).code("=\"${").name(*name).code(":+$").name(*name).code(":}\"");
                    w.arg(word).code(" ;; esac\n");
                }
                w.code("export ").name(*name).code("\n");
            }
            Item::Alias { name, argv, .. } => {
                w.code("alias ").name(*name).code("=").nested(|w| {
                    w.arg(argv.program());
                    for a in argv.args() {
                        w.code(" ").arg(a);
                    }
                });
                w.code("\n");
            }
            Item::ScriptFn { name, body, .. } => {
                function_open(w, name);
                w.code("  /bin/sh -c ").body(body).code(" ").text(name.as_str()).code(" \"$@\"\n}\n");
            }
            Item::InShellFn { name, dialect, code, .. } => {
                function_open(w, name);
                if self.zsh && *dialect == Dialect::Posix {
                    w.code("  emulate -L sh\n");
                }
                w.code("  eval ").body(code).code("\n}\n");
            }
        }
    }
}

/// `case ":${N-}:" in *:WORD:*`. The `-` keeps it working under `set -u`.
fn path_case(w: &mut Writer, name: &crate::model::EnvName, word: &Word) {
    w.code("case \":${").name(name).code("-}:\" in *:").arg(word).code(":*");
}

/// `function NAME {` rather than `NAME() {`: an existing alias NAME would expand inside `NAME()`.
fn function_open(w: &mut Writer, name: &CommandName) {
    w.code("unalias ").name(name).code(" 2>/dev/null\nfunction ").name(name).code(" {\n");
}
