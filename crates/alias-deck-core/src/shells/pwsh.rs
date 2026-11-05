//! PowerShell 7.3 and later.

use super::{header_comments, Backend};
use crate::compile::{CompileOptions, Item, Note, Target};
use crate::model::{Argv, CommandName, Dialect, Os, PackMeta};
use crate::writer::{Quoting, Writer};

pub(crate) struct Pwsh;

const KEYWORDS: &[&str] = &[
    "begin",
    "break",
    "catch",
    "class",
    "clean",
    "continue",
    "data",
    "define",
    "do",
    "dynamicparam",
    "else",
    "elseif",
    "end",
    "enum",
    "exit",
    "filter",
    "finally",
    "for",
    "foreach",
    "from",
    "function",
    "hidden",
    "if",
    "in",
    "inlinescript",
    "parallel",
    "param",
    "process",
    "return",
    "sequence",
    "switch",
    "throw",
    "trap",
    "try",
    "until",
    "using",
    "var",
    "while",
    "workflow",
];

/// A one-word alias keeps pwsh's native argument passing as `Set-Alias`. A self-referential
/// one would resolve to itself, so it goes through a function.
fn is_set_alias(argv: &Argv, self_ref: bool) -> bool {
    argv.args().is_empty() && !self_ref
}

/// Built-in aliases outrank functions, and `gp` and `gcm` are ReadOnly, hence `-Force`.
fn remove_alias(w: &mut Writer, name: &CommandName) {
    w.code("Remove-Alias -Name ").text(name.as_str()).code(" -Force -ErrorAction Ignore\n");
}

/// A pwsh function does not hand its pipeline input to a native command on its own. Piping an
/// empty `$input` when nothing is piped would close stdin, hence the branch.
fn forwarding_function(w: &mut Writer, name: &CommandName, call: impl Fn(&mut Writer)) {
    remove_alias(w, name);
    w.code("function ").name(name).code(" {\n    if ($MyInvocation.ExpectingInput) { $input | ");
    call(w);
    w.code(" }\n    else { ");
    call(w);
    w.code(" }\n}\n");
}

fn call_args(w: &mut Writer, argv: &Argv) {
    for a in argv.args() {
        w.code(" ").arg(a);
    }
}

impl Backend for Pwsh {
    fn quoting(&self) -> Quoting {
        Quoting::Pwsh
    }

    fn dialects(&self) -> &'static [Dialect] {
        &[Dialect::Pwsh]
    }

    fn is_reserved(&self, name: &CommandName) -> bool {
        KEYWORDS.contains(&name.as_str().to_ascii_lowercase().as_str())
    }

    fn command_key(&self, name: &CommandName) -> String {
        name.as_str().to_ascii_lowercase()
    }

    fn list_separator(&self, os: Os) -> char {
        match os {
            Os::Windows => ';',
            Os::MacOs | Os::Linux => ':',
        }
    }

    fn notes(&self, item: &Item<'_>) -> Vec<Note> {
        match item {
            Item::Alias { argv, self_ref, .. } if !is_set_alias(argv, *self_ref) => vec![Note::PwshArgRewriting],
            Item::ScriptFn { .. } => vec![Note::PwshArgRewriting],
            _ => Vec::new(),
        }
    }

    fn header(&self, w: &mut Writer, target: Target, packs: &[&PackMeta]) {
        header_comments(w, target, packs);
        // 7.3 made native argument passing exact on Unix, which every wrapper here relies on.
        w.code("if ($PSVersionTable.PSVersion -lt [version]'7.3') {\n");
        w.code("    Write-Warning 'alias-deck: PowerShell 7.3 or newer is required; nothing was loaded.'\n");
        w.code("    return\n}\n");
    }

    fn render(&self, w: &mut Writer, item: &Item<'_>, _options: &CompileOptions) {
        match item {
            Item::Secret { name, lookup } => {
                w.code("& { try { $v = ((& ").arg(lookup.program());
                call_args(w, lookup);
                w.code(" 2>$null) -join \"`n\").TrimEnd(\"`n\"); if ($LASTEXITCODE -eq 0 -and $v) { $env:");
                w.name(*name).code(" = $v } } catch {} }\n");
            }
            Item::SetEnv { name, value } => {
                w.code("$env:").name(*name).code(" = ").expr(value).code("\n");
            }
            Item::PathList { name, prepend, append } => {
                let entries = prepend.iter().rev().map(|w| (w, true)).chain(append.iter().map(|w| (w, false)));
                for (word, front) in entries {
                    w.code("& {\n    $d = ").expr(word).code("; $s = [IO.Path]::PathSeparator\n");
                    w.code("    if (-not $env:").name(*name).code(") { $env:").name(*name).code(" = $d }\n");
                    w.code("    elseif ($env:").name(*name).code(".Split($s) -cnotcontains $d) { $env:").name(*name);
                    if front {
                        w.code(" = $d + $s + $env:").name(*name).code(" }\n}\n");
                    } else {
                        w.code(" = $env:").name(*name).code(" + $s + $d }\n}\n");
                    }
                }
            }
            Item::Alias { name, argv, self_ref } if is_set_alias(argv, *self_ref) => {
                remove_alias(w, name);
                w.code("Set-Alias -Name ").text(name.as_str()).code(" -Value ").arg(argv.program()).code("\n");
            }
            Item::Alias { name, argv, self_ref } => forwarding_function(w, name, |w| {
                if *self_ref {
                    w.code("& (Get-Command -Name ")
                        .text(name.as_str())
                        .code(" -CommandType Application -TotalCount 1)");
                } else {
                    w.code("& ").arg(argv.program());
                }
                call_args(w, argv);
                w.code(" @args");
            }),
            Item::ScriptFn { name, body, .. } => forwarding_function(w, name, |w| {
                w.code("& '/bin/sh' -c ").body(body).code(" ").text(name.as_str()).code(" @args");
            }),
            Item::InShellFn { name, code, .. } => {
                remove_alias(w, name);
                w.code("function ").name(*name).code(" {\n    . ([scriptblock]::Create(").body(code);
                w.code(")) @args\n}\n");
            }
        }
    }
}
