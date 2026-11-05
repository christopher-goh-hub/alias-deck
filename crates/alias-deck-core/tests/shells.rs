//! Sources compiled output in the real shells and checks behavior byte for byte.
//!
//! This binary is also its own probe executables: symlinked as `git`, `sudo`, `grep`,
//! `envprobe`, `cwdprobe`, `secretprobe`, or `canary`, it reports what a child process received
//! (argv, exported env bytes, cwd, stdin) instead of running the harness. Expectations come from
//! the parsed pack model, never from the shell under test.
//!
//! A shell that is not installed is skipped and printed. `ALIAS_DECK_REQUIRE_SHELLS=bash,pwsh`
//! fails the run unless each named shell ran in at least one configuration;
//! `ALIAS_DECK_REQUIRE_ALL_SHELLS=1` names all four.
//!
//! On Windows the harness does not run: its expectations assume POSIX paths, a `HOME` that pwsh
//! honors, and `:` as the PATH separator. The CLI's tests source the built files in pwsh and
//! Git Bash there instead.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs, thread};

use alias_deck_core::{
    compile, Bindings, CompileOptions, Compiled, Deck, Entry, EnvValue, FishAliasStyle, OmitReason, Os, Outcome, Pack,
    PackDocument, Shell, Target, Word,
};

/// Every hazard class in one secret: quotes of all kinds, `$`, a backtick, a backslash, UTF-8,
/// and an interior newline.
const SECRET: &str = "ghp_t\u{e9}st $HOME 'q' \"d\" `x` \\ \u{2713} \u{2018}s\u{2019}\nline2";

/// Caller arguments typed after every alias and function. No `*`: pwsh globs wildcard arguments
/// that pass through a wrapper function, a documented limitation.
const EXTRA: [&str; 7] = ["x y", "", "$HOME", "it's \u{2018}q\u{2019}", "--", "-Force", "a?b"];

const ERR_BEGIN: &str = "@@ERR begin";
const ERR_END: &str = "@@ERR end";
const PROBES: [&str; 4] = ["envprobe", "cwdprobe", "secretprobe", "canary"];
const FUZZ_SEED: u64 = 0x5EED_A11A_5DEC;
const FUZZ_COUNT: usize = 150;

fn main() -> ExitCode {
    let argv0 = env::args_os().next().unwrap_or_default();
    let name = Path::new(&argv0).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.starts_with("shells") {
        return probe(&name, env::args_os().skip(1).collect());
    }
    harness()
}

#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    std::os::unix::ffi::OsStrExt::as_bytes(s).to_vec()
}

#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn argv_line<S: AsRef<str>>(argv: &[S]) -> String {
    let parts: Vec<String> = argv.iter().map(|a| hex(a.as_ref().as_bytes())).collect();
    format!("@@ARGV {}", parts.join(","))
}

fn probe(name: &str, args: Vec<OsString>) -> ExitCode {
    let out = std::io::stdout();
    let mut out = out.lock();
    match name {
        "envprobe" => {
            for a in &args {
                match env::var_os(a) {
                    Some(v) => writeln!(out, "@@ENV {} {}", a.to_string_lossy(), hex(&os_bytes(&v))),
                    None => writeln!(out, "@@ENV {} UNSET", a.to_string_lossy()),
                }
                .unwrap();
            }
        }
        "cwdprobe" => writeln!(out, "@@CWD {}", hex(&os_bytes(env::current_dir().unwrap().as_os_str()))).unwrap(),
        "secretprobe" => match args.first().and_then(|a| a.to_str()) {
            Some("found") => write!(out, "{SECRET}\n\n").unwrap(),
            Some("empty") => {}
            _ => {
                eprintln!("secretprobe: The specified item could not be found in the keychain.");
                return ExitCode::from(44);
            }
        },
        "canary" => {
            let path = env::var_os("CANARY").expect("CANARY is set by the harness");
            let mut f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(f, "canary ran: {:?}", args).unwrap();
        }
        _ => {
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            if name == "git" && args == ["rev-parse", "--show-toplevel"] {
                writeln!(out, "{}", env::var("PROBE_TOPLEVEL").unwrap()).unwrap();
            } else if args.iter().any(|a| a == "--probe-stdin") {
                let mut buf = Vec::new();
                std::io::stdin().read_to_end(&mut buf).unwrap();
                writeln!(out, "@@STDIN {}", hex(&buf)).unwrap();
            } else {
                let mut argv = vec![name.to_owned()];
                argv.extend(args);
                writeln!(out, "{}", argv_line(&argv)).unwrap();
            }
        }
    }
    ExitCode::SUCCESS
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fam {
    Posix,
    Fish,
    Pwsh,
}

fn fam(shell: Shell) -> Fam {
    match shell {
        Shell::Bash | Shell::Zsh => Fam::Posix,
        Shell::Fish => Fam::Fish,
        Shell::Pwsh => Fam::Pwsh,
    }
}

/// The harness's own quoting, always quoted and deliberately simpler than the product's.
fn dq(f: Fam, s: &str) -> String {
    match f {
        Fam::Posix => format!("'{}'", s.replace('\'', "'\\''")),
        Fam::Fish => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Fam::Pwsh => {
            let doubled: String = s
                .chars()
                .flat_map(|c| if "'\u{2018}\u{2019}\u{201a}\u{201b}".contains(c) { vec![c, c] } else { vec![c] })
                .collect();
            format!("'{doubled}'")
        }
    }
}

struct Config {
    label: &'static str,
    shell: Shell,
    candidates: &'static [&'static str],
    version: &'static str,
    args: &'static [&'static str],
    hostile: bool,
}

const BASH_ARGS: &[&str] = &["--norc", "--noprofile", "-i"];
const BASH5: &[&str] = &["/opt/homebrew/bin/bash", "/usr/local/bin/bash", "/usr/bin/bash", "/bin/bash"];
const ZSH: &[&str] = &["/bin/zsh", "/usr/bin/zsh", "/opt/homebrew/bin/zsh", "zsh"];
const FISH: &[&str] = &["/opt/homebrew/bin/fish", "/usr/local/bin/fish", "/usr/bin/fish", "fish"];
const PWSH: &[&str] = &["~/.local/bin/pwsh", "/opt/homebrew/bin/pwsh", "/usr/local/bin/pwsh", "/usr/bin/pwsh", "pwsh"];
const PWSH_ARGS: &[&str] = &["-NoProfile", "-NonInteractive", "-File"];

#[rustfmt::skip]
const CONFIGS: &[Config] = &[
    Config { label: "bash 3.2", shell: Shell::Bash, candidates: &["/bin/bash"], version: "version 3.2", args: BASH_ARGS, hostile: false },
    Config { label: "bash 3.2 hostile", shell: Shell::Bash, candidates: &["/bin/bash"], version: "version 3.2", args: BASH_ARGS, hostile: true },
    Config { label: "bash 5", shell: Shell::Bash, candidates: BASH5, version: "version 5.", args: BASH_ARGS, hostile: false },
    Config { label: "bash 5 hostile", shell: Shell::Bash, candidates: BASH5, version: "version 5.", args: BASH_ARGS, hostile: true },
    Config { label: "zsh", shell: Shell::Zsh, candidates: ZSH, version: "zsh 5.", args: &["-f", "-i"], hostile: false },
    Config { label: "zsh hostile", shell: Shell::Zsh, candidates: ZSH, version: "zsh 5.", args: &["-f", "-i"], hostile: true },
    Config { label: "fish", shell: Shell::Fish, candidates: FISH, version: "fish, version", args: &["--no-config"], hostile: false },
    Config { label: "pwsh", shell: Shell::Pwsh, candidates: PWSH, version: "PowerShell 7.", args: PWSH_ARGS, hostile: false },
    Config { label: "pwsh strict", shell: Shell::Pwsh, candidates: PWSH, version: "PowerShell 7.", args: PWSH_ARGS, hostile: true },
];

fn find_shell(c: &Config) -> Option<(PathBuf, String)> {
    let home = env::var("HOME").unwrap_or_default();
    let path_dirs: Vec<PathBuf> = env::var_os("PATH").map(|p| env::split_paths(&p).collect()).unwrap_or_default();
    let candidates = c.candidates.iter().flat_map(|cand| {
        if let Some(rest) = cand.strip_prefix("~/") {
            vec![Path::new(&home).join(rest)]
        } else if cand.contains('/') {
            vec![PathBuf::from(cand)]
        } else {
            path_dirs.iter().map(|d| d.join(cand)).collect()
        }
    });
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let Ok(out) = Command::new(&path).arg("--version").stdin(Stdio::null()).output() else { continue };
        let version = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned();
        if version.contains(c.version) {
            return Some((path, version));
        }
    }
    None
}

enum Action {
    Call(String, Vec<String>),
    Cd(String),
    Piped(String, Vec<String>),
    Defined(String),
}

struct Check {
    id: String,
    actions: Vec<Action>,
    expect: Vec<String>,
}

fn call(name: &str, args: &[&str]) -> Action {
    Action::Call(name.to_owned(), args.iter().map(|s| s.to_string()).collect())
}

fn status(n: i32) -> String {
    format!("@@STATUS {n}")
}

struct Driver {
    f: Fam,
    lines: Vec<String>,
}

impl Driver {
    fn step(&mut self, id: &str) {
        let q = dq(self.f, &format!("@@STEP {id}"));
        self.lines.push(if self.f == Fam::Pwsh { q } else { format!("echo {q}") });
    }

    fn source(&mut self, path: &Path, out: &Path, err: &Path) {
        let (p, o, e) = (path.to_str().unwrap(), out.to_str().unwrap(), err.to_str().unwrap());
        let (p, o, e) = (dq(self.f, p), dq(self.f, o), dq(self.f, e));
        // fish reports some errors on the process's own stderr, past `2>file`, so in the
        // non-interactive shells that stderr is bracketed and must stay empty between markers.
        // Interactive bash and zsh echo typed lines there, and `2>file` already holds their errors.
        let mark = |m: &str| match self.f {
            Fam::Pwsh => format!("[Console]::Error.WriteLine('{m}')"),
            Fam::Fish => format!("echo '{m}' >&2"),
            Fam::Posix => String::new(),
        };
        let (begin, end) = (mark(ERR_BEGIN), mark(ERR_END));
        self.lines.push(begin);
        self.lines.push(match self.f {
            Fam::Posix => format!(". {p} >{o} 2>{e}"),
            Fam::Fish => format!("source {p} >{o} 2>{e}"),
            Fam::Pwsh => format!(". {p} *> {o}"),
        });
        self.lines.push(end);
    }

    fn action(&mut self, a: &Action) {
        let f = self.f;
        let words = |name: &str, args: &[String]| {
            std::iter::once(name.to_owned()).chain(args.iter().map(|a| dq(f, a))).collect::<Vec<_>>().join(" ")
        };
        match a {
            Action::Call(name, args) => {
                if f == Fam::Pwsh {
                    self.lines.push("$global:LASTEXITCODE = 0".into());
                }
                self.lines.push(words(name, args));
                self.lines.push(
                    match f {
                        Fam::Posix => "echo \"@@STATUS $?\"",
                        Fam::Fish => "echo \"@@STATUS $status\"",
                        Fam::Pwsh => "\"@@STATUS $LASTEXITCODE\"",
                    }
                    .into(),
                );
            }
            Action::Cd(path) => self.lines.push(match f {
                Fam::Pwsh => format!("Set-Location -LiteralPath {}", dq(f, path)),
                _ => format!("cd {}", dq(f, path)),
            }),
            Action::Piped(name, args) => self.lines.push(match f {
                Fam::Pwsh => format!("'piped' | {}", words(name, args)),
                _ => format!("printf 'piped\\n' | {}", words(name, args)),
            }),
            Action::Defined(name) => self.lines.push(match f {
                Fam::Posix => format!("if type {name} >/dev/null 2>&1; then echo @@PRESENT; else echo @@ABSENT; fi"),
                Fam::Fish => format!("if functions -q {name}; or abbr -q {name}; echo @@PRESENT; else; echo @@ABSENT; end"),
                Fam::Pwsh => format!(
                    "if (Get-Command -Name {} -CommandType Function,Alias -ErrorAction Ignore) {{ '@@PRESENT' }} else {{ '@@ABSENT' }}",
                    dq(f, name)
                ),
            }),
        }
    }
}

/// Options and pre-state a real rc file might already have set before sourcing.
fn prelude(c: &Config, function_names: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    match c.shell {
        Shell::Bash => lines.push("set +H".to_owned()),
        Shell::Zsh => lines.push("setopt nobanghist".to_owned()),
        _ => {}
    }
    if !c.hostile {
        return lines;
    }
    match c.shell {
        Shell::Bash => lines.push("set -u; shopt -s extglob failglob".into()),
        Shell::Zsh => lines.push(
            "setopt rcquotes ignorebraces ksharrays shwordsplit extendedglob globsubst nounset magicequalsubst noclobber"
                .into(),
        ),
        Shell::Pwsh => lines.push("Set-StrictMode -Version Latest; $ErrorActionPreference = 'Stop'".into()),
        Shell::Fish => {}
    }
    for n in function_names {
        lines.push(match c.shell {
            Shell::Pwsh => format!("Set-Alias -Name {n} -Value Get-Date -Force"),
            _ => format!("alias {n}='echo HIJACKED'"),
        });
    }
    lines
}

struct Run {
    sections: HashMap<String, Vec<String>>,
    /// Everything sourcing printed, first and second time.
    source_output: String,
    canary: Option<String>,
    raw: String,
}

fn run_driver(
    c: &Config,
    bin: &Path,
    dir: &Path,
    env: &[(String, String)],
    compiled: &str,
    function_names: &[String],
    checks: &[Check],
) -> Run {
    let f = fam(c.shell);
    let ext = c.shell.extension();
    let compiled_path = dir.join(format!("compiled.{ext}"));
    fs::write(&compiled_path, compiled).unwrap();
    let canary = dir.join("canary");
    let _ = fs::remove_file(&canary);

    let mut d = Driver { f, lines: prelude(c, function_names) };
    d.step("path-before");
    d.action(&call("envprobe", &["PATH", "MANPATH"]));
    let outs: Vec<PathBuf> = (0..4).map(|i| dir.join(format!("source-{i}.txt"))).collect();
    d.source(&compiled_path, &outs[0], &outs[1]);
    d.step("path-after-1");
    d.action(&call("envprobe", &["PATH", "MANPATH"]));
    d.source(&compiled_path, &outs[2], &outs[3]);
    d.step("path-after-2");
    d.action(&call("envprobe", &["PATH", "MANPATH"]));
    for ch in checks {
        d.step(&ch.id);
        for a in &ch.actions {
            d.action(a);
        }
    }
    d.step("end");
    let driver = dir.join(format!("driver.{}", if f == Fam::Pwsh { "ps1" } else { "txt" }));
    fs::write(&driver, d.lines.join("\n") + "\n").unwrap();

    let (stdout_path, stderr_path) = (dir.join("stdout.txt"), dir.join("stderr.txt"));
    let mut cmd = Command::new(bin);
    cmd.args(c.args).env_clear().envs(env.iter().map(|(k, v)| (k, v))).env("CANARY", &canary).current_dir(dir);
    cmd.stdout(fs::File::create(&stdout_path).unwrap()).stderr(fs::File::create(&stderr_path).unwrap());
    // bash and zsh read the driver as an interactive shell reads typed lines, so aliases expand.
    if f == Fam::Posix {
        cmd.stdin(fs::File::open(&driver).unwrap());
    } else {
        cmd.arg(&driver).stdin(Stdio::null());
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(300);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let raw = fs::read_to_string(&stdout_path).unwrap_or_default();
    let mut sections: HashMap<String, Vec<String>> = HashMap::new();
    let mut cur: Option<String> = None;
    for line in raw.lines() {
        if let Some(id) = line.strip_prefix("@@STEP ") {
            cur = Some(id.to_owned());
            sections.insert(id.to_owned(), Vec::new());
        } else if line.starts_with("@@") {
            if let Some(id) = &cur {
                sections.get_mut(id).unwrap().push(line.to_owned());
            }
        }
    }
    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    let mut source_output = outs.iter().map(|p| fs::read_to_string(p).unwrap_or_default()).collect::<String>();
    for chunk in stderr.split(ERR_BEGIN).skip(1) {
        source_output.push_str(chunk.split(ERR_END).next().unwrap_or(chunk).trim_start_matches('\n'));
    }
    let raw = raw + "\n--- stderr ---\n" + &stderr;
    Run { sections, source_output, canary: fs::read_to_string(&canary).ok(), raw }
}

struct Fixture {
    home: String,
    root: String,
}

fn resolve(w: &Word, home: &str) -> String {
    match w {
        Word::Literal(t) => t.as_str().to_owned(),
        Word::Home(t) => format!("{home}{}", t.as_str()),
    }
}

fn normalize(p: &Path) -> String {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out.to_string_lossy().into_owned()
}

fn env_line(name: &str, value: Option<&str>) -> String {
    match value {
        Some(v) => format!("@@ENV {name} {}", hex(v.as_bytes())),
        None => format!("@@ENV {name} UNSET"),
    }
}

fn cwd_line(p: &str) -> String {
    format!("@@CWD {}", hex(p.as_bytes()))
}

fn emitted(c: &Compiled, name: &str) -> bool {
    c.entries.iter().any(|r| r.entry.name.to_string() == name && r.outcome == Outcome::Emitted)
}

/// Generic checks for every alias and env entry, derived from the model and the report.
fn model_checks(pack: &Pack, compiled: &Compiled, fx: &Fixture, secrets: &HashMap<&str, Option<&str>>) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut env_names = Vec::new();
    let mut env_expect = Vec::new();
    for (g, e) in pack.entries() {
        let name = e.name().to_string();
        let report = compiled.entries.iter().find(|r| r.entry.group == g.name && r.entry.name == e.name()).unwrap();
        let shadowed = matches!(report.outcome, Outcome::Omitted(OmitReason::ShadowedBy(_)));
        if g.name.as_str() == "hijack" {
            continue;
        }
        match e {
            Entry::Alias(a) if report.outcome == Outcome::Emitted => {
                let program = resolve(a.argv.program(), &fx.home);
                let args: Vec<String> = a.argv.args().iter().map(|w| resolve(w, &fx.home)).collect();
                if program == "cd" {
                    let start = format!("{}/a/b", fx.root);
                    let target = normalize(&Path::new(&start).join(&args[0]));
                    checks.push(Check {
                        id: format!("alias {name} changes directory"),
                        actions: vec![Action::Cd(start), call(&name, &[]), call("cwdprobe", &[])],
                        expect: vec![status(0), cwd_line(&target), status(0)],
                    });
                    continue;
                }
                assert!(!program.contains('/'), "alias {name}: the harness stubs only bare program names");
                let fixed: Vec<String> = std::iter::once(program).chain(args).collect();
                let extra = if g.name.as_str() == "fuzz" || g.name.as_str() == "names" { &[][..] } else { &EXTRA[..] };
                let mut want = fixed.clone();
                want.extend(extra.iter().map(|s| s.to_string()));
                checks.push(Check {
                    id: format!("alias {name} passes exact argv"),
                    actions: vec![call(&name, extra)],
                    expect: vec![argv_line(&want), status(0)],
                });
            }
            Entry::Alias(_) | Entry::Function(_) if !report.outcome.is_omitted() || shadowed => {}
            Entry::Alias(_) | Entry::Function(_) => {
                if matches!(report.outcome, Outcome::Omitted(OmitReason::NoBodyFor { .. })) {
                    checks.push(Check {
                        id: format!("omitted {name} is not defined"),
                        actions: vec![Action::Defined(name.clone())],
                        expect: vec!["@@ABSENT".into()],
                    });
                }
            }
            Entry::Env(env) if !shadowed => match env.value() {
                EnvValue::Set(w) => {
                    env_names.push(name.clone());
                    env_expect.push(env_line(&name, Some(&resolve(w, &fx.home))));
                }
                EnvValue::Secret { .. } => {
                    env_names.push(name.clone());
                    let want = secrets.get(name.as_str()).copied().flatten();
                    env_expect.push(env_line(&name, if report.outcome == Outcome::Emitted { want } else { None }));
                }
                EnvValue::PathList { .. } => {}
            },
            Entry::Env(_) => {}
        }
    }
    let names: Vec<&str> = env_names.iter().map(String::as_str).collect();
    env_expect.push(status(0));
    checks.push(Check {
        id: "env and secrets hold exact bytes".into(),
        actions: vec![call("envprobe", &names)],
        expect: env_expect,
    });
    checks
}

/// `prepend` and `append` with ensure-present semantics, from the model.
fn path_after(pack: &Pack, compiled: &Compiled, var: &str, before: Option<String>, home: &str) -> Option<String> {
    let mut list: Vec<String> = before.map(|b| b.split(':').map(str::to_owned).collect()).unwrap_or_default();
    let mut touched = false;
    for (g, e) in pack.entries() {
        let Entry::Env(env) = e else { continue };
        let EnvValue::PathList { prepend, append } = env.value() else { continue };
        let emitted = compiled
            .entries
            .iter()
            .any(|r| r.entry.group == g.name && r.entry.name == e.name() && r.outcome == Outcome::Emitted);
        if env.name().as_str() != var || !emitted {
            continue;
        }
        touched = true;
        for w in prepend.iter().rev() {
            let v = resolve(w, home);
            if !list.contains(&v) {
                list.insert(0, v);
            }
        }
        for w in append {
            let v = resolve(w, home);
            if !list.contains(&v) {
                list.push(v);
            }
        }
    }
    (touched || !list.is_empty()).then(|| list.join(":"))
}

fn decode_env(line: Option<&String>) -> Option<String> {
    let v = line?.rsplit(' ').next()?;
    if v == "UNSET" {
        return None;
    }
    let bytes: Vec<u8> = (0..v.len()).step_by(2).map(|i| u8::from_str_radix(&v[i..i + 2], 16).unwrap()).collect();
    Some(String::from_utf8(bytes).unwrap())
}

fn example_checks(compiled: &Compiled, fx: &Fixture) -> Vec<Check> {
    let mut checks = Vec::new();
    let joined = EXTRA.join(" ");
    checks.push(Check {
        id: "script gcm forwards every argument".into(),
        actions: vec![call("gcm", &EXTRA)],
        expect: vec![argv_line(&["git", "commit", "-a", "-m", &joined]), status(0)],
    });
    checks.push(Check {
        id: "script gcm exits 2 with no arguments".into(),
        actions: vec![call("gcm", &[])],
        expect: vec![status(2)],
    });
    let made = format!("{}/new dir/it's \u{2018}x\u{2019}", fx.root);
    checks.push(Check {
        id: "in-shell mkcd creates and enters".into(),
        actions: vec![Action::Cd(fx.root.clone()), call("mkcd", &[&made]), call("cwdprobe", &[])],
        expect: vec![status(0), cwd_line(&made), status(0)],
    });
    if emitted(compiled, "groot") {
        checks.push(Check {
            id: "in-shell groot enters the toplevel".into(),
            actions: vec![Action::Cd(format!("{}/a/b", fx.root)), call("groot", &[]), call("cwdprobe", &[])],
            expect: vec![status(0), cwd_line(&format!("{}/top level", fx.root)), status(0)],
        });
    }
    for alias in ["gs", "g"] {
        checks.push(Check {
            id: format!("alias {alias} forwards piped stdin"),
            actions: vec![Action::Piped(alias.into(), vec!["--probe-stdin".into()])],
            expect: vec![format!("@@STDIN {}", hex(b"piped\n"))],
        });
    }
    checks
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Every ASCII metacharacter, tab, CR, LF, the pwsh single quotes, smart double quotes, NBSP,
/// U+2028, BOM, zero-width space, an astral code point, and a fish private-use code point.
fn hazard_words(n: usize) -> Vec<String> {
    let alphabet: Vec<char> = "'\"$`\\!%*?[]{}()<>|&;#~=,:@^ \t\n\r-abcXYZ019_./+\u{2018}\u{2019}\u{201a}\u{201b}\u{201c}\u{201d}\u{201e}\u{201f}\u{2014}\u{276f}\u{e9}\u{1d11e}\u{a0}\u{2028}\u{feff}\u{200b}\u{f600}"
        .chars()
        .collect();
    let prefixes = ["%self", "=ls", "a==ls", "x:=ls", "-", "--", "~", "~/", "@", "007"];
    let mut r = Rng(FUZZ_SEED);
    (0..n)
        .map(|_| {
            let mut s = String::new();
            if r.below(4) == 0 {
                s.push_str(prefixes[r.below(prefixes.len())]);
            }
            for _ in 0..r.below(13) {
                s.push(alphabet[r.below(alphabet.len())]);
            }
            s
        })
        .collect()
}

fn quote_vector_inputs() -> Vec<String> {
    include_str!("quote_vectors.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let h = l.split('\t').next().unwrap();
            String::from_utf8((0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect())
                .unwrap()
        })
        .collect()
}

fn toml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const ODD_NAMES: &[&str] = &["a.b", "a+b", "x-y", "1x", "_u", "...", "..x", "_"];
/// Every name some backend reserves; each shell must reserve or run it correctly.
#[rustfmt::skip]
const RESERVED_UNION: &[&str] = &[
    "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in", "select", "then",
    "time", "until", "while", "declare", "end", "export", "float", "foreach", "integer", "local", "nocorrect",
    "noglob", "readonly", "repeat", "typeset", "emulate", "alias", "command", "eval", "unalias", "unset", "abbr",
    "and", "argparse", "begin", "break", "builtin", "contains", "continue", "exec", "not", "or", "read", "return",
    "set", "status", "string", "switch", "test", "catch", "class", "clean", "data", "define", "dynamicparam",
    "elseif", "enum", "exit", "filter", "finally", "from", "hidden", "inlinescript", "parallel", "param", "process",
    "sequence", "throw", "trap", "try", "using", "var", "workflow", "Get-Command", "Join-Path", "Remove-Alias",
    "Set-Alias", "Write-Warning",
];
/// Commands some compiled file runs. Aliased to `canary`, none
/// of them may run while the file is sourced twice.
#[rustfmt::skip]
const HIJACK: &[&str] = &[
    "alias", "command", "eval", "export", "unalias", "unset", "emulate", "abbr", "contains", "Get-Command",
    "Join-Path", "Remove-Alias", "Set-Alias", "Write-Warning", "cat",
];

fn fixture_pack() -> (String, String) {
    let mut t = String::from(
        "format = 1\n[pack]\nname = \"fixture\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"harness fixture\"\n",
    );
    t.push_str("[groups.env.env]\nMANPATH = { prepend = [\"/m1\"], append = [\"~/m2\"] }\n");
    for s in ["S_FOUND", "S_MISSING", "S_EMPTY", "S_NOCMD", "S_FILE", "S_UNBOUND"] {
        let _ = writeln!(t, "{s} = {{ secret = \"for the harness\" }}");
    }
    let vectors = quote_vector_inputs();
    let fuzz = hazard_words(FUZZ_COUNT);
    for (i, v) in vectors.iter().enumerate() {
        let _ = writeln!(t, "QV_{i} = {}", toml_str(v));
    }
    for (i, v) in fuzz.iter().enumerate() {
        let _ = writeln!(t, "FZ_{i} = {}", toml_str(v));
    }
    t.push_str(concat!(
        "[groups.fns.functions.argdump_sh]\nscript = 'exec git \"$@\"'\n",
        "[groups.fns.functions.argdump_in]\nshell.posix = 'git \"$@\"'\nshell.fish = 'git $argv'\nshell.pwsh = 'git @args'\n",
        "[groups.fns.functions.setvar]\nshell.posix = 'export INSHELL_SET=\"$1\"'\n",
        "shell.fish = 'set -gx INSHELL_SET $argv[1]'\nshell.pwsh = '$env:INSHELL_SET = $args[0]'\n",
        "[groups.fns.functions.ret3]\nshell.posix = 'return 3'\nshell.fish = 'return 3'\n",
        "[groups.fns.functions.splitarg]\nshell.posix = 'git $1'\n",
        "shell.fish = 'git (string split \" \" -- $argv[1])'\nshell.pwsh = 'git ($args[0] -split \" \")'\n",
        "[groups.breakout.functions.breakout]\n",
        "shell.posix = \"echo hi\\n}\\ntouch \\\"$CANARY\\\"\\nevil() {\\necho x\"\n",
        "shell.fish = \"echo hi\\nend\\ntouch $CANARY\\nfunction evil\\necho x\"\n",
        "shell.pwsh = \"'hi'\\n}\\nNew-Item -ItemType File -Path $env:CANARY | Out-Null\\nfunction evil {\\n'x'\"\n",
        "[groups.breakout.functions.breakout_sh]\n",
        "description = \"'; touch \\\"$CANARY\\\"; echo '\"\n",
        "script = \"x'\\n} ; touch \\\"$CANARY\\\" ; '\"\n",
        "[groups.breakout.aliases]\n",
        "breakout_alias = [\"git\", \"'; touch $CANARY; '\", \"$(touch $CANARY)\", \"`touch $CANARY`\", \"(touch $CANARY)\", \"$(New-Item $env:CANARY)\", \"'+(New-Item $env:CANARY)+'\"]\n",
    ));
    t.push_str("[groups.fuzz.aliases]\n");
    for (i, v) in vectors.iter().enumerate() {
        let _ = writeln!(t, "qv{i} = [\"git\", {}]", toml_str(v));
    }
    for (i, v) in fuzz.iter().enumerate() {
        let _ = writeln!(t, "fz{i} = [\"git\", {}, \"end\"]", toml_str(v));
    }
    for (i, v) in fuzz.iter().enumerate() {
        let body = format!("exec git \"$@\" '{}'", v.replace('\'', "'\\''"));
        let _ = writeln!(t, "[groups.fuzz.functions.fzs{i}]\nscript = {}", toml_str(&body));
        if !v.is_empty() {
            let _ = writeln!(t, "description = {}", toml_str(v));
        }
    }
    t.push_str("[groups.zafter.aliases]\nafter_breakout = \"git after\"\n");

    let bindings = concat!(
        "S_FOUND = { command = [\"secretprobe\", \"found\"] }\n",
        "S_MISSING = { command = [\"secretprobe\", \"missing\"] }\n",
        "S_EMPTY = { command = [\"secretprobe\", \"empty\"] }\n",
        "S_NOCMD = { command = [\"alias-deck-no-such-command\"] }\n",
        "S_FILE = { file = \"~/secret file 'x'.txt\" }\n",
    )
    .to_owned();
    (t, bindings)
}

/// Aliases named after keywords, builtins, and the commands compiled files run. Kept apart from
/// the fixture because an alias named `return` or `set` legitimately changes later shell code.
fn names_pack() -> String {
    let mut t = String::from(concat!(
        "format = 1\n[pack]\nname = \"names\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"d\"\n",
        "[groups.env.env]\nMANPATH = { prepend = [\"/m1\"] }\nE = \"~/e\"\nS_FOUND = { secret = \"s\" }\n",
        "[groups.fns.functions.sh_fn]\nscript = 'exit 0'\n",
        "[groups.fns.functions.in_fn]\nshell.posix = ':'\nshell.fish = 'true'\nshell.pwsh = '$null'\n",
    ));
    // A function, not an alias, shadows the lookup program: bash and zsh do not expand an
    // alias inside `$(...)` here, but they do call a function.
    t.push_str("[groups.lookup.functions.secretprobe]\nscript = 'exec canary secretprobe'\n");
    t.push_str("[groups.hijack.aliases]\n");
    for n in HIJACK {
        let _ = writeln!(t, "{} = [\"canary\", {}]", toml_str(n), toml_str(n));
    }
    t.push_str("[groups.names.aliases]\n");
    for n in ODD_NAMES.iter().chain(RESERVED_UNION) {
        let _ = writeln!(t, "{} = [\"git\", {}]", toml_str(n), toml_str(n));
    }
    t
}

fn fixture_checks(pack: &Pack, compiled: &Compiled, fx: &Fixture) -> Vec<Check> {
    let mut checks = Vec::new();
    let fuzz = hazard_words(FUZZ_COUNT);
    let mut want = vec!["git".to_owned()];
    want.extend(EXTRA.iter().map(|s| s.to_string()));
    checks.push(Check {
        id: "script function forwards every argument".into(),
        actions: vec![call("argdump_sh", &EXTRA)],
        expect: vec![argv_line(&want), status(0)],
    });
    checks.push(Check {
        id: "in-shell function forwards every argument".into(),
        actions: vec![call("argdump_in", &EXTRA)],
        expect: vec![argv_line(&want), status(0)],
    });
    let value = "it's $x \u{2018}q\u{2019} \"d\"";
    checks.push(Check {
        id: "in-shell function exports into the caller".into(),
        actions: vec![call("setvar", &[value]), call("envprobe", &["INSHELL_SET"])],
        expect: vec![status(0), env_line("INSHELL_SET", Some(value)), status(0)],
    });
    if emitted(compiled, "ret3") {
        checks.push(Check {
            id: "in-shell return status propagates".into(),
            actions: vec![call("ret3", &[])],
            expect: vec![status(3)],
        });
    }
    checks.push(Check {
        id: "posix bodies run with sh word splitting".into(),
        actions: vec![call("splitarg", &["a b"])],
        expect: vec![argv_line(&["git", "a", "b"]), status(0)],
    });
    checks.push(Check {
        id: "breakout bodies stay inside their wrapper".into(),
        actions: vec![
            Action::Defined("breakout".into()),
            Action::Defined("breakout_sh".into()),
            Action::Defined("evil".into()),
            call("after_breakout", &[]),
        ],
        expect: vec![
            "@@PRESENT".into(),
            "@@PRESENT".into(),
            "@@ABSENT".into(),
            argv_line(&["git", "after"]),
            status(0),
        ],
    });
    for (i, v) in fuzz.iter().enumerate() {
        checks.push(Check {
            id: format!("fuzz script body {i}"),
            actions: vec![call(&format!("fzs{i}"), &[])],
            expect: vec![argv_line(&["git", v]), status(0)],
        });
    }
    let secrets: HashMap<&str, Option<&str>> = [
        ("S_FOUND", Some(SECRET)),
        ("S_FILE", Some(SECRET)),
        ("S_MISSING", None),
        ("S_EMPTY", None),
        ("S_NOCMD", None),
        ("S_UNBOUND", None),
    ]
    .into_iter()
    .collect();
    checks.extend(model_checks(pack, compiled, fx, &secrets));
    checks
}

struct ConfigResult {
    label: String,
    lines: Vec<String>,
    failures: Vec<String>,
}

fn evaluate(
    suite: &str,
    run: &Run,
    checks: &[Check],
    pack: &Pack,
    compiled: &Compiled,
    home: &str,
    failures: &mut Vec<String>,
) -> usize {
    let mut n = 0;
    let mut fail = |what: String| failures.push(format!("{suite}: {what}"));
    n += 1;
    if !run.source_output.is_empty() {
        fail(format!("sourcing printed output: {:?}", run.source_output));
    }
    n += 1;
    if let Some(c) = &run.canary {
        fail(format!("pack code ran: {c}"));
    }
    let get = |id: &str, i: usize| run.sections.get(id).and_then(|s| s.get(i));
    for (i, var) in ["PATH", "MANPATH"].into_iter().enumerate() {
        let before = decode_env(get("path-before", i));
        let want = path_after(pack, compiled, var, before, home);
        for step in ["path-after-1", "path-after-2"] {
            n += 1;
            let got = decode_env(get(step, i));
            if got != want {
                fail(format!("{var} at {step}: want {want:?}, got {got:?}"));
            }
        }
    }
    for ch in checks {
        n += 1;
        let got = run.sections.get(&ch.id);
        if got != Some(&ch.expect) {
            fail(format!("{}\n      want {:?}\n      got  {:?}", ch.id, ch.expect, got));
        }
    }
    n += 1;
    if !run.sections.contains_key("end") {
        fail(format!("the driver did not finish; output:\n{}", run.raw.chars().take(3000).collect::<String>()));
    }
    n
}

#[cfg(unix)]
fn link_probe(exe: &Path, link: &Path) {
    std::os::unix::fs::symlink(exe, link).unwrap();
}

#[cfg(not(unix))]
fn link_probe(exe: &Path, link: &Path) {
    let link = link.with_extension("exe");
    if fs::hard_link(exe, &link).is_err() {
        fs::copy(exe, &link).unwrap();
    }
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

/// Emitted functions, which the hostile pre-state shadows with aliases beforehand.
fn function_names(pack: &Pack, compiled: &Compiled) -> Vec<String> {
    pack.entries()
        .filter(|(_, e)| matches!(e, Entry::Function(_)) && emitted(compiled, &e.name().to_string()))
        .map(|(_, e)| e.name().to_string())
        .collect()
}

fn run_config(c: &Config, bin: &Path, version: &str, work: &Path, idx: usize) -> ConfigResult {
    let dir = work.join(format!("cfg{idx}"));
    let home = dir.join("home dir $x 'q'");
    let root = dir.join("root");
    for p in [home.join(".dotfiles"), root.join("a/b"), root.join("top level")] {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(home.join("secret file 'x'.txt"), format!("{SECRET}\n\n")).unwrap();
    let fx = Fixture { home: home.to_str().unwrap().to_owned(), root: root.to_str().unwrap().to_owned() };
    let stub = work.join("stub");
    let env: Vec<(String, String)> = vec![
        ("HOME".into(), fx.home.clone()),
        ("PATH".into(), format!("{}:/usr/bin:/bin", stub.display())),
        ("USER".into(), "tester".into()),
        ("LANG".into(), "en_US.UTF-8".into()),
        ("LC_ALL".into(), "en_US.UTF-8".into()),
        ("TERM".into(), "dumb".into()),
        ("TMPDIR".into(), work.join("tmp").to_str().unwrap().into()),
        ("PROBE_TOPLEVEL".into(), format!("{}/top level", fx.root)),
    ];
    let target = Target { shell: c.shell, os: host_os() };
    let mut failures = Vec::new();
    let mut lines = Vec::new();

    let example_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/devkit.toml")).unwrap();
    let example = PackDocument::parse(&example_src).unwrap().pack().clone();
    let bindings = Bindings::parse("GITHUB_TOKEN = { command = [\"secretprobe\", \"found\"] }").unwrap();
    let compiled = compile(&Deck::new(vec![example.clone()]), &bindings, target, &CompileOptions::default());
    let secrets: HashMap<&str, Option<&str>> =
        [("GITHUB_TOKEN", Some(SECRET)), ("NPM_TOKEN", None)].into_iter().collect();
    let mut checks = example_checks(&compiled, &fx);
    checks.extend(model_checks(&example, &compiled, &fx, &secrets));
    let ex_dir = dir.join("example");
    fs::create_dir_all(&ex_dir).unwrap();
    let names = function_names(&example, &compiled);
    let run = run_driver(c, bin, &ex_dir, &env, &compiled.source, &names, &checks);
    let before = failures.len();
    let n = evaluate("example", &run, &checks, &example, &compiled, &fx.home, &mut failures);
    lines.push(format!("example {}/{n}", n - (failures.len() - before)));

    let (fixture_src, fixture_bindings) = fixture_pack();
    let fixture = PackDocument::parse(&fixture_src).unwrap_or_else(|e| panic!("fixture pack: {e:?}")).pack().clone();
    let fb = Bindings::parse(&fixture_bindings).unwrap();
    let compiled = compile(&Deck::new(vec![fixture.clone()]), &fb, target, &CompileOptions::default());
    let checks = fixture_checks(&fixture, &compiled, &fx);
    let fx_dir = dir.join("fixture");
    fs::create_dir_all(&fx_dir).unwrap();
    let names = function_names(&fixture, &compiled);
    let run = run_driver(c, bin, &fx_dir, &env, &compiled.source, &names[..2], &checks);
    let before = failures.len();
    let n = evaluate("fixture", &run, &checks, &fixture, &compiled, &fx.home, &mut failures);
    lines.push(format!("fixture {}/{n}", n - (failures.len() - before)));

    let names = PackDocument::parse(&names_pack()).unwrap_or_else(|e| panic!("names pack: {e:?}")).pack().clone();
    let nb = Bindings::parse("S_FOUND = { command = [\"secretprobe\", \"found\"] }").unwrap();
    let compiled = compile(&Deck::new(vec![names.clone()]), &nb, target, &CompileOptions::default());
    let secrets: HashMap<&str, Option<&str>> = [("S_FOUND", Some(SECRET))].into_iter().collect();
    let checks = model_checks(&names, &compiled, &fx, &secrets);
    let nm_dir = dir.join("names");
    fs::create_dir_all(&nm_dir).unwrap();
    let run = run_driver(c, bin, &nm_dir, &env, &compiled.source, &[], &checks);
    let before = failures.len();
    let n = evaluate("names", &run, &checks, &names, &compiled, &fx.home, &mut failures);
    lines.push(format!("names {}/{n}", n - (failures.len() - before)));

    if c.shell == Shell::Fish {
        let opts = CompileOptions { fish_aliases: FishAliasStyle::Abbr };
        let compiled = compile(&Deck::new(vec![example.clone()]), &bindings, target, &opts);
        let checks: Vec<Check> = example
            .entries()
            .filter(|(_, e)| matches!(e, Entry::Alias(_)) && emitted(&compiled, &e.name().to_string()))
            .map(|(_, e)| Check {
                id: format!("abbr {} is defined", e.name()),
                actions: vec![Action::Defined(e.name().to_string())],
                expect: vec!["@@PRESENT".into()],
            })
            .collect();
        let ab_dir = dir.join("abbr");
        fs::create_dir_all(&ab_dir).unwrap();
        let run = run_driver(c, bin, &ab_dir, &env, &compiled.source, &[], &checks);
        let before = failures.len();
        let n = evaluate("abbr", &run, &checks, &example, &compiled, &fx.home, &mut failures);
        lines.push(format!("abbr {}/{n}", n - (failures.len() - before)));
    }
    ConfigResult { label: format!("{} ({version})", c.label), lines, failures }
}

/// The shells this run must exercise, from `ALIAS_DECK_REQUIRE_SHELLS` (a comma-separated list)
/// or `ALIAS_DECK_REQUIRE_ALL_SHELLS=1`. An unknown name panics, so a typo in CI cannot quietly
/// require nothing.
fn required_shells() -> Vec<Shell> {
    if env::var("ALIAS_DECK_REQUIRE_ALL_SHELLS").is_ok_and(|v| v == "1") {
        return Shell::ALL.to_vec();
    }
    let list = env::var("ALIAS_DECK_REQUIRE_SHELLS").unwrap_or_default();
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| Shell::from_key(s).unwrap_or_else(|| panic!("ALIAS_DECK_REQUIRE_SHELLS: unknown shell `{s}`")))
        .collect()
}

fn harness() -> ExitCode {
    let required = required_shells();
    if cfg!(windows) {
        println!("real-shell harness: not run on Windows; the CLI's tests source the built files in pwsh and bash");
        return ExitCode::SUCCESS;
    }
    let work = env::temp_dir().join(format!("alias-deck-shells-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(work.join("stub")).unwrap();
    fs::create_dir_all(work.join("tmp")).unwrap();
    let work = work.canonicalize().unwrap();
    let exe = env::current_exe().unwrap();
    for name in PROBES.iter().chain(&["git", "sudo", "grep"]) {
        link_probe(&exe, &work.join("stub").join(name));
    }

    let mut handles = Vec::new();
    let mut skipped = Vec::new();
    for (idx, c) in CONFIGS.iter().enumerate() {
        match find_shell(c) {
            None => skipped.push(c),
            Some((bin, version)) => {
                let work = work.clone();
                handles.push(thread::spawn(move || run_config(c, &bin, &version, &work, idx)));
            }
        }
    }
    let mut failed = 0;
    let mut total_lines = Vec::new();
    for h in handles {
        let o = h.join().expect("a config thread panicked");
        total_lines.push(format!("  {:<44} {}", o.label, o.lines.join(", ")));
        for f in o.failures.iter().take(15) {
            println!("FAIL [{}] {f}", o.label);
        }
        failed += o.failures.len();
    }
    println!("real-shell harness:");
    for l in &total_lines {
        println!("{l}");
    }
    for c in &skipped {
        println!("  {:<44} SKIPPED: not installed", c.label);
    }
    if failed == 0 {
        let _ = fs::remove_dir_all(&work);
    }
    if failed > 0 {
        println!("{failed} failed checks; work dir kept at {}", work.display());
        return ExitCode::FAILURE;
    }
    let ran = |shell: Shell| CONFIGS.iter().any(|c| c.shell == shell && !skipped.iter().any(|s| s.label == c.label));
    let missing: Vec<&str> = required.iter().filter(|s| !ran(**s)).map(|s| s.key()).collect();
    if !missing.is_empty() {
        println!("required shells did not run: {}", missing.join(", "));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
