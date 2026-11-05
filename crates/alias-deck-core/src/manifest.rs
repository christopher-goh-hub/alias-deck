//! The deck manifest (`deck.toml`): which packs the user wants, in precedence order. People,
//! the CLI, and later the GUI all write it, so edits go through `toml_edit` and keep the user's
//! comments and order. The model is re-derived from the document after every edit.

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

#[derive(Debug, Clone)]
pub struct Manifest {
    doc: DocumentMut,
    packs: Vec<ManifestPack>,
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
        cx.only_keys(root, &["format", "packs"], &whole);
        cx.format_one(root, &whole);
        let mut packs: Vec<ManifestPack> = Vec::new();
        if let Some(item) = root.get("packs") {
            match item.as_array_of_tables() {
                None => cx.error(span(item, &whole), ParseErrorKind::WrongType { expected: "[[packs]] tables" }),
                Some(tables) => {
                    for t in tables.iter() {
                        let at = t.span().unwrap_or_else(|| whole.clone());
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
        cx.finish(Some(Manifest { doc: doc.into_mut(), packs }))
    }

    pub fn packs(&self) -> &[ManifestPack] {
        &self.packs
    }

    pub fn find(&self, source: &Source) -> Option<&ManifestPack> {
        self.packs.iter().find(|p| p.source.same_pack(source))
    }

    /// Adds the pack last, so it wins name clashes. A pack already listed keeps its place, takes
    /// the new ref, and is enabled again.
    pub fn upsert(&mut self, source: &Source) {
        let at = self.packs.iter().position(|p| p.source.same_pack(source));
        let tables = self.tables_mut();
        match at.and_then(|i| tables.get_mut(i)) {
            Some(t) => {
                match source.reference() {
                    Some(r) => t["ref"] = value(r.as_str()),
                    None => drop(t.remove("ref")),
                }
                t.remove("enabled");
            }
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

fn parse_pack(cx: &mut Cx, t: &Table, at: &Range<usize>) -> Option<ManifestPack> {
    cx.only_keys(t, &["source", "ref", "enabled"], at);
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
        assert!(m.packs().iter().all(|p| p.enabled), "re-adding a pack enables it");
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
    }
}
