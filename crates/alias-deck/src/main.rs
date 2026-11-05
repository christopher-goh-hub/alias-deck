//! The `alias-deck` command: argument parsing, prompts, and rendering over the library.

mod render;

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use alias_deck::{
    approve, build, check_update, context, decide, fetch, find_pack, list, load_installed, remove, setup, Confirm,
    Decision, Error, Paths, UpdateCheck,
};
use alias_deck_core::{
    compile, diff_packs, lint, Bindings, CompileOptions, Deck, Finding, Os, PackDocument, Rule, Severity, Shell,
    Source, Target,
};

const USAGE: &str = "\
usage: alias-deck <command> [options]

Install shared packs of shell aliases, functions, and environment variables.

  add <source>[@ref] [--yes] [--allow <rule>]...
                       fetch a pack, show everything it adds, and install it once you approve
  update [<pack>] [--yes] [--allow <rule>]...
                       show what changed upstream and install it once you approve
  remove <pack>        drop a pack by name or source, then rebuild
  list                 packs in precedence order
  build                compile every enabled pack into the init files
  setup [--shell <s>]  source the init file from your shell's startup file
  lint <pack.toml>...  check packs you write; exits 1 on a Danger finding
  compile <pack.toml>... [--shell <s>] [--os <os>] [--secrets <file>]
                       print one compiled file to stdout

Sources: ./pack.toml, ~/pack.toml, /abs/pack.toml, github:owner/repo[//path.toml],
git+https://host/repo.git[//path.toml], git+file:///dir/repo[//path.toml].
Without a terminal, add and update need --yes. A Danger finding also needs --allow <rule>.
Shells: bash, zsh, fish, pwsh. --shell defaults to $SHELL.";

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    yes: bool,
    allow: Vec<Rule>,
    shell: Option<Shell>,
    os: Option<Os>,
    secrets: Option<PathBuf>,
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") || argv.first().is_some_and(|a| a == "help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(command) = argv.first() else { return usage_error("expected a command") };
    if command == "--version" || command == "-V" {
        println!("alias-deck {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let flags: &[&str] = match command.as_str() {
        "add" | "update" => &["--yes", "--allow"],
        "setup" => &["--shell"],
        "compile" => &["--shell", "--os", "--secrets"],
        "remove" | "list" | "build" | "lint" => &[],
        other => return usage_error(&format!("unknown command `{other}`")),
    };
    let args = match parse_args(&argv[1..], flags) {
        Ok(a) => a,
        Err(msg) => return usage_error(&msg),
    };
    let arity = match command.as_str() {
        "add" | "remove" => args.positional.len() == 1,
        "update" => args.positional.len() <= 1,
        "list" | "build" | "setup" => args.positional.is_empty(),
        _ => !args.positional.is_empty(),
    };
    if !arity {
        return usage_error(&format!("wrong number of arguments for `{command}`"));
    }
    match command.as_str() {
        "compile" => run_compile(args),
        "lint" => run_lint(&args.positional),
        _ => match Paths::from_env() {
            Err(e) => fail(&e),
            Ok(paths) => match command.as_str() {
                "add" => run_add(&paths, &args),
                "update" => run_update(&paths, &args),
                "remove" => run_remove(&paths, &args.positional[0]),
                "list" => run_list(&paths),
                "build" => run_build(&paths),
                _ => run_setup(&paths, args.shell),
            },
        },
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("alias-deck: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

fn fail(e: &Error) -> ExitCode {
    match e {
        Error::Parse { .. } => eprintln!("{e}"),
        _ => eprintln!("alias-deck: {e}"),
    }
    ExitCode::FAILURE
}

fn parse_args(argv: &[String], allowed: &[&str]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let flag = if a == "-y" { "--yes" } else { a.as_str() };
        if flag.starts_with('-') && flag != "-" && !allowed.contains(&flag) {
            return Err(format!("unknown option `{a}`"));
        }
        let mut value = || it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match flag {
            "--yes" => args.yes = true,
            "--allow" => {
                let v = value()?;
                let ids: Vec<&str> = Rule::all().map(Rule::id).collect();
                let rule =
                    Rule::from_id(&v).ok_or_else(|| format!("unknown rule `{v}` (rules: {})", ids.join(", ")))?;
                args.allow.push(rule);
            }
            "--shell" => {
                let v = value()?;
                args.shell = Some(Shell::from_key(&v).ok_or_else(|| format!("unknown shell `{v}`"))?);
            }
            "--os" => {
                let v = value()?;
                args.os = Some(Os::from_key(&v).ok_or_else(|| format!("unknown os `{v}`"))?);
            }
            "--secrets" => args.secrets = Some(PathBuf::from(value()?)),
            _ => args.positional.push(a.clone()),
        }
    }
    Ok(args)
}

/// `$SHELL` names the login shell, which is the one most users want an init file for. Windows
/// sets no `$SHELL` outside Git Bash, and PowerShell is its shell.
fn detect_shell() -> Option<Shell> {
    let Some(shell) = env::var_os("SHELL") else { return cfg!(windows).then_some(Shell::Pwsh) };
    let name = Path::new(&shell).file_name()?.to_str()?;
    Shell::from_key(name.strip_suffix(".exe").unwrap_or(name))
}

fn read(path: &Path) -> Result<String, ExitCode> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!("alias-deck: {}: {e}", path.display());
        ExitCode::FAILURE
    })
}

fn parse_file(path: &Path) -> Result<PackDocument, ExitCode> {
    let src = read(path)?;
    PackDocument::parse(&src)
        .map_err(|errors| fail(&Error::Parse { origin: path.display().to_string(), src: src.clone(), errors }))
}

fn run_compile(args: Args) -> ExitCode {
    let Some(shell) = args.shell.or_else(detect_shell) else {
        return usage_error("could not tell your shell from $SHELL; pass --shell");
    };
    let target = Target { shell, os: args.os.unwrap_or_else(alias_deck::host_os) };
    let mut packs = Vec::new();
    let mut failed = false;
    for path in &args.positional {
        match parse_file(Path::new(path)) {
            Ok(doc) => packs.push(doc.pack().clone()),
            Err(_) => failed = true,
        }
    }
    let secrets = args.secrets.or_else(|| Paths::from_env().ok().map(|p| p.secrets()).filter(|p| p.is_file()));
    let bindings = match secrets {
        None => Bindings::default(),
        Some(path) => {
            let src = match read(&path) {
                Ok(s) => s,
                Err(code) => return code,
            };
            Bindings::parse(&src).unwrap_or_else(|errors| {
                fail(&Error::Parse { origin: path.display().to_string(), src: src.clone(), errors });
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
    eprint!("{}", render::compile_report(&[&out]));
    ExitCode::SUCCESS
}

fn run_lint(files: &[String]) -> ExitCode {
    let mut code = ExitCode::SUCCESS;
    for file in files {
        let doc = match parse_file(Path::new(file)) {
            Ok(d) => d,
            Err(c) => {
                code = c;
                continue;
            }
        };
        let findings = lint(doc.pack());
        print!("{}", render::findings(&format!("{file}: "), &findings));
        if findings.is_empty() {
            println!("{file}: no findings");
        }
        if findings.iter().any(|f| f.severity == Severity::Danger) {
            code = ExitCode::FAILURE;
        }
    }
    code
}

/// A relative local path is relative to where the user typed it, so the manifest stores it
/// absolute; `~/` stays as typed, so a synced manifest works on another machine.
fn absolute_source(source: Source) -> Source {
    match source {
        Source::Local(p) if p.is_relative() && !p.starts_with("~") => {
            Source::Local(std::path::absolute(&p).unwrap_or(p))
        }
        other => other,
    }
}

fn run_add(paths: &Paths, args: &Args) -> ExitCode {
    let source = match Source::parse_with_ref(&args.positional[0]) {
        Ok(s) => absolute_source(s),
        Err(e) => return usage_error(&format!("`{}`: {e}", args.positional[0])),
    };
    let result = (|| {
        let installed = load_installed(paths)?;
        if let Some(existing) = installed.manifest.find(&source) {
            if existing.source.reference() == source.reference() {
                println!("{source} is already in your deck. Run `alias-deck update` to check it for changes.");
                return Ok(ExitCode::SUCCESS);
            }
        }
        let cand = fetch(paths, &source)?;
        let ctx = context(paths, &installed, &cand)?;
        print!("{}", render::preview(&cand, &ctx));
        let name = cand.doc.pack().meta().name.as_str().to_owned();
        if !approved(decide(&cand.findings, args.yes, &args.allow), &name, &cand.findings) {
            return Ok(ExitCode::FAILURE);
        }
        let report = approve(paths, &cand)?;
        println!("Installed {name} from {source}.");
        eprint!("{}", render::compile_report(&report.outputs.iter().map(|o| &o.compiled).collect::<Vec<_>>()));
        Ok(ExitCode::SUCCESS)
    })();
    result.unwrap_or_else(|e: Error| fail(&e))
}

fn run_update(paths: &Paths, args: &Args) -> ExitCode {
    let result = (|| {
        let installed = load_installed(paths)?;
        let targets = match args.positional.first() {
            Some(q) => vec![find_pack(paths, &installed, q)?],
            None => installed.manifest.packs().to_vec(),
        };
        let mut code = ExitCode::SUCCESS;
        let mut last_build = None;
        for mp in &targets {
            match check_update(paths, &installed, mp)? {
                UpdateCheck::Local => println!("{}: a local file, read live at every build", mp.source),
                UpdateCheck::UpToDate => println!("{}: up to date", mp.source),
                UpdateCheck::Changed { old, new } => {
                    let name = new.doc.pack().meta().name.as_str().to_owned();
                    let findings = match &old {
                        None => {
                            print!("{}", render::preview(&new, &context(paths, &installed, &new)?));
                            new.findings.clone()
                        }
                        Some(old) => {
                            context(paths, &installed, &new)?;
                            let diff = diff_packs(old.pack(), new.doc.pack());
                            let locked = installed.lock.get(&mp.source);
                            print!("{}", render::update(old, &new, locked.map(|l| &l.commit), &diff));
                            diff.new_findings
                        }
                    };
                    if !approved(decide(&findings, args.yes, &args.allow), &name, &findings) {
                        code = ExitCode::FAILURE;
                        continue;
                    }
                    last_build = Some(approve(paths, &new)?);
                    println!("Updated {name}.");
                }
            }
        }
        if let Some(report) = last_build {
            eprint!("{}", render::compile_report(&report.outputs.iter().map(|o| &o.compiled).collect::<Vec<_>>()));
        }
        Ok(code)
    })();
    result.unwrap_or_else(|e: Error| fail(&e))
}

/// Applies the approval decision, prompting on the terminal when it asks for a person.
fn approved(decision: Decision, name: &str, findings: &[Finding]) -> bool {
    let stdin = io::stdin();
    let terminal = stdin.is_terminal().then(|| stdin.lock());
    confirm(decision, name, findings, terminal.map(|t| Box::new(t) as Box<dyn BufRead>))
}

/// `terminal` is `None` when stdin is not a terminal: then nobody can answer, so it refuses.
fn confirm(decision: Decision, name: &str, findings: &[Finding], terminal: Option<Box<dyn BufRead + '_>>) -> bool {
    let confirm = match decision {
        Decision::Proceed => return true,
        Decision::Refuse { unallowed } => {
            let ids: Vec<&str> = unallowed.iter().map(|r| r.id()).collect();
            let flags: Vec<String> = ids.iter().map(|id| format!("--allow {id}")).collect();
            eprintln!(
                "alias-deck: refusing to install {name} with --yes: it has Danger findings ({}). \
                 If you have read the preview and trust it, re-run with {}.",
                ids.join(", "),
                flags.join(" ")
            );
            return false;
        }
        Decision::Ask(c) => c,
    };
    let Some(mut terminal) = terminal else {
        let hint = if findings.iter().any(|f| f.severity == Severity::Danger) {
            "--yes and an --allow <rule> for each Danger finding"
        } else {
            "--yes"
        };
        eprintln!("alias-deck: stdin is not a terminal, so alias-deck cannot ask you to confirm. Read the preview above, then re-run with {hint}.");
        return false;
    };
    let prompt = match confirm {
        Confirm::YesNo => format!("Install {name}? [y/N] "),
        Confirm::TypeName => format!("{name} has Danger findings. Type the pack name ({name}) to install it: "),
    };
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if terminal.read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim();
    let ok = match confirm {
        Confirm::YesNo => matches!(answer, "y" | "Y" | "yes" | "YES" | "Yes"),
        Confirm::TypeName => answer == name,
    };
    if !ok {
        eprintln!("alias-deck: not installed");
    }
    ok
}

fn run_remove(paths: &Paths, query: &str) -> ExitCode {
    match remove(paths, query) {
        Ok(removed) => {
            println!("Removed {}.", removed.source);
            eprint!(
                "{}",
                render::compile_report(&removed.report.outputs.iter().map(|o| &o.compiled).collect::<Vec<_>>())
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn run_list(paths: &Paths) -> ExitCode {
    match list(paths) {
        Ok(packs) if packs.is_empty() => {
            println!("Your deck is empty. Add a pack with `alias-deck add <source>`.");
            ExitCode::SUCCESS
        }
        Ok(packs) => {
            print!("{}", render::list(&packs));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn run_build(paths: &Paths) -> ExitCode {
    match build(paths) {
        Ok(report) => {
            let files: Vec<String> = report.outputs.iter().map(|o| o.path.display().to_string()).collect();
            println!("Built {}.", files.join(", "));
            eprint!("{}", render::compile_report(&report.outputs.iter().map(|o| &o.compiled).collect::<Vec<_>>()));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn run_setup(paths: &Paths, shell: Option<Shell>) -> ExitCode {
    let Some(shell) = shell.or_else(detect_shell) else {
        return usage_error("could not tell your shell from $SHELL; pass --shell");
    };
    match setup(paths, shell) {
        Ok(changes) => {
            for c in &changes {
                let verb = if c.changed { "Added to" } else { "Already set up in" };
                println!("{verb} {} line {}: {}", c.file.display(), c.line_number, c.line);
            }
            if !paths.init(shell).exists() {
                println!("No init file yet. Run `alias-deck add <source>` to build one.");
            }
            if changes.iter().any(|c| c.changed) {
                println!("Open a new shell, or run `exec $SHELL`, to load your deck.");
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(decision: Decision, typed: Option<&'static str>) -> bool {
        let terminal = typed.map(|t| Box::new(io::Cursor::new(t)) as Box<dyn BufRead>);
        confirm(decision, "alice-tools", &[], terminal)
    }

    #[test]
    fn prompts_accept_only_the_expected_answer() {
        assert!(answer(Decision::Ask(Confirm::YesNo), Some("y\n")));
        assert!(!answer(Decision::Ask(Confirm::YesNo), Some("\n")), "the default is no");
        assert!(!answer(Decision::Ask(Confirm::TypeName), Some("y\n")), "a reflexive y is not the pack name");
        assert!(answer(Decision::Ask(Confirm::TypeName), Some("alice-tools\n")));
        assert!(!answer(Decision::Ask(Confirm::YesNo), None), "no terminal means no answer");
        assert!(answer(Decision::Proceed, None));
        assert!(!answer(Decision::Refuse { unallowed: vec![Rule::EnvLoaderHijack] }, Some("y\n")));
    }
}
