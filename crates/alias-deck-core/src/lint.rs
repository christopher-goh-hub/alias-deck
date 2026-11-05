//! Safety lint for packs from strangers. Pure and deterministic: one table of rules, each a
//! matcher over one entry. Body rules are text heuristics. They flag code for a person to read
//! and never prove it safe; the preview shows every body in full.

use std::fmt;

use crate::model::{Entry, EntryRef, EnvValue, FunctionBody, Pack, Word};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warn,
    Danger,
}

impl Severity {
    pub fn key(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Danger => "danger",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// A rule's id is stable: users name it in `--allow <id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    EnvLoaderHijack,
    PathUnsafeDir,
    ShadowsSensitive,
    BodyPipeToShell,
    EnvTraffic,
    EnvHistory,
    BodyRisky,
    ShadowsCore,
    PathPrepend,
    NativeFunction,
    Secret,
}

impl Rule {
    pub fn id(self) -> &'static str {
        match self {
            Rule::EnvLoaderHijack => "env-loader-hijack",
            Rule::PathUnsafeDir => "path-unsafe-dir",
            Rule::ShadowsSensitive => "shadows-sensitive",
            Rule::BodyPipeToShell => "body-pipe-to-shell",
            Rule::EnvTraffic => "env-traffic",
            Rule::EnvHistory => "env-history",
            Rule::BodyRisky => "body-risky",
            Rule::ShadowsCore => "shadows-core",
            Rule::PathPrepend => "path-prepend",
            Rule::NativeFunction => "native-function",
            Rule::Secret => "secret",
        }
    }

    pub fn from_id(id: &str) -> Option<Rule> {
        RULES.iter().map(|r| r.0).find(|r| r.id() == id)
    }

    pub fn severity(self) -> Severity {
        RULES.iter().find(|r| r.0 == self).map(|r| r.1).expect("every rule is in the table")
    }

    pub fn all() -> impl Iterator<Item = Rule> {
        RULES.iter().map(|r| r.0)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: Rule,
    pub severity: Severity,
    pub entry: EntryRef,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}] {}: {}", self.severity, self.rule, self.entry, self.message)
    }
}

type Matcher = fn(&Entry) -> Option<String>;

/// The whole policy. Order is report order within one entry.
const RULES: [(Rule, Severity, Matcher); 11] = [
    (Rule::EnvLoaderHijack, Severity::Danger, env_loader_hijack),
    (Rule::PathUnsafeDir, Severity::Danger, path_unsafe_dir),
    (Rule::ShadowsSensitive, Severity::Danger, shadows_sensitive),
    (Rule::BodyPipeToShell, Severity::Danger, body_pipe_to_shell),
    (Rule::EnvTraffic, Severity::Warn, env_traffic),
    (Rule::EnvHistory, Severity::Warn, env_history),
    (Rule::BodyRisky, Severity::Warn, body_risky),
    (Rule::ShadowsCore, Severity::Info, shadows_core),
    (Rule::PathPrepend, Severity::Info, path_prepend),
    (Rule::NativeFunction, Severity::Info, native_function),
    (Rule::Secret, Severity::Info, secret),
];

/// Findings in entry order, then rule-table order.
pub fn lint(pack: &Pack) -> Vec<Finding> {
    let mut out = Vec::new();
    for (group, entry) in pack.entries() {
        for (rule, severity, check) in RULES {
            if let Some(message) = check(entry) {
                let entry = EntryRef { pack: pack.meta().name.clone(), group: group.name.clone(), name: entry.name() };
                out.push(Finding { rule, severity, entry, message });
            }
        }
    }
    out
}

const LOADER_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "BASH_ENV",
    "ENV",
    "ZDOTDIR",
    "PROMPT_COMMAND",
    "NODE_OPTIONS",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_EXEC_PATH",
    "GIT_CONFIG_GLOBAL",
    "SSH_ASKPASS",
    "SUDO_ASKPASS",
    "GIT_ASKPASS",
];

const UNSAFE_DIRS: &[&str] = &["/tmp", "/var/tmp", "/dev/shm", "/private/tmp"];

const SENSITIVE_COMMANDS: &[&str] = &[
    "sudo",
    "su",
    "doas",
    "ssh",
    "scp",
    "sftp",
    "ssh-add",
    "ssh-keygen",
    "gpg",
    "gpg2",
    "passwd",
    "login",
    "security",
    "op",
    "pass",
    "kinit",
    "secret-tool",
];

/// Compared case-insensitively: curl and friends read both spellings.
const PROXY_VARS: &[&str] = &["http_proxy", "https_proxy", "all_proxy", "no_proxy"];

const TRAFFIC_VARS: &[&str] = &[
    "SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_NO_VERIFY",
    "NODE_TLS_REJECT_UNAUTHORIZED",
];

const HISTORY_VARS: &[&str] = &["HISTFILE", "HISTSIZE", "HISTFILESIZE", "SAVEHIST", "HISTCONTROL", "HISTIGNORE"];

const CORE_COMMANDS: &[&str] = &[
    "ls", "cd", "rm", "cp", "mv", "cat", "grep", "find", "git", "curl", "make", "wget", "ln", "chmod", "chown",
    "mkdir", "rmdir", "touch", "echo", "printf", "test", "kill", "ps", "tar", "sed", "awk", "head", "tail", "sort",
    "diff", "less", "man", "which", "vi", "vim", "open", "python", "python3", "node", "npm", "docker", "kubectl",
];

const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch", "iwr", "irm", "invoke-webrequest", "invoke-restmethod"];

const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "iex", "invoke-expression"];

const RC_FILES: &[&str] =
    &[".bashrc", ".bash_profile", ".profile", ".zshrc", ".zprofile", ".zshenv", ".zlogin", "config.fish", "$profile"];

const HOME_TARGETS: &[&str] = &["/", "/*", "~", "~/", "~/*", "$HOME", "$HOME/", "$HOME/*", "${HOME}", "${HOME}/"];

fn env_loader_hijack(e: &Entry) -> Option<String> {
    let Entry::Env(env) = e else { return None };
    let name = env.name().as_str();
    (LOADER_VARS.contains(&name) || name.starts_with("DYLD_"))
        .then(|| format!("sets {name}, which can inject code into other programs you run"))
}

/// The words a PATH change puts on PATH. A plain `PATH = "..."` replaces the list, so its
/// elements are split out; an empty element means the current directory.
fn path_words(e: &Entry) -> Option<Vec<String>> {
    let Entry::Env(env) = e else { return None };
    if env.name().as_str() != "PATH" {
        return None;
    }
    match env.value() {
        EnvValue::PathList { prepend, append } => Some(prepend.iter().chain(append).map(Word::to_string).collect()),
        EnvValue::Set(w) => Some(w.to_string().split(':').map(str::to_owned).collect()),
        EnvValue::Secret { .. } => None,
    }
}

fn path_unsafe_dir(e: &Entry) -> Option<String> {
    let unsafe_dirs: Vec<String> = path_words(e)?
        .into_iter()
        .filter(|w| {
            let relative = !w.starts_with('/') && w != "~" && !w.starts_with("~/");
            let shared = UNSAFE_DIRS.iter().any(|d| w == d || w.starts_with(&format!("{d}/")));
            relative || shared
        })
        .collect();
    (!unsafe_dirs.is_empty()).then(|| {
        format!(
            "puts {} on PATH, a relative or world-writable location anyone could plant commands in",
            quoted_list(&unsafe_dirs)
        )
    })
}

fn command_name(e: &Entry) -> Option<&str> {
    match e {
        Entry::Alias(a) => Some(a.name.as_str()),
        Entry::Function(f) => Some(f.name.as_str()),
        Entry::Env(_) => None,
    }
}

fn shadows_sensitive(e: &Entry) -> Option<String> {
    let name = command_name(e).filter(|n| SENSITIVE_COMMANDS.contains(n))?;
    Some(format!("defines `{name}`, which handles passwords or keys and would see what you type into it"))
}

fn env_traffic(e: &Entry) -> Option<String> {
    let Entry::Env(env) = e else { return None };
    let name = env.name().as_str();
    let lower = name.to_ascii_lowercase();
    (PROXY_VARS.contains(&lower.as_str()) || TRAFFIC_VARS.contains(&name))
        .then(|| format!("sets {name}, which redirects network traffic or changes which certificates are trusted"))
}

fn env_history(e: &Entry) -> Option<String> {
    let Entry::Env(env) = e else { return None };
    let name = env.name().as_str();
    HISTORY_VARS.contains(&name).then(|| format!("sets {name}, which changes where or what your shell history keeps"))
}

fn shadows_core(e: &Entry) -> Option<String> {
    let name = command_name(e).filter(|n| CORE_COMMANDS.contains(n))?;
    Some(format!("defines `{name}`, which replaces a common command in your shell"))
}

fn path_prepend(e: &Entry) -> Option<String> {
    let Entry::Env(env) = e else { return None };
    if env.name().as_str() != "PATH" {
        return None;
    }
    match env.value() {
        EnvValue::PathList { prepend, .. } if !prepend.is_empty() => {
            let words: Vec<String> = prepend.iter().map(Word::to_string).collect();
            Some(format!("puts {} ahead of system tools on PATH", quoted_list(&words)))
        }
        EnvValue::Set(w) => Some(format!("replaces PATH with `{w}`")),
        _ => None,
    }
}

fn native_function(e: &Entry) -> Option<String> {
    let Entry::Function(f) = e else { return None };
    matches!(f.body, FunctionBody::InShell(_))
        .then(|| "runs inside your shell, so it can change your directory and variables".to_owned())
}

fn secret(e: &Entry) -> Option<String> {
    let Entry::Env(env) = e else { return None };
    match env.value() {
        EnvValue::Secret { purpose } => Some(format!("asks for a secret: {purpose}")),
        _ => None,
    }
}

fn bodies(e: &Entry) -> Vec<&str> {
    match e {
        Entry::Function(f) => match &f.body {
            FunctionBody::Script(t) => vec![t.as_str()],
            FunctionBody::InShell(b) => b.dialects().filter_map(|d| b.get(d)).map(|t| t.as_str()).collect(),
        },
        _ => Vec::new(),
    }
}

/// Splits a line into words and the punctuation that separates commands. Quotes are dropped, so
/// `"$HOME"` and `$HOME` read the same.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in line.chars() {
        if c.is_whitespace() || c == '"' || c == '\'' {
            out.extend((!word.is_empty()).then(|| std::mem::take(&mut word)));
        } else if "|;&()<>`".contains(c) {
            out.extend((!word.is_empty()).then(|| std::mem::take(&mut word)));
            out.push(c.to_string());
        } else {
            word.push(c);
        }
    }
    out.extend((!word.is_empty()).then_some(word));
    out
}

fn is_one_of(tok: &str, list: &[&str]) -> bool {
    list.contains(&tok.to_ascii_lowercase().as_str())
}

fn pipes_download_to_shell(line: &str) -> bool {
    let t = tokens(line);
    let Some(dl) = t.iter().position(|w| is_one_of(w, DOWNLOADERS)) else { return false };
    let piped = (dl..t.len()).any(|i| {
        let prev_is_pipe = i > 0 && t[i - 1] == "|";
        if t[i] != "|" || prev_is_pipe || t.get(i + 1).is_some_and(|n| n == "|") {
            return false;
        }
        let next = t[i + 1..].iter().find(|w| !matches!(w.as_str(), "sudo" | "env") && !w.starts_with('-'));
        next.is_some_and(|w| is_one_of(w, SHELLS))
    });
    let substituted = t[..dl].iter().position(|w| is_one_of(w, SHELLS) || w == "eval" || w == "source" || w == ".");
    piped || substituted.is_some_and(|s| t[s..dl].iter().any(|w| w == "("))
}

fn body_pipe_to_shell(e: &Entry) -> Option<String> {
    let lines: Vec<&str> =
        bodies(e).into_iter().flat_map(str::lines).map(str::trim).filter(|l| pipes_download_to_shell(l)).collect();
    (!lines.is_empty()).then(|| format!("runs code it downloads: {}", quoted_list(&lines)))
}

fn risky_in_line(line: &str) -> Vec<&'static str> {
    let t = tokens(line);
    let has = |w: &str| t.iter().any(|x| x == w);
    let after = |cmd: &str, pred: &dyn Fn(&str) -> bool| {
        t.iter().position(|x| x == cmd).is_some_and(|i| t[i + 1..].iter().any(|x| pred(x)))
    };
    let mut out = Vec::new();
    if has("eval") {
        out.push("eval");
    }
    if after("base64", &|x| matches!(x, "-d" | "-D" | "--decode")) {
        out.push("base64 -d");
    }
    if let Some(i) = t.iter().position(|x| x == "rm") {
        let args = &t[i + 1..];
        let flags: String =
            args.iter().filter(|a| a.starts_with('-') && !a.starts_with("--")).map(|a| &a[1..]).collect();
        let recursive = flags.contains(['r', 'R']) || args.iter().any(|a| a == "--recursive");
        let force = flags.contains('f') || args.iter().any(|a| a == "--force");
        if recursive && force && args.iter().any(|a| HOME_TARGETS.contains(&a.as_str())) {
            out.push("rm -rf on / or your home directory");
        }
    }
    if after("chmod", &|x| matches!(x, "777" | "0777" | "a+rwx")) {
        out.push("chmod 777");
    }
    if has("sudo") {
        out.push("sudo");
    }
    let into_rc = t.windows(2).any(|w| {
        w[0] == ">" && {
            let base = w[1].rsplit('/').next().unwrap_or(&w[1]).to_ascii_lowercase();
            RC_FILES.contains(&base.as_str())
        }
    });
    if into_rc {
        out.push("a redirect into a shell startup file");
    }
    out
}

fn body_risky(e: &Entry) -> Option<String> {
    let mut found: Vec<&str> = Vec::new();
    for hit in bodies(e).into_iter().flat_map(str::lines).flat_map(risky_in_line) {
        if !found.contains(&hit) {
            found.push(hit);
        }
    }
    (!found.is_empty()).then(|| format!("uses {}", found.join(", ")))
}

fn quoted_list<S: AsRef<str>>(items: &[S]) -> String {
    items.iter().map(|s| format!("`{}`", s.as_ref())).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackDocument;

    fn findings(body: &str) -> Vec<(Rule, Severity)> {
        let src = format!(
            "format = 1\n[pack]\nname = \"p\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"d\"\n{body}"
        );
        let doc = PackDocument::parse(&src).unwrap_or_else(|e| panic!("{body}: {e:?}"));
        lint(doc.pack()).into_iter().map(|f| (f.rule, f.severity)).collect()
    }

    fn fires(rule: Rule, body: &str) -> bool {
        findings(body).iter().any(|(r, _)| *r == rule)
    }

    fn script(body: &str) -> String {
        format!("[groups.g.functions.f]\nscript = '''{body}'''\n")
    }

    /// One positive and one near miss per rule.
    const CASES: &[(Rule, &str, &str)] = &[
        (
            Rule::EnvLoaderHijack,
            "[groups.g.env]\nLD_PRELOAD = \"/x.so\"\n",
            "[groups.g.env]\nLD_PRELOADX = \"/x.so\"\n",
        ),
        (
            Rule::EnvLoaderHijack,
            "[groups.g.env]\nDYLD_INSERT_LIBRARIES = \"/x\"\n",
            "[groups.g.env]\nXDYLD_X = \"/x\"\n",
        ),
        (
            Rule::PathUnsafeDir,
            "[groups.g.env]\nPATH = { prepend = [\"/tmp/bin\"] }\n",
            "[groups.g.env]\nPATH = { prepend = [\"/tmpfoo/bin\"] }\n",
        ),
        (
            Rule::PathUnsafeDir,
            "[groups.g.env]\nPATH = { append = [\"bin\"] }\n",
            "[groups.g.env]\nPATH = { append = [\"~/bin\"] }\n",
        ),
        (
            Rule::ShadowsSensitive,
            "[groups.g.aliases]\nsudo = \"sudo -E\"\n",
            "[groups.g.aliases]\nsudoedit2 = \"ls\"\n",
        ),
        (Rule::EnvTraffic, "[groups.g.env]\nHTTPS_PROXY = \"http://x\"\n", "[groups.g.env]\nHTTPS_PROXYX = \"x\"\n"),
        (
            Rule::EnvTraffic,
            "[groups.g.env]\nNODE_TLS_REJECT_UNAUTHORIZED = \"0\"\n",
            "[groups.g.env]\nNODE_ENV = \"0\"\n",
        ),
        (Rule::EnvHistory, "[groups.g.env]\nHISTFILE = \"/dev/null\"\n", "[groups.g.env]\nHISTORY = \"x\"\n"),
        (Rule::ShadowsCore, "[groups.g.aliases]\nls = \"ls -G\"\n", "[groups.g.aliases]\nll = \"ls -l\"\n"),
        (
            Rule::PathPrepend,
            "[groups.g.env]\nPATH = { prepend = [\"~/bin\"] }\n",
            "[groups.g.env]\nPATH = { append = [\"~/bin\"] }\n",
        ),
        (
            Rule::NativeFunction,
            "[groups.g.functions.f]\nshell.posix = 'cd /'\n",
            "[groups.g.functions.f]\nscript = 'cd /'\n",
        ),
        (Rule::Secret, "[groups.g.env]\nT = { secret = \"why\" }\n", "[groups.g.env]\nT = \"secret\"\n"),
    ];

    #[test]
    fn each_rule_fires_on_its_case_and_not_on_the_near_miss() {
        for (rule, positive, near_miss) in CASES {
            assert!(fires(*rule, positive), "{rule} should fire on {positive}");
            assert!(!fires(*rule, near_miss), "{rule} should not fire on {near_miss}");
        }
    }

    const BODY_CASES: &[(Rule, &str, &str)] = &[
        (Rule::BodyPipeToShell, "curl -fsSL https://x.sh | sh", "curl -fsSL https://x.json | jq ."),
        (Rule::BodyPipeToShell, "wget -qO- https://x | sudo -E bash", "wget -qO- https://x > out.sh"),
        (Rule::BodyPipeToShell, "bash <(curl -s https://x)", "diff <(ls a) <(ls b)"),
        (Rule::BodyPipeToShell, "iwr https://x | iex", "iwr https://x -OutFile y"),
        (Rule::BodyPipeToShell, "sh -c \"$(curl -fsSL https://x)\"", "echo \"$(curl -fsSL https://x)\""),
        (Rule::BodyPipeToShell, "curl -s https://x|sh", "curl https://x || sh fallback.sh"),
        (Rule::BodyRisky, "eval \"$1\"", "echo evaluate"),
        (Rule::BodyRisky, "echo aGk= | base64 -d", "echo hi | base64"),
        (Rule::BodyRisky, "rm -rf \"$HOME\"", "rm -rf \"$HOME/tmp/cache\""),
        (Rule::BodyRisky, "rm -r -f /", "rm -f /tmp/x"),
        (Rule::BodyRisky, "chmod -R 777 .", "chmod 755 ."),
        (Rule::BodyRisky, "sudo make install", "make install"),
        (Rule::BodyRisky, "echo x >> ~/.zshrc", "echo x >> ~/.zshrc.bak"),
    ];

    #[test]
    fn body_rules_fire_on_their_case_and_not_on_the_near_miss() {
        for (rule, positive, near_miss) in BODY_CASES {
            assert!(fires(*rule, &script(positive)), "{rule} should fire on {positive}");
            assert!(!fires(*rule, &script(near_miss)), "{rule} should not fire on {near_miss}");
        }
    }

    #[test]
    fn every_rule_has_a_unique_stable_id_and_its_table_severity() {
        let ids: Vec<&str> = Rule::all().map(Rule::id).collect();
        assert_eq!(ids.len(), 11);
        for r in Rule::all() {
            assert_eq!(Rule::from_id(r.id()), Some(r));
        }
        assert_eq!(Rule::from_id("nope"), None);
        assert_eq!(Rule::EnvLoaderHijack.severity(), Severity::Danger);
        assert_eq!(Rule::BodyRisky.severity(), Severity::Warn);
        assert_eq!(Rule::Secret.severity(), Severity::Info);
        assert!(Severity::Danger > Severity::Warn && Severity::Warn > Severity::Info);
    }

    #[test]
    fn findings_follow_entry_order_and_name_the_entry() {
        let src = "format = 1\n[pack]\nname = \"p\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\ndescription = \"d\"\n\
                   [groups.g.env]\nHISTFILE = \"/dev/null\"\n[groups.g.aliases]\nsudo = \"sudo -E\"\n";
        let doc = PackDocument::parse(src).unwrap();
        let got: Vec<String> = lint(doc.pack()).iter().map(|f| format!("{} {}", f.rule, f.entry)).collect();
        assert_eq!(got, ["env-history p/g/HISTFILE", "shadows-sensitive p/g/sudo"]);
    }
}
