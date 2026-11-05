//! `alias-deck compile <pack.toml>... --shell <shell> [--os <os>] [--secrets <file>]`
//!
//! Prints the compiled source to stdout and every omitted entry and note to stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use alias_deck_core::{compile, Bindings, CompileOptions, Deck, Os, PackDocument, ParseError, Shell, Target};

const USAGE: &str = "\
usage: alias-deck compile <pack.toml>... [--shell bash|zsh|fish|pwsh] [--os macos|linux|windows] [--secrets <file>]

Compiles packs into one file for your shell to source. Later packs win name clashes.
The source goes to stdout. Omitted entries and notes go to stderr.

  --shell    target shell (default: from $SHELL)
  --os       target OS (default: this machine's)
  --secrets  secret bindings (default: ~/.config/alias-deck/secrets.toml if it exists)";

struct Args {
    packs: Vec<PathBuf>,
    shell: Option<Shell>,
    os: Option<Os>,
    secrets: Option<PathBuf>,
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match argv.first().map(String::as_str) {
        Some("compile") => {}
        Some("--version" | "-V") => {
            println!("alias-deck {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => return usage_error("expected a command"),
    }
    let args = match parse_args(&argv[1..]) {
        Ok(a) => a,
        Err(msg) => return usage_error(&msg),
    };
    run(args)
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("alias-deck: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args { packs: Vec::new(), shell: None, os: None, secrets: None };
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match a.as_str() {
            "--shell" => {
                let v = value("--shell")?;
                args.shell = Some(Shell::from_key(&v).ok_or_else(|| format!("unknown shell `{v}`"))?);
            }
            "--os" => {
                let v = value("--os")?;
                args.os = Some(Os::from_key(&v).ok_or_else(|| format!("unknown os `{v}`"))?);
            }
            "--secrets" => args.secrets = Some(PathBuf::from(value("--secrets")?)),
            flag if flag.starts_with('-') && flag != "-" => return Err(format!("unknown option `{flag}`")),
            pack => args.packs.push(PathBuf::from(pack)),
        }
    }
    if args.packs.is_empty() {
        return Err("give at least one pack file".into());
    }
    Ok(args)
}

fn host_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    }
}

/// `$SHELL` names the login shell, which is the one most users want an init file for.
fn detect_shell() -> Option<Shell> {
    let shell = env::var_os("SHELL")?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    Shell::from_key(name.strip_suffix(".exe").unwrap_or(name))
}

fn default_secrets() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("alias-deck/secrets.toml")).filter(|p| p.is_file())
}

fn report_parse_errors(path: &Path, src: &str, errors: &[ParseError]) {
    for e in errors {
        let (line, col) = e.line_col(src);
        eprintln!("{}:{line}:{col}: {e}", path.display());
    }
}

fn read(path: &Path) -> Result<String, ExitCode> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!("alias-deck: {}: {e}", path.display());
        ExitCode::FAILURE
    })
}

fn run(args: Args) -> ExitCode {
    let Some(shell) = args.shell.or_else(detect_shell) else {
        return usage_error("could not tell your shell from $SHELL; pass --shell");
    };
    let target = Target { shell, os: args.os.unwrap_or_else(host_os) };

    let mut packs = Vec::new();
    let mut failed = false;
    for path in &args.packs {
        let src = match read(path) {
            Ok(s) => s,
            Err(code) => return code,
        };
        match PackDocument::parse(&src) {
            Ok(doc) => packs.push(doc.pack().clone()),
            Err(errors) => {
                report_parse_errors(path, &src, &errors);
                failed = true;
            }
        }
    }
    let bindings = match args.secrets.or_else(default_secrets) {
        None => Bindings::default(),
        Some(path) => {
            let src = match read(&path) {
                Ok(s) => s,
                Err(code) => return code,
            };
            Bindings::parse(&src).unwrap_or_else(|errors| {
                report_parse_errors(&path, &src, &errors);
                failed = true;
                Bindings::default()
            })
        }
    };
    if failed {
        return ExitCode::FAILURE;
    }

    let out = compile(&Deck::new(packs), &bindings, target, &CompileOptions::default());
    print!("{}", out.source);
    for r in &out.entries {
        if r.outcome.is_omitted() {
            eprintln!("alias-deck: {r}");
        }
        for n in &r.notes {
            eprintln!("alias-deck: {}: note: {n}", r.entry);
        }
    }
    ExitCode::SUCCESS
}
