//! Detects `path:line` style file references in agent terminal output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use regex::Regex;

static REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        (?P<path>[\w@~.\-+/]*[\w\-+])
        (?:
            (?: : | \#L ) (?P<l1>\d+)
            (?: : (?P<c1>\d+) | -L? (?P<e1>\d+) )?
          |
            `? \)? ,? \s+ (?: (?:at|on|in|around|near) \s+ )? \(? lines? \s+ (?P<l2>\d+)
            (?: \s* [-–] \s* (?P<e2>\d+) )?
        )?",
    )
    .unwrap()
});

#[derive(Clone, Debug)]
pub struct FileRef {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub end_line: Option<usize>,
    pub col: Option<usize>,
    /// Set when a partial path (e.g. `index.ts`) matched several files.
    pub ambiguous: Option<String>,
    /// Screen segments covered by the reference: (row, start_col, end_col exclusive).
    pub segments: Vec<(u16, u16, u16)>,
}

impl FileRef {
    pub fn contains(&self, row: u16, col: u16) -> bool {
        self.segments
            .iter()
            .any(|&(r, s, e)| r == row && col >= s && col < e)
    }
}

/// All project files, for fuzzy open and resolving partial paths.
#[derive(Default)]
pub struct FileIndex {
    pub files: Vec<String>,
    by_name: HashMap<String, Vec<usize>>,
}

impl FileIndex {
    pub fn build(root: &Path) -> Self {
        let mut index = Self::default();
        let walk = WalkBuilder::new(root)
            .hidden(false)
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        for entry in walk.filter_map(Result::ok).take(200_000) {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let rel = relative(root, entry.path()).into_owned();
            let name = entry.file_name().to_string_lossy().into_owned();
            index.by_name.entry(name).or_default().push(index.files.len());
            index.files.push(rel);
        }
        index
    }

    /// Files whose relative path equals `partial` or ends with `/partial`.
    pub fn find_suffix(&self, partial: &str) -> Vec<&str> {
        let name = partial.rsplit('/').next().unwrap_or(partial);
        let Some(ids) = self.by_name.get(name) else { return Vec::new() };
        ids.iter()
            .map(|&i| self.files[i].as_str())
            .filter(|f| *f == partial || (f.ends_with(partial) && f.as_bytes()[f.len() - partial.len() - 1] == b'/'))
            .collect()
    }
}

#[derive(Clone)]
pub struct Resolved {
    pub path: PathBuf,
    pub ambiguous: bool,
}

/// Caches filesystem existence checks so we can scan every frame cheaply.
pub struct Resolver {
    root: PathBuf,
    home: Option<PathBuf>,
    index: Arc<FileIndex>,
    cache: HashMap<String, (Option<Resolved>, Instant)>,
}

impl Resolver {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            home: std::env::var_os("HOME").map(PathBuf::from),
            index: Arc::default(),
            cache: HashMap::new(),
        }
    }

    pub fn set_index(&mut self, index: Arc<FileIndex>) {
        self.index = index;
        self.cache.clear();
    }

    pub fn resolve(&mut self, token: &str) -> Option<Resolved> {
        if let Some((hit, at)) = self.cache.get(token) {
            if at.elapsed() < Duration::from_secs(2) {
                return hit.clone();
            }
        }
        let hit = self.resolve_uncached(token);
        if self.cache.len() > 4096 {
            self.cache.clear();
        }
        self.cache
            .insert(token.to_string(), (hit.clone(), Instant::now()));
        hit
    }

    fn resolve_uncached(&self, token: &str) -> Option<Resolved> {
        // Bare words are almost never references; require a separator or extension.
        if !token.contains('/') && !token.contains('.') {
            return None;
        }
        if token.chars().all(|c| c == '.' || c == '/') {
            return None;
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        let push = |p: &str, out: &mut Vec<PathBuf>| {
            if let Some(rest) = p.strip_prefix("~/") {
                if let Some(home) = &self.home {
                    out.push(home.join(rest));
                }
            } else if p.starts_with('/') {
                out.push(PathBuf::from(p));
            } else {
                out.push(self.root.join(p.trim_start_matches("./")));
            }
        };
        push(token, &mut candidates);
        for prefix in ["a/", "b/"] {
            if let Some(rest) = token.strip_prefix(prefix) {
                push(rest, &mut candidates);
            }
        }
        // Directories only count inside the project (skips shell prompts like `~/project$`).
        let direct = candidates
            .into_iter()
            .find(|p| p.is_file() || (p.is_dir() && p.starts_with(&self.root) && *p != self.root));
        if let Some(path) = direct {
            return Some(Resolved { path, ambiguous: false });
        }
        // Agents often shorten paths (`index.ts:146`, `engine/src/index.ts`); look them up.
        let partial = token.trim_start_matches("./");
        let partial = partial.strip_prefix("a/").or(partial.strip_prefix("b/")).unwrap_or(partial);
        if partial.starts_with('/') || partial.starts_with('~') || !partial.rsplit('/').next()?.contains('.') {
            return None;
        }
        let matches = self.index.find_suffix(partial);
        let first = matches.first()?;
        Some(Resolved { path: self.root.join(first), ambiguous: matches.len() > 1 })
    }
}

/// Scan the visible screen of a vt100 terminal for file references.
pub fn scan_screen(screen: &vt100::Screen, resolver: &mut Resolver) -> Vec<FileRef> {
    let (rows, cols) = screen.size();
    let mut refs = Vec::new();
    let mut text = String::new();
    // Screen position of every byte in `text`.
    let mut pos: Vec<(u16, u16)> = Vec::new();

    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let s = cell.contents();
            let s = if s.is_empty() { " " } else { s };
            text.push_str(s);
            pos.extend(std::iter::repeat_n((row, col), s.len()));
        }
        // Logical lines continue across soft-wrapped rows.
        if !screen.row_wrapped(row) || row + 1 == rows {
            scan_line(&text, &pos, resolver, &mut refs);
            text.clear();
            pos.clear();
        }
    }
    refs
}

fn scan_line(
    text: &str,
    pos: &[(u16, u16)],
    resolver: &mut Resolver,
    out: &mut Vec<FileRef>,
) {
    for caps in REF_RE.captures_iter(text) {
        let path_m = caps.name("path").unwrap();
        let Some(resolved) = resolver.resolve(path_m.as_str()) else { continue };
        let num = |name: &str| caps.name(name).and_then(|m| m.as_str().parse().ok());
        let whole = caps.get(0).unwrap();
        // Only extend the highlight over the line suffix when we understood it.
        let end = if num("l1").is_some() || num("l2").is_some() {
            whole.end()
        } else {
            path_m.end()
        };
        out.push(FileRef {
            path: resolved.path,
            ambiguous: resolved.ambiguous.then(|| path_m.as_str().to_string()),
            line: num("l1").or(num("l2")),
            end_line: num("e1").or(num("e2")),
            col: num("c1"),
            segments: segments(pos, path_m.start(), end),
        });
    }
}

fn segments(pos: &[(u16, u16)], start: usize, end: usize) -> Vec<(u16, u16, u16)> {
    let mut segs: Vec<(u16, u16, u16)> = Vec::new();
    for &(row, col) in &pos[start..end] {
        match segs.last_mut() {
            Some(seg) if seg.0 == row => seg.2 = seg.2.max(col + 1),
            _ => segs.push((row, col, col + 1)),
        }
    }
    segs
}

/// Parse user-typed `path:line` in quick open.
pub fn split_line_suffix(input: &str) -> (&str, Option<usize>) {
    if let Some((p, l)) = input.rsplit_once(':') {
        if let Ok(n) = l.parse() {
            return (p, Some(n));
        }
    }
    (input, None)
}

pub fn relative<'a>(root: &Path, path: &'a Path) -> std::borrow::Cow<'a, str> {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy(),
        Err(_) => path.to_string_lossy(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str, root: &Path) -> Vec<FileRef> {
        let mut parser = vt100::Parser::new(5, 80, 0);
        parser.process(text.as_bytes());
        let mut r = Resolver::new(root.to_path_buf());
        r.set_index(Arc::new(FileIndex::build(root)));
        scan_screen(parser.screen(), &mut r)
    }

    #[test]
    fn finds_refs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let refs = scan(
            "See src/main.rs:12:4 and `Cargo.toml`.\r\n● Read(src/refs.rs)\r\nsrc/app.rs (lines 10-20) nope.rs:3",
            root,
        );
        let got: Vec<_> = refs
            .iter()
            .map(|r| (relative(root, &r.path).to_string(), r.line, r.col, r.end_line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("src/main.rs".into(), Some(12), Some(4), None),
                ("Cargo.toml".into(), None, None, None),
                ("src/refs.rs".into(), None, None, None),
                ("src/app.rs".into(), Some(10), None, Some(20)),
            ]
        );
        assert_eq!(refs[0].segments, vec![(0, 4, 20)]);
    }

    #[test]
    fn partial_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let refs = scan("bug in editor.rs:88, see agent.rs around line 12; e.g. claude.ai", root);
        let got: Vec<_> = refs
            .iter()
            .map(|r| (relative(root, &r.path).to_string(), r.line, r.ambiguous.is_some()))
            .collect();
        assert_eq!(got, vec![("src/editor.rs".into(), Some(88), false), ("src/agent.rs".into(), Some(12), false)]);

        let mut index = FileIndex::default();
        for (i, f) in ["a/index.ts", "b/index.ts", "xindex.ts"].iter().enumerate() {
            index.files.push(f.to_string());
            index.by_name.entry(f.rsplit('/').next().unwrap().to_string()).or_default().push(i);
        }
        assert_eq!(index.find_suffix("index.ts"), vec!["a/index.ts", "b/index.ts"]);
        assert_eq!(index.find_suffix("b/index.ts"), vec!["b/index.ts"]);
    }

    #[test]
    fn wrapped_ref() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut parser = vt100::Parser::new(5, 10, 0);
        parser.process(b"xxxxx src/main.rs:7");
        let mut r = Resolver::new(root.to_path_buf());
        let refs = scan_screen(parser.screen(), &mut r);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].line, Some(7));
        assert_eq!(refs[0].segments, vec![(0, 6, 10), (1, 0, 9)]);
    }
}
