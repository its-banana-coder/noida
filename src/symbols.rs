//! Tree-sitter powered symbols: file outlines, a project-wide definition
//! index, and structural (syntax-aware) selection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Java,
}

impl Lang {
    pub fn for_path(path: &Path) -> Option<Lang> {
        Some(match path.extension()?.to_str()? {
            "rs" => Lang::Rust,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "py" => Lang::Python,
            "go" => Lang::Go,
            "java" => Lang::Java,
            _ => return None,
        })
    }

    fn language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
        }
    }

    fn tags_source(self) -> String {
        match self {
            Lang::Rust => tree_sitter_rust::TAGS_QUERY.into(),
            // The TypeScript grammar extends JavaScript; its tags only add TS-specific constructs.
            Lang::TypeScript | Lang::Tsx => format!("{}\n{}", tree_sitter_javascript::TAGS_QUERY, tree_sitter_typescript::TAGS_QUERY),
            Lang::JavaScript => tree_sitter_javascript::TAGS_QUERY.into(),
            Lang::Python => tree_sitter_python::TAGS_QUERY.into(),
            Lang::Go => tree_sitter_go::TAGS_QUERY.into(),
            Lang::Java => tree_sitter_java::TAGS_QUERY.into(),
        }
    }

    fn tags(self) -> Option<&'static Query> {
        static CACHE: OnceLock<std::sync::Mutex<HashMap<Lang, Option<&'static Query>>>> = OnceLock::new();
        let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
        *cache.entry(self).or_insert_with(|| {
            Query::new(&self.language(), &self.tags_source()).ok().map(|q| &*Box::leak(Box::new(q)))
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    /// 0-based line and char column of the symbol name.
    pub line: usize,
    pub col: usize,
    /// Last line of the whole definition (for breadcrumbs and sticky scroll).
    pub end_line: usize,
}

fn parse(lang: Lang, text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).ok()?;
    parser.parse(text, None)
}

pub fn outline(path: &Path, text: &str) -> Vec<Symbol> {
    let Some(lang) = Lang::for_path(path) else { return Vec::new() };
    let (Some(tree), Some(query)) = (parse(lang, text), lang.tags()) else { return Vec::new() };
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    let mut out: Vec<Symbol> = Vec::new();
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut kind = None;
        let mut end_line = 0;
        for c in m.captures {
            let cap = names[c.index as usize];
            if cap == "name" {
                name_node = Some(c.node);
            } else if let Some(k) = cap.strip_prefix("definition.") {
                kind = Some(k);
                end_line = c.node.end_position().row;
            }
        }
        let (Some(node), Some(kind)) = (name_node, kind) else { continue };
        let Ok(name) = node.utf8_text(text.as_bytes()) else { continue };
        let pos = node.start_position();
        let line_start = text[..node.start_byte()].rfind('\n').map_or(0, |i| i + 1);
        let col = text[line_start..node.start_byte()].chars().count();
        // Patterns overlap (a Rust method also matches the function pattern); keep the first.
        if out.iter().any(|s| s.line == pos.row && s.col == col) {
            continue;
        }
        out.push(Symbol { name: name.to_string(), kind: kind.to_string(), line: pos.row, col, end_line: end_line.max(pos.row) });
    }
    out.sort_by_key(|s| (s.line, s.col));
    out
}

#[derive(Default)]
pub struct ProjectSymbols {
    pub symbols: Vec<(PathBuf, Symbol)>,
    by_name: HashMap<String, Vec<usize>>,
}

impl ProjectSymbols {
    pub fn build(root: &Path, files: &[String]) -> Self {
        let mut ps = ProjectSymbols::default();
        for rel in files.iter().filter(|f| Lang::for_path(Path::new(f)).is_some()).take(20_000) {
            let path = root.join(rel);
            if std::fs::metadata(&path).map_or(true, |m| m.len() > 1_000_000) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for sym in outline(&path, &text) {
                ps.by_name.entry(sym.name.clone()).or_default().push(ps.symbols.len());
                ps.symbols.push((path.clone(), sym));
            }
        }
        ps
    }

    pub fn definitions(&self, name: &str) -> Vec<&(PathBuf, Symbol)> {
        self.by_name.get(name).map(|ids| ids.iter().map(|&i| &self.symbols[i]).collect()).unwrap_or_default()
    }
}

/// The smallest syntax node strictly larger than the byte range `sel`.
pub fn expand(path: &Path, text: &str, sel: (usize, usize)) -> Option<(usize, usize)> {
    let tree = parse(Lang::for_path(path)?, text)?;
    let mut node = tree.root_node().descendant_for_byte_range(sel.0, sel.1)?;
    loop {
        let r = (node.start_byte(), node.end_byte());
        if (r.0 < sel.0 || r.1 > sel.1) && r.0 <= sel.0 && r.1 >= sel.1 && (node.is_named() || node.parent().is_none()) {
            return Some(r);
        }
        node = node.parent()?;
    }
}

/// Whether an identifier looks specific enough to link from agent prose:
/// camelCase, PascalCase with an inner capital, or snake_case.
pub fn looks_like_code(word: &str) -> bool {
    word.len() >= 4
        && word.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
        && (word.contains('_') || word.chars().skip(1).any(|c| c.is_uppercase()))
}

/// Whole-word occurrences of `name` across the project: (path, line, col), 0-based.
pub fn find_references(root: &Path, files: &[String], name: &str, limit: usize) -> Vec<(PathBuf, usize, usize, String)> {
    let Ok(re) = regex::Regex::new(&format!(r"\b{}\b", regex::escape(name))) else { return Vec::new() };
    let mut out = Vec::new();
    for rel in files {
        let path = root.join(rel);
        if std::fs::metadata(&path).map_or(true, |m| m.len() > 2_000_000) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if !text.contains(name) {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            for m in re.find_iter(line) {
                out.push((path.clone(), i, line[..m.start()].chars().count(), line.trim().to_string()));
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_outline() {
        let src = "struct Engine;\nimpl Engine {\n    fn render(&self) {}\n}\nfn resolve_layout() {}\ntrait Draw {}\n";
        let syms = outline(Path::new("a.rs"), src);
        let got: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind.as_str(), s.line)).collect();
        assert_eq!(got, vec![("Engine", "class", 0), ("render", "method", 2), ("resolve_layout", "function", 4), ("Draw", "interface", 5)]);
        assert_eq!(syms[1].col, 7);
        assert_eq!((syms[1].line, syms[1].end_line), (2, 2));
        assert_eq!(syms[0].end_line, 0);
    }

    #[test]
    fn typescript_and_python_outline() {
        let ts = "export class AnimationEngine {\n  resolveAnimatedLayout() {}\n}\ninterface Props {}\nfunction renderFrame() {}\n";
        let names: Vec<_> = outline(Path::new("x.ts"), ts).into_iter().map(|s| s.name).collect();
        for n in ["AnimationEngine", "resolveAnimatedLayout", "Props", "renderFrame"] {
            assert!(names.contains(&n.to_string()), "{n} missing from {names:?}");
        }
        let py = "class Foo:\n    def bar(self):\n        pass\ndef baz():\n    pass\n";
        let names: Vec<_> = outline(Path::new("x.py"), py).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["Foo", "bar", "baz"]);
        let go = "package main\nfunc Handle() {}\ntype Server struct{}\n";
        assert!(!outline(Path::new("x.go"), go).is_empty());
    }

    #[test]
    fn expands_selection() {
        let src = "fn main() {\n    let x = foo(1, 2);\n}\n";
        let one = src.find('1').unwrap();
        let r1 = expand(Path::new("a.rs"), src, (one, one)).unwrap();
        assert_eq!(&src[r1.0..r1.1], "1");
        let r2 = expand(Path::new("a.rs"), src, r1).unwrap();
        assert_eq!(&src[r2.0..r2.1], "(1, 2)");
        let r3 = expand(Path::new("a.rs"), src, r2).unwrap();
        assert_eq!(&src[r3.0..r3.1], "foo(1, 2)");
    }

    #[test]
    fn code_words() {
        assert!(looks_like_code("resolveAnimatedLayout"));
        assert!(looks_like_code("AnimationEngine"));
        assert!(looks_like_code("refresh_session"));
        assert!(!looks_like_code("render"));
        assert!(!looks_like_code("The"));
    }
}
