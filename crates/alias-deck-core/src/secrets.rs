//! Secret bindings live in the user's own file, never in a pack. A pack only names a need, so a
//! stranger's pack cannot choose to read `~/.ssh/id_ed25519` or run a command of its choosing.

use std::collections::BTreeMap;
use std::ops::Range;

use toml_edit::TableLike;

use crate::document::{span, Cx, ParseError, ParseErrorKind};
use crate::model::{Argv, EnvName, InvalidValue, Os, Text, Word};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings(BTreeMap<EnvName, SecretSource>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    Keychain {
        service: Text,
        account: Text,
    },
    File {
        path: Word,
    },
    /// The user's own argv, such as `op read op://Private/OpenAI/credential`.
    Command {
        argv: Argv,
    },
}

impl Bindings {
    /// `NAME = { keychain = { service, account } } | { file = "~/path" } | { command = [argv] }`.
    pub fn parse(src: &str) -> Result<Bindings, Vec<ParseError>> {
        let doc = Cx::parse_toml(src)?;
        let mut cx = Cx::new();
        let whole = 0..src.len();
        let root = doc.as_table();
        let mut map = BTreeMap::new();
        for (name, item) in root.iter() {
            let at = span(item, &whole);
            let name = cx.key_valid(EnvName::new(name), root, name, &at);
            let source = cx.table(item, &at).and_then(|t| parse_source(&mut cx, t, &at));
            if let (Some(name), Some(source)) = (name, source) {
                map.insert(name, source);
            }
        }
        cx.finish(Some(Bindings(map)))
    }

    pub fn get(&self, name: &EnvName) -> Option<&SecretSource> {
        self.0.get(name)
    }
}

fn parse_source(cx: &mut Cx, t: &dyn TableLike, at: &Range<usize>) -> Option<SecretSource> {
    cx.only_keys(t, &["keychain", "file", "command"], at);
    let present: Vec<&str> = ["keychain", "file", "command"].into_iter().filter(|k| t.contains_key(k)).collect();
    if present.len() != 1 {
        cx.error(at.clone(), ParseErrorKind::SecretSourceShape);
        return None;
    }
    let item = t.get(present[0])?;
    let at = span(item, at);
    match present[0] {
        "keychain" => {
            let kc = cx.table(item, &at)?;
            cx.only_keys(kc, &["service", "account"], &at);
            let mut field = |key: &'static str| match kc.get(key) {
                None => {
                    cx.error(at.clone(), ParseErrorKind::MissingKey(key));
                    None
                }
                Some(v) => {
                    let text = cx.text(v, &at)?;
                    cx.valid(if text.as_str().is_empty() { Err(InvalidValue::Empty) } else { Ok(text) }, v, &at)
                }
            };
            let (service, account) = (field("service"), field("account"));
            Some(SecretSource::Keychain { service: service?, account: account? })
        }
        "file" => {
            let s = cx.str(item, &at)?;
            match cx.valid(Word::parse(s), item, &at)? {
                Word::Literal(t) if !t.as_str().starts_with('/') => {
                    cx.error(at, ParseErrorKind::RelativeSecretPath);
                    None
                }
                path => Some(SecretSource::File { path }),
            }
        }
        _ => cx.argv(item, &at).map(|argv| SecretSource::Command { argv }),
    }
}

fn lit(s: &str) -> Word {
    Word::Literal(Text::new(s).expect("lookup words come from Text or static strings"))
}

fn argv(program: &str, args: Vec<Word>) -> Argv {
    Argv::new(lit(program), args).expect("lookup programs are fixed absolute paths or names")
}

impl SecretSource {
    /// The argv whose stdout is the secret, from a fixed template. `None` when this source has no
    /// lookup on `os`; Windows has neither a keychain template nor `/bin/cat`.
    pub(crate) fn lookup(&self, os: Os) -> Option<Argv> {
        match (self, os) {
            (SecretSource::Keychain { service, account }, Os::MacOs) => Some(argv(
                "/usr/bin/security",
                vec![
                    lit("find-generic-password"),
                    lit("-s"),
                    lit(service.as_str()),
                    lit("-a"),
                    lit(account.as_str()),
                    lit("-w"),
                ],
            )),
            (SecretSource::Keychain { service, account }, Os::Linux) => Some(argv(
                "secret-tool",
                vec![lit("lookup"), lit("service"), lit(service.as_str()), lit("account"), lit(account.as_str())],
            )),
            (SecretSource::File { path }, Os::MacOs | Os::Linux) => {
                Some(argv("/bin/cat", vec![lit("--"), path.clone()]))
            }
            (SecretSource::Command { argv }, _) => Some(argv.clone()),
            (SecretSource::Keychain { .. } | SecretSource::File { .. }, Os::Windows) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(a: &Argv) -> Vec<String> {
        std::iter::once(a.program())
            .chain(a.args())
            .map(|w| match w {
                Word::Literal(t) => t.as_str().to_owned(),
                Word::Home(t) => format!("$HOME{t}"),
            })
            .collect()
    }

    const SRC: &str = r#"
GITHUB_TOKEN = { keychain = { service = "alias-deck", account = "github-token" } }
NPM_TOKEN    = { file = "~/.config/npm/token" }
OPENAI_KEY   = { command = ["op", "read", "op://Private/OpenAI/credential"] }
"#;

    #[test]
    fn each_source_has_a_fixed_lookup_template() {
        let b = Bindings::parse(SRC).unwrap();
        let get = |n: &str| b.get(&EnvName::new(n).unwrap()).unwrap();
        let gh = get("GITHUB_TOKEN");
        assert_eq!(
            words(&gh.lookup(Os::MacOs).unwrap()),
            ["/usr/bin/security", "find-generic-password", "-s", "alias-deck", "-a", "github-token", "-w"]
        );
        assert_eq!(
            words(&gh.lookup(Os::Linux).unwrap()),
            ["secret-tool", "lookup", "service", "alias-deck", "account", "github-token"]
        );
        assert_eq!(gh.lookup(Os::Windows), None, "no keychain template on Windows");
        assert_eq!(words(&get("NPM_TOKEN").lookup(Os::Linux).unwrap()), ["/bin/cat", "--", "$HOME/.config/npm/token"]);
        assert_eq!(get("NPM_TOKEN").lookup(Os::Windows), None, "no /bin/cat on Windows");
        assert_eq!(
            words(&get("OPENAI_KEY").lookup(Os::Windows).unwrap()),
            ["op", "read", "op://Private/OpenAI/credential"]
        );
    }

    #[test]
    fn rejects_ambiguous_relative_or_unknown_sources() {
        let kinds = |src: &str| Bindings::parse(src).err().unwrap().into_iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(kinds("A = { file = \"/x\", command = [\"y\"] }"), [ParseErrorKind::SecretSourceShape]);
        assert_eq!(kinds("A = { file = \"token.txt\" }"), [ParseErrorKind::RelativeSecretPath]);
        assert_eq!(
            kinds("A = { url = \"https://x\" }"),
            [ParseErrorKind::UnknownKey("url".into()), ParseErrorKind::SecretSourceShape]
        );
        assert_eq!(kinds("A = { keychain = { service = \"s\" } }"), [ParseErrorKind::MissingKey("account")]);
        assert_eq!(kinds("1A = { file = \"/x\" }"), [ParseErrorKind::Invalid(InvalidValue::BadEnvName)]);
    }
}
