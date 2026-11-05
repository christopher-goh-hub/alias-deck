//! The deck manifest (`deck.toml`): which packs the user wants, in precedence order, and which
//! registries name packs. People, the CLI, and later the GUI all write it, so edits go through
//! `toml_edit` and keep the user's comments and order. The model is re-derived from the document
//! after every edit.

use std::ops::Range;

use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use crate::document::{span, Cx, ParseError, ParseErrorKind};
use crate::source::{GitRef, Source, SourceError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestPack {
    /// Carries the manifest's `ref` key, if any.
    pub source: Source,
    pub enabled: bool,
}

/// The official registry, in this project's repository.
pub const DEFAULT_REGISTRY: &str = "github:christopher-goh-hub/alias-deck//registry/index.toml";
pub const DEFAULT_REGISTRY_REF: &str = "main";

#[derive(Debug, Clone)]
pub struct Manifest {
    doc: DocumentMut,
    packs: Vec<ManifestPack>,
    /// `None` when the file has no `[[registries]]`, which means the default registry.
    registries: Option<Vec<Source>>,
}

impl Default for Manifest {
    fn default() -> Manifest {
        Manifest::parse("format = 1\n").expect("the empty manifest is valid")
    }
}

impl Manifest {
    pub fn parse(src: &str) -> Result<Manifest, Vec<ParseError>> {
        let doc = Cx::parse_toml(src)?;
        let mut cx = Cx::new();
        let whole = 0..src.len();
        let root = doc.as_table();
        cx.only_keys(root, &["format", "packs", "registries"], &whole);
        cx.format_one(root, &whole);
        let registries = root.get("registries").map(|item| parse_registries(&mut cx, item, &whole));
        let mut packs: Vec<ManifestPack> = Vec::new();
        if let Some(item) = root.get("packs") {
            match item.as_array_of_tables() {
                None => cx.error(span(item, &whole), ParseErrorKind::WrongType { expected: "[[packs]] tables" }),
                Some(tables) => {
                    for t in tables.iter() {
                        let at = t.span().unwrap_or_else(|| whole.clone());
                        cx.only_keys(t, &["source", "ref", "enabled"], &at);
                        if let Some(p) = parse_pack(&mut cx, t, &at) {
                            if packs.iter().any(|q| q.source.same_pack(&p.source)) {
                                cx.error(at, ParseErrorKind::DuplicateSource(p.source.to_string()));
                            } else {
                                packs.push(p);
                            }
                        }
                    }
                }
            }
        }
        cx.finish(Some(Manifest { doc: doc.into_mut(), packs, registries }))
    }

    pub fn packs(&self) -> &[ManifestPack] {
        &self.packs
    }

    /// The registries to consult, in precedence order: a later one wins a name both list. A
    /// `[[registries]]` list replaces the default, so it names the default too if it wants it.
    pub fn registries(&self) -> Vec<Source> {
        self.registries.clone().unwrap_or_else(|| {
            let default = Source::parse(DEFAULT_REGISTRY).expect("the default registry is a valid source");
            vec![default.with_ref(Some(GitRef::new(DEFAULT_REGISTRY_REF).expect("a valid ref")))]
        })
    }

    pub fn find(&self, source: &Source) -> Option<&ManifestPack> {
        self.packs.iter().find(|p| p.source.same_pack(source))
    }

    /// Adds the pack last, so it wins name clashes. A pack already listed keeps its place and its
    /// `enabled` flag, and takes the new ref.
    pub fn upsert(&mut self, source: &Source) {
        let at = self.packs.iter().position(|p| p.source.same_pack(source));
        let tables = self.tables_mut();
        match at.and_then(|i| tables.get_mut(i)) {
            Some(t) => match source.reference() {
                Some(r) => t["ref"] = value(r.as_str()),
                None => drop(t.remove("ref")),
            },
            None => {
                let mut t = Table::new();
                t["source"] = value(source.to_string());
                if let Some(r) = source.reference() {
                    t["ref"] = value(r.as_str());
                }
                tables.push(t);
            }
        }
        self.rederive();
    }

    pub fn remove(&mut self, source: &Source) -> bool {
        let Some(i) = self.packs.iter().position(|p| p.source.same_pack(source)) else { return false };
        self.tables_mut().remove(i);
        self.rederive();
        true
    }

    pub fn render(&self) -> String {
        self.doc.to_string()
    }

    fn tables_mut(&mut self) -> &mut ArrayOfTables {
        if self.doc.get("packs").is_none() {
            self.doc.insert("packs", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        self.doc["packs"].as_array_of_tables_mut().expect("parse accepted only [[packs]] tables")
    }

    fn rederive(&mut self) {
        *self = Manifest::parse(&self.doc.to_string()).expect("an edit through the model keeps the manifest valid");
    }
}

/// Each entry is a git source with an optional `ref`, like a pack. A local index would name
/// files only this machine has, which a registry exists to avoid.
fn parse_registries(cx: &mut Cx, item: &Item, whole: &Range<usize>) -> Vec<Source> {
    let Some(tables) = item.as_array_of_tables() else {
        cx.error(span(item, whole), ParseErrorKind::WrongType { expected: "[[registries]] tables" });
        return Vec::new();
    };
    let mut out: Vec<Source> = Vec::new();
    for t in tables.iter() {
        let at = t.span().unwrap_or_else(|| whole.clone());
        cx.only_keys(t, &["source", "ref"], &at);
        let Some(p) = parse_pack(cx, t, &at) else { continue };
        if let Source::Local(_) = p.source {
            let item = t.get("source").expect("parse_pack found a source");
            cx.error(span(item, &at), ParseErrorKind::WrongType { expected: "a git source" });
        } else if out.iter().any(|s| s.same_pack(&p.source)) {
            cx.error(at, ParseErrorKind::DuplicateSource(p.source.to_string()));
        } else {
            out.push(p.source);
        }
    }
    out
}

fn parse_pack(cx: &mut Cx, t: &Table, at: &Range<usize>) -> Option<ManifestPack> {
    let bad_source = |cx: &mut Cx, item: &Item, e: SourceError| cx.error(span(item, at), ParseErrorKind::BadSource(e));
    let source = match t.get("source") {
        None => {
            cx.error(at.clone(), ParseErrorKind::MissingKey("source"));
            None
        }
        Some(item) => {
            let s = cx.str(item, at)?;
            Source::parse(s).map_err(|e| bad_source(cx, item, e)).ok()
        }
    };
    let reference = match t.get("ref") {
        None => Some(None),
        Some(item) => {
            let s = cx.str(item, at)?;
            let r = GitRef::new(s).map_err(|e| bad_source(cx, item, e)).ok()?;
            if matches!(source, Some(Source::Local(_))) {
                bad_source(cx, item, SourceError::RefOnLocal);
                return None;
            }
            Some(Some(r))
        }
    };
    let enabled = match t.get("enabled") {
        None => Some(true),
        Some(item) => {
            let b = item.as_bool();
            if b.is_none() {
                cx.error(span(item, at), ParseErrorKind::WrongType { expected: "true or false" });
            }
            b
        }
    };
    Some(ManifestPack { source: source?.with_ref(reference?), enabled: enabled? })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "# my deck\nformat = 1\n\n[[packs]]\nsource = \"github:alice/dotpacks//git.toml\"   # pinned\nref = \"v2\"\n\n[[packs]]\nsource = \"~/dotfiles/mine.toml\"\nenabled = false\n";

    #[test]
    fn parses_sources_refs_and_enabled_in_order() {
        let m = Manifest::parse(USER).unwrap();
        let got: Vec<(String, Option<String>, bool)> = m
            .packs()
            .iter()
            .map(|p| (p.source.to_string(), p.source.reference().map(|r| r.to_string()), p.enabled))
            .collect();
        assert_eq!(
            got,
            [
                ("github:alice/dotpacks//git.toml".into(), Some("v2".into()), true),
                ("~/dotfiles/mine.toml".into(), None, false)
            ]
        );
    }

    #[test]
    fn edits_keep_comments_and_order() {
        let mut m = Manifest::parse(USER).unwrap();
        m.upsert(&Source::parse_with_ref("github:bob/packs@main").unwrap());
        m.upsert(&Source::parse("~/dotfiles/mine.toml").unwrap());
        let out = m.render();
        assert!(out.starts_with("# my deck\nformat = 1\n"), "{out}");
        assert!(out.contains("source = \"github:alice/dotpacks//git.toml\"   # pinned\n"), "{out}");
        let order: Vec<String> = m.packs().iter().map(|p| p.source.to_string()).collect();
        assert_eq!(order, ["github:alice/dotpacks//git.toml", "~/dotfiles/mine.toml", "github:bob/packs"]);
        assert!(!m.packs()[1].enabled, "re-adding a pack keeps it disabled");
        assert_eq!(m.packs()[2].source.reference().unwrap().as_str(), "main");

        assert!(m.remove(&Source::parse("github:alice/dotpacks//git.toml").unwrap()));
        assert!(!m.remove(&Source::parse("github:nobody/x").unwrap()));
        assert_eq!(Manifest::parse(&m.render()).unwrap().packs().len(), 2);
    }

    #[test]
    fn upsert_twice_is_one_entry() {
        let mut m = Manifest::default();
        let s = Source::parse("git+file:///tmp/r//p.toml").unwrap();
        m.upsert(&s);
        let once = m.render();
        m.upsert(&s);
        assert_eq!(m.render(), once);
        assert_eq!(once, "format = 1\n\n[[packs]]\nsource = \"git+file:///tmp/r//p.toml\"\n");
    }

    #[test]
    fn registries_default_to_the_official_index_and_a_list_replaces_it() {
        let spelled = |m: &Manifest| -> Vec<(String, Option<String>)> {
            m.registries().iter().map(|s| (s.to_string(), s.reference().map(|r| r.to_string()))).collect()
        };
        assert_eq!(spelled(&Manifest::default()), [(DEFAULT_REGISTRY.to_owned(), Some("main".to_owned()))]);
        let mut m = Manifest::parse(
            "format = 1\n[[registries]]\nsource = \"github:me/packs//index.toml\"\n\n[[registries]]\nsource = \"git+file:///srv/reg//index.toml\"\nref = \"stable\"\n",
        )
        .unwrap();
        let mine = [
            ("github:me/packs//index.toml".to_owned(), None),
            ("git+file:///srv/reg//index.toml".to_owned(), Some("stable".to_owned())),
        ];
        assert_eq!(spelled(&m), mine);
        m.upsert(&Source::parse("github:a/b").unwrap());
        assert_eq!(spelled(&m), mine, "editing packs keeps the registries");
        assert!(m.render().contains("[[registries]]\nsource = \"github:me/packs//index.toml\""));
    }

    #[test]
    fn rejects_bad_entries_with_spans() {
        let kinds = |src: &str| Manifest::parse(src).unwrap_err().into_iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds("format = 1\n[[packs]]\nsource = \"x.toml\"\n"),
            [ParseErrorKind::BadSource(SourceError::Unrecognized)]
        );
        assert_eq!(
            kinds("format = 1\n[[packs]]\nsource = \"./x.toml\"\nref = \"v1\"\n"),
            [ParseErrorKind::BadSource(SourceError::RefOnLocal)]
        );
        assert_eq!(
            kinds("format = 1\n[[packs]]\nsource = \"github:a/b\"\n[[packs]]\nsource = \"github:a/b//pack.toml\"\n"),
            [ParseErrorKind::DuplicateSource("github:a/b".into())]
        );
        assert_eq!(
            kinds("format = 1\n[[packs]]\nsorce = \"github:a/b\"\n"),
            [ParseErrorKind::UnknownKey("sorce".into()), ParseErrorKind::MissingKey("source")]
        );
        assert_eq!(kinds("[[packs]]\nsource = \"github:a/b\"\n"), [ParseErrorKind::UnsupportedFormat]);
        assert_eq!(
            kinds("format = 1\n[[registries]]\nsource = \"./index.toml\"\n"),
            [ParseErrorKind::WrongType { expected: "a git source" }]
        );
        assert_eq!(
            kinds("format = 1\n[[registries]]\nsource = \"github:a/b\"\nenabled = false\n"),
            [ParseErrorKind::UnknownKey("enabled".into())]
        );
    }
}
