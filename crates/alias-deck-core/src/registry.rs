//! A registry index: pack names mapped to where each pack lives. It is a directory, not a seal of
//! approval. A name resolves to a source, and that source goes through the same fetch, preview,
//! lint, and approval as any other.

use std::ops::Range;

use toml_edit::{Item, Table};

use crate::document::{span, Cx, ParseError, ParseErrorKind};
use crate::model::{InvalidValue, Slug, Text};
use crate::source::Source;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    /// Equals the `[pack].name` of the pack at `source`.
    pub name: Slug,
    /// Always a git source: a local path in a published index names a file on someone else's disk.
    pub source: Source,
    pub description: Text,
    pub tags: Vec<Slug>,
}

impl RegistryEntry {
    /// Case-insensitive substring match on the name, the description, or any tag. An empty term
    /// matches everything.
    pub fn matches(&self, term: &str) -> bool {
        let term = term.to_lowercase();
        self.name.as_str().contains(&term)
            || self.description.as_str().to_lowercase().contains(&term)
            || self.tags.iter().any(|t| t.as_str().contains(&term))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryIndex {
    /// Sorted by name, and names are unique.
    entries: Vec<RegistryEntry>,
}

impl RegistryIndex {
    pub fn parse(src: &str) -> Result<RegistryIndex, Vec<ParseError>> {
        let doc = Cx::parse_toml(src)?;
        let mut cx = Cx::new();
        let whole = 0..src.len();
        let root = doc.as_table();
        cx.only_keys(root, &["format", "packs"], &whole);
        cx.format_one(root, &whole);
        let mut entries: Vec<RegistryEntry> = Vec::new();
        if let Some(item) = root.get("packs") {
            match item.as_array_of_tables() {
                None => cx.error(span(item, &whole), ParseErrorKind::WrongType { expected: "[[packs]] tables" }),
                Some(tables) => {
                    for t in tables.iter() {
                        let at = t.span().unwrap_or_else(|| whole.clone());
                        let Some(entry) = parse_entry(&mut cx, t, &at) else { continue };
                        if entries.iter().any(|e| e.name == entry.name) {
                            cx.error(at, ParseErrorKind::DuplicateName(entry.name.as_str().to_owned()));
                        } else {
                            entries.push(entry);
                        }
                    }
                }
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        cx.finish(Some(RegistryIndex { entries }))
    }

    pub fn entries(&self) -> &[RegistryEntry] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.name.as_str() == name)
    }
}

fn parse_entry(cx: &mut Cx, t: &Table, at: &Range<usize>) -> Option<RegistryEntry> {
    cx.only_keys(t, &["name", "source", "description", "tags"], at);
    let required = |cx: &mut Cx, key: &'static str| {
        let item = t.get(key);
        if item.is_none() {
            cx.error(at.clone(), ParseErrorKind::MissingKey(key));
        }
        item
    };
    let name = required(cx, "name").and_then(|i| {
        let s = cx.str(i, at)?;
        cx.valid(Slug::new(s), i, at)
    });
    let source = required(cx, "source").and_then(|i| git_source(cx, i, at));
    let description = required(cx, "description").and_then(|i| {
        let text = cx.text(i, at)?;
        let empty = text.as_str().trim().is_empty();
        cx.valid(if empty { Err(InvalidValue::Empty) } else { Ok(text) }, i, at)
    });
    let tags = match t.get("tags") {
        None => Some(Vec::new()),
        Some(i) => cx.strings(i, at).map(|tags| {
            tags.into_iter()
                .filter_map(|(s, sp)| Slug::new(s).map_err(|e| cx.error(sp, ParseErrorKind::Invalid(e))).ok())
                .collect()
        }),
    };
    Some(RegistryEntry { name: name?, source: source?, description: description?, tags: tags? })
}

/// The command-line spelling, so an entry may pin a ref: `github:a/b//x.toml@v1`.
fn git_source(cx: &mut Cx, item: &Item, at: &Range<usize>) -> Option<Source> {
    let s = cx.str(item, at)?;
    match Source::parse_with_ref(s) {
        Ok(source @ Source::Git { .. }) => Some(source),
        Ok(Source::Local(_)) => {
            cx.error(span(item, at), ParseErrorKind::WrongType { expected: "a git source" });
            None
        }
        Err(e) => {
            cx.error(span(item, at), ParseErrorKind::BadSource(e));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"format = 1

[[packs]]
name = "navigation"
source = "github:a/packs//nav.toml"
description = "Move around the file system"
tags = ["cd", "files"]

[[packs]]
name = "git-essentials"
source = "git+https://example.com/r.git//git.toml@v2"
description = "Everyday Git shortcuts"
"#;

    #[test]
    fn parses_entries_sorted_by_name() {
        let index = RegistryIndex::parse(INDEX).unwrap();
        let names: Vec<&str> = index.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["git-essentials", "navigation"]);
        let git = index.get("git-essentials").unwrap();
        assert_eq!(git.source.to_string(), "git+https://example.com/r.git//git.toml");
        assert_eq!(git.source.reference().map(|r| r.as_str()), Some("v2"));
        assert!(git.tags.is_empty());
        assert!(index.get("nope").is_none());
    }

    #[test]
    fn matches_name_description_and_tags_ignoring_case() {
        let index = RegistryIndex::parse(INDEX).unwrap();
        let hits = |term: &str| -> Vec<&str> {
            index.entries().iter().filter(|e| e.matches(term)).map(|e| e.name.as_str()).collect()
        };
        assert_eq!(hits(""), ["git-essentials", "navigation"]);
        assert_eq!(hits("GIT"), ["git-essentials"]);
        assert_eq!(hits("file system"), ["navigation"]);
        assert_eq!(hits("cd"), ["navigation"]);
        assert!(hits("docker").is_empty());
    }

    #[test]
    fn rejects_local_sources_duplicates_unknown_keys_and_bad_tags() {
        let kinds = |src: &str| RegistryIndex::parse(src).unwrap_err().into_iter().map(|e| e.kind).collect::<Vec<_>>();
        let entry = |name: &str, source: &str| {
            format!("[[packs]]\nname = \"{name}\"\nsource = \"{source}\"\ndescription = \"d\"\n")
        };
        assert_eq!(
            kinds(&format!("format = 1\n{}", entry("a", "./a.toml"))),
            [ParseErrorKind::WrongType { expected: "a git source" }]
        );
        assert_eq!(
            kinds(&format!("format = 1\n{}{}", entry("a", "github:x/y"), entry("a", "github:x/z"))),
            [ParseErrorKind::DuplicateName("a".into())]
        );
        assert_eq!(
            kinds(&format!("format = 1\n{}tag = [\"x\"]\n", entry("a", "github:x/y"))),
            [ParseErrorKind::UnknownKey("tag".into())]
        );
        assert_eq!(
            kinds(&format!("format = 1\n{}tags = [\"Git Tools\"]\n", entry("a", "github:x/y"))),
            [ParseErrorKind::Invalid(InvalidValue::BadSlug)]
        );
        assert_eq!(
            kinds("format = 1\n[[packs]]\nname = \"a\"\nsource = \"github:x/y\"\ndescription = \" \"\n"),
            [ParseErrorKind::Invalid(InvalidValue::Empty)]
        );
        assert_eq!(kinds(&entry("a", "github:x/y")), [ParseErrorKind::UnsupportedFormat]);
    }
}
