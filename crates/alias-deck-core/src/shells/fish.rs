//! fish 3.4 and later (`"$(...)"` captures a secret as one value).

use super::{header_comments, Backend};
use crate::compile::{CompileOptions, FishAliasStyle, Item, Target};
use crate::model::{Argv, CommandName, Dialect, PackMeta, Text};
use crate::writer::{Quoting, Writer};

pub(crate) struct Fish;

/// fish 4.9 refuses `function NAME; end` for each of these (measured), plus `abbr` and
/// `contains`, which the compiled file itself runs.
#[rustfmt::skip]
const RESERVED: &[&str] = &[
    "_", "abbr", "and", "argparse", "begin", "break", "builtin", "case", "command", "contains", "continue", "else",
    "end", "eval", "exec", "for", "function", "if", "not", "or", "read", "return", "set", "status", "string",
    "switch", "test", "time", "while",
];

fn argv_words(w: &mut Writer, argv: &Argv) {
    w.arg(argv.program());
    for a in argv.args() {
        w.code(" ").arg(a);
    }
}

fn function_open(w: &mut Writer, name: &CommandName, description: Option<&Text>) {
    w.code("function ").name(name);
    if let Some(d) = description {
        w.code(" --description ").text(d.as_str());
    }
    w.code("\n");
}

impl Backend for Fish {
    fn quoting(&self) -> Quoting {
        Quoting::Fish
    }

    fn dialects(&self) -> &'static [Dialect] {
        &[Dialect::Fish]
    }

    fn is_reserved(&self, name: &CommandName) -> bool {
        RESERVED.contains(&name.as_str())
    }

    fn header(&self, w: &mut Writer, target: Target, packs: &[&PackMeta]) {
        header_comments(w, target, packs);
    }

    fn render(&self, w: &mut Writer, item: &Item<'_>, options: &CompileOptions) {
        match item {
            // `command -q` first: fish prints "Unknown command" despite `2>/dev/null`.
            Item::Secret { name, lookup } => {
                w.code("begin\n    command -q ").arg(lookup.program());
                w.code("\n    and set -l __alias_deck_v \"$(command ");
                argv_words(w, lookup);
                w.code(" 2>/dev/null)\"\n    and test -n \"$__alias_deck_v\"\n    and set -gx ").name(*name);
                w.code(" \"$__alias_deck_v\"\nend\n");
            }
            Item::SetEnv { name, value } => {
                w.code("set -gx ").name(*name).code(" ").expr(value).code("\n");
            }
            Item::PathList { name, prepend, append } => {
                for word in prepend.iter().rev() {
                    w.code("if not contains -- ").arg(word).code(" $").name(*name).code("\n    set -gx --path ");
                    w.name(*name).code(" ").arg(word).code(" $").name(*name).code("\nend\n");
                }
                for word in append.iter() {
                    w.code("if not contains -- ").arg(word).code(" $").name(*name).code("\n    set -gx --path ");
                    w.name(*name).code(" $").name(*name).code(" ").arg(word).code("\nend\n");
                }
            }
            Item::Alias { name, argv, self_ref } => match options.fish_aliases {
                FishAliasStyle::Abbr => {
                    w.code("abbr --add -- ").name(*name).code(" ").nested(|w| argv_words(w, argv)).code("\n");
                }
                // fish's own `alias` evaluates its text at startup, so a plain function is used.
                FishAliasStyle::Function if *self_ref => {
                    w.code("function ").name(*name).code("\n    command ");
                    argv_words(w, argv);
                    w.code(" $argv\nend\n");
                }
                FishAliasStyle::Function => {
                    w.code("function ").name(*name).code(" --wraps ").nested(|w| argv_words(w, argv)).code("\n    ");
                    argv_words(w, argv);
                    w.code(" $argv\nend\n");
                }
            },
            Item::ScriptFn { name, body, description } => {
                function_open(w, name, *description);
                w.code("    /bin/sh -c ").body(body).code(" ").text(name.as_str()).code(" $argv\nend\n");
            }
            Item::InShellFn { name, code, description, .. } => {
                function_open(w, name, *description);
                w.code("    eval ").body(code).code("\nend\n");
            }
        }
    }
}
