//! Text buffer with multiple cursors, syntax highlighting, search/replace,
//! folding and language-aware editing helpers.

mod lang;
mod view;

pub use view::Click;

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use regex::Regex;
use syntect::highlighting::{FontStyle, HighlightState, Highlighter, RangedHighlightIterator, Theme};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use unicode_width::UnicodeWidthChar;

use crate::git::LineMark;
use crate::settings;
use crate::symbols::Symbol;

const MAX_HIGHLIGHT_LINES: usize = 50_000;
const MAX_HIGHLIGHT_LINE_LEN: usize = 4_000;
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

pub struct Syntax {
    pub set: SyntaxSet,
    dark: Theme,
    light: Theme,
}

impl Syntax {
    pub fn load() -> Self {
        let themes = two_face::theme::extra();
        Self {
            set: two_face::syntax::extra_newlines(),
            dark: themes.get(two_face::theme::EmbeddedThemeName::CatppuccinMocha).clone(),
            light: themes.get(two_face::theme::EmbeddedThemeName::CatppuccinLatte).clone(),
        }
    }

    pub fn theme(&self) -> &Theme {
        if crate::theme::is_light() { &self.light } else { &self.dark }
    }
}

/// (line, char index)
pub type Pos = (usize, usize);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cursor {
    pub anchor: Pos,
    pub head: Pos,
    want: Option<usize>,
}

impl Cursor {
    fn at(p: Pos) -> Self {
        Self { anchor: p, head: p, want: None }
    }

    fn range(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

struct Snapshot {
    lines: Vec<String>,
    cursors: Vec<Cursor>,
}

#[derive(PartialEq, Clone, Copy)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

pub enum KeyResult {
    Handled,
    Ignored,
    Copy(String),
    Message(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SearchOpts {
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub in_selection: bool,
}

pub struct Search {
    pub opts: SearchOpts,
    pub matches: Vec<(Pos, Pos)>,
    pub current: Option<usize>,
    pub error: Option<String>,
    scope: Option<(Pos, Pos)>,
    re: Option<Regex>,
}

pub struct Doc {
    pub path: PathBuf,
    /// Preview tabs are replaced by the next preview until edited or pinned.
    pub preview: bool,
    pub pinned: bool,
    /// Show Markdown rendered instead of as source.
    pub md_preview: bool,
    pub md_scroll: usize,
    md_cache: Option<(u64, u16, bool, std::rc::Rc<Vec<crate::markdown::MdLine>>)>,
    lines: Vec<String>,
    cursors: Vec<Cursor>,
    top: (usize, usize),
    scroll_x: usize,
    reveal: bool,
    pub dirty: bool,
    mtime: Option<SystemTime>,
    crlf: bool,
    trailing_newline: bool,
    indent_unit: String,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(EditKind, Instant)>,
    flash: Option<(usize, usize, Instant)>,
    pub syntax_name: String,
    pub edits: u64,
    edited_at: Option<Instant>,
    pub marks: HashMap<usize, LineMark>,
    sel_stack: Vec<Vec<Cursor>>,
    hl_states: Vec<(ParseState, HighlightState)>,
    hl_lines: Vec<Vec<(Style, Range<usize>)>>,
    hl_light: bool,
    pub wrap: bool,
    folds: BTreeSet<usize>,
    hidden: Vec<(usize, usize)>,
    hidden_key: (u64, usize),
    pub search: Option<Search>,
    outline: (u64, Vec<Symbol>),
    view: view::ViewState,
}

impl Doc {
    pub fn open(path: &Path, syntax: &Syntax) -> io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(io::Error::other("file too large"));
        }
        let bytes = std::fs::read(path)?;
        if bytes[..bytes.len().min(8000)].contains(&0) {
            return Err(io::Error::other("binary file"));
        }
        let text = String::from_utf8(bytes).map_err(|_| io::Error::other("not UTF-8"))?;
        let mut doc = Self::from_text(path, &text, syntax);
        doc.mtime = meta.modified().ok();
        Ok(doc)
    }

    pub fn from_text(path: &Path, text: &str, syntax: &Syntax) -> Self {
        let mut doc = Self {
            path: path.to_path_buf(),
            preview: false,
            pinned: false,
            md_preview: crate::markdown::is_markdown(path),
            md_scroll: 0,
            md_cache: None,
            lines: Vec::new(),
            cursors: vec![Cursor::at((0, 0))],
            top: (0, 0),
            scroll_x: 0,
            reveal: true,
            dirty: false,
            mtime: None,
            crlf: false,
            trailing_newline: true,
            indent_unit: "    ".into(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            flash: None,
            syntax_name: String::new(),
            edits: 0,
            edited_at: None,
            marks: HashMap::new(),
            sel_stack: Vec::new(),
            hl_states: Vec::new(),
            hl_lines: Vec::new(),
            hl_light: false,
            wrap: false,
            folds: BTreeSet::new(),
            hidden: Vec::new(),
            hidden_key: (u64::MAX, 0),
            search: None,
            outline: (u64::MAX, Vec::new()),
            view: view::ViewState::default(),
        };
        doc.set_text(text);
        doc.init_highlight(syntax);
        doc
    }

    fn set_text(&mut self, text: &str) {
        self.crlf = text.contains("\r\n");
        self.trailing_newline = text.is_empty() || text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        self.lines = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let tabs = self.lines.iter().filter(|l| l.starts_with('\t')).count();
        let spaces = self.lines.iter().filter(|l| l.starts_with("  ")).count();
        let two = self.lines.iter().filter(|l| l.starts_with("  ") && !l.starts_with("   ")).count();
        self.indent_unit = if tabs > spaces {
            "\t".into()
        } else if spaces > 0 && two * 2 > spaces {
            "  ".into()
        } else {
            " ".repeat(settings::tab_width())
        };
        self.clamp_cursors();
    }

    fn init_highlight(&mut self, syntax: &Syntax) {
        let first = self.lines.first().map(String::as_str).unwrap_or("");
        let reference = syntax
            .set
            .find_syntax_for_file(&self.path)
            .ok()
            .flatten()
            .or_else(|| syntax.set.find_syntax_by_first_line(first))
            .unwrap_or_else(|| syntax.set.find_syntax_plain_text());
        self.syntax_name = reference.name.clone();
        let highlighter = Highlighter::new(syntax.theme());
        self.hl_states = vec![(ParseState::new(reference), HighlightState::new(&highlighter, ScopeStack::new()))];
        self.hl_lines.clear();
        self.hl_light = crate::theme::is_light();
    }

    fn invalidate(&mut self, from: usize) {
        self.hl_lines.truncate(from);
        self.hl_states.truncate(from + 1);
    }

    fn ensure_highlight(&mut self, upto: usize, syntax: &Syntax) {
        if self.hl_light != crate::theme::is_light() {
            self.init_highlight(syntax);
        }
        if self.lines.len() > MAX_HIGHLIGHT_LINES || self.hl_states.is_empty() {
            return;
        }
        let upto = upto.min(self.lines.len());
        if self.hl_lines.len() >= upto {
            return;
        }
        let highlighter = Highlighter::new(syntax.theme());
        while self.hl_lines.len() < upto {
            let i = self.hl_lines.len();
            let (mut ps, mut hs) = self.hl_states[i].clone();
            let mut spans = Vec::new();
            if self.lines[i].len() <= MAX_HIGHLIGHT_LINE_LEN {
                let line = format!("{}\n", self.lines[i]);
                let ops = ps.parse_line(&line, &syntax.set).unwrap_or_default();
                for (style, _, range) in RangedHighlightIterator::new(&mut hs, &ops, &line, &highlighter) {
                    let fg = style.foreground;
                    let mut s = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
                    if style.font_style.contains(FontStyle::BOLD) {
                        s = s.add_modifier(Modifier::BOLD);
                    }
                    if style.font_style.contains(FontStyle::ITALIC) {
                        s = s.add_modifier(Modifier::ITALIC);
                    }
                    spans.push((s, range));
                }
            }
            self.hl_lines.push(spans);
            self.hl_states.push((ps, hs));
        }
    }

    // ---- disk ----

    pub fn save(&mut self) -> io::Result<()> {
        let sep = if self.crlf { "\r\n" } else { "\n" };
        let mut text = self.lines.join(sep);
        if self.trailing_newline {
            text.push_str(sep);
        }
        std::fs::write(&self.path, text)?;
        self.mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        self.dirty = false;
        Ok(())
    }

    /// Returns Some(message) when the file changed on disk.
    pub fn check_disk(&mut self, syntax: &Syntax) -> Option<String> {
        let mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()?;
        if Some(mtime) == self.mtime {
            return None;
        }
        self.mtime = Some(mtime);
        let name = self.file_name();
        if self.dirty {
            return Some(format!("{name} changed on disk (you have unsaved edits)"));
        }
        let text = std::fs::read_to_string(&self.path).ok()?;
        self.push_undo(EditKind::Other);
        self.set_text(&text);
        self.changed(0);
        self.init_highlight(syntax);
        self.refresh_search();
        Some(format!("{name} reloaded (changed on disk)"))
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    // ---- basic queries ----

    fn primary(&self) -> &Cursor {
        self.cursors.last().expect("at least one cursor")
    }

    pub fn cursor_count(&self) -> usize {
        self.cursors.len()
    }

    pub fn cursor_line(&self) -> usize {
        self.primary().head.0 + 1
    }

    pub fn cursor_col(&self) -> usize {
        self.primary().head.1 + 1
    }

    pub fn cursor_line0(&self) -> usize {
        self.primary().head.0
    }

    pub fn line_text(&self, line: usize) -> Option<&str> {
        self.lines.get(line).map(String::as_str)
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn edited_at(&self) -> Option<Instant> {
        self.edited_at
    }

    pub fn flash_active(&self) -> bool {
        self.flash.is_some_and(|(_, _, t)| t.elapsed().as_millis() < 1500)
    }

    fn line_chars(&self, line: usize) -> usize {
        self.lines[line].chars().count()
    }

    fn first_non_ws(&self, line: usize) -> usize {
        self.lines[line].chars().take_while(|c| c.is_whitespace()).count()
    }

    fn clamp(&self, (l, c): Pos) -> Pos {
        let l = l.min(self.lines.len() - 1);
        (l, c.min(self.line_chars(l)))
    }

    fn clamp_cursors(&mut self) {
        for i in 0..self.cursors.len() {
            let c = self.cursors[i];
            self.cursors[i].anchor = self.clamp(c.anchor);
            self.cursors[i].head = self.clamp(c.head);
        }
    }

    fn byte_idx(&self, line: usize, cx: usize) -> usize {
        let l = &self.lines[line];
        l.char_indices().nth(cx).map_or(l.len(), |(b, _)| b)
    }

    fn char_at(&self, (l, c): Pos) -> Option<char> {
        self.lines.get(l)?.chars().nth(c)
    }

    fn char_before(&self, (l, c): Pos) -> Option<char> {
        if c == 0 { None } else { self.lines.get(l)?.chars().nth(c - 1) }
    }

    pub fn display_col(&self, line: usize, cx: usize) -> usize {
        let mut col = 0;
        for c in self.lines[line].chars().take(cx) {
            col += char_width(c, col);
        }
        col
    }

    fn cx_for_display_col(&self, line: usize, target: usize) -> usize {
        let mut col = 0;
        for (i, c) in self.lines[line].chars().enumerate() {
            let w = char_width(c, col);
            if col + w > target {
                return i;
            }
            col += w;
        }
        self.line_chars(line)
    }

    fn text_range(&self, (s, e): (Pos, Pos)) -> String {
        let (sl, sc) = s;
        let (el, ec) = e;
        if sl == el {
            return self.lines[sl][self.byte_idx(sl, sc)..self.byte_idx(sl, ec)].to_string();
        }
        let mut out = self.lines[sl][self.byte_idx(sl, sc)..].to_string();
        for l in &self.lines[sl + 1..el] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[el][..self.byte_idx(el, ec)]);
        out
    }

    pub fn selected_text(&self) -> Option<String> {
        let parts: Vec<String> = self.cursors.iter().filter(|c| !c.is_empty()).map(|c| self.text_range(c.range())).collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    /// 1-based inclusive line range of the primary selection, or the cursor line.
    pub fn selected_lines(&self) -> (usize, usize) {
        let ((sl, _), (el, ec)) = self.primary().range();
        let el = if ec == 0 && el > sl { el - 1 } else { el };
        (sl + 1, el + 1)
    }

    /// Rendered Markdown lines for the preview, cached per edit and width.
    pub fn markdown_lines(&mut self, width: u16) -> std::rc::Rc<Vec<crate::markdown::MdLine>> {
        let light = crate::theme::is_light();
        if let Some((edits, w, l, lines)) = &self.md_cache {
            if *edits == self.edits && *w == width && *l == light {
                return lines.clone();
            }
        }
        let lines = std::rc::Rc::new(crate::markdown::render(&self.text(), width as usize));
        self.md_cache = Some((self.edits, width, light, lines.clone()));
        lines
    }

    pub fn uses_tabs(&self) -> bool {
        self.indent_unit == "\t"
    }

    pub fn has_selection(&self) -> bool {
        !self.primary().is_empty()
    }

    // ---- core edit primitive ----

    fn byte_of(&self, (line, cx): Pos) -> usize {
        self.lines[..line].iter().map(|l| l.len() + 1).sum::<usize>() + self.byte_idx(line, cx)
    }

    fn pos_of_byte(&self, mut b: usize) -> Pos {
        for (i, l) in self.lines.iter().enumerate() {
            if b <= l.len() {
                return (i, l.get(..b).map_or(l.chars().count(), |s| s.chars().count()));
            }
            b -= l.len() + 1;
        }
        let last = self.lines.len() - 1;
        (last, self.line_chars(last))
    }

    /// Replace `s..e` with `text`, shifting every cursor, fold and search
    /// match after the edit. Returns the end of the inserted text.
    fn replace(&mut self, s: Pos, e: Pos, text: &str) -> Pos {
        let (s, e) = if s <= e { (s, e) } else { (e, s) };
        let sb = self.byte_idx(s.0, s.1);
        let eb = self.byte_idx(e.0, e.1);
        let tail = self.lines[e.0][eb..].to_string();
        let head = self.lines[s.0][..sb].to_string();
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        let mut new_lines: Vec<String> = vec![format!("{head}{first}")];
        new_lines.extend(parts.map(String::from));
        let last_len = new_lines.last().unwrap().chars().count();
        let ne = (s.0 + new_lines.len() - 1, if new_lines.len() == 1 { s.1 + first.chars().count() } else { last_len });
        new_lines.last_mut().unwrap().push_str(&tail);
        self.lines.splice(s.0..=e.0, new_lines);

        let dl = ne.0 as isize - e.0 as isize;
        let adjust = |p: Pos| -> Pos {
            if p < s {
                p
            } else if p >= e {
                if p.0 == e.0 { (ne.0, ne.1 + (p.1 - e.1)) } else { ((p.0 as isize + dl) as usize, p.1) }
            } else {
                ne
            }
        };
        for c in &mut self.cursors {
            c.anchor = adjust(c.anchor);
            c.head = adjust(c.head);
        }
        if dl != 0 {
            self.folds = self
                .folds
                .iter()
                .filter(|&&f| f <= s.0 || f > e.0)
                .map(|&f| if f > e.0 { (f as isize + dl) as usize } else { f })
                .collect();
        }
        self.changed(s.0);
        ne
    }

    fn changed(&mut self, from_line: usize) {
        self.edits += 1;
        self.edited_at = Some(Instant::now());
        self.sel_stack.clear();
        self.invalidate(from_line);
    }

    fn push_undo(&mut self, kind: EditKind) {
        let coalesce = kind != EditKind::Other && self.last_edit.is_some_and(|(k, t)| k == kind && t.elapsed().as_millis() < 1000);
        self.last_edit = Some((kind, Instant::now()));
        if !coalesce {
            self.undo.push(Snapshot { lines: self.lines.clone(), cursors: self.cursors.clone() });
            if self.undo.len() > 300 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    fn restore(&mut self, from_undo: bool) {
        let (src, dst) = if from_undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(snap) = src.pop() else { return };
        dst.push(Snapshot { lines: std::mem::replace(&mut self.lines, snap.lines), cursors: std::mem::replace(&mut self.cursors, snap.cursors) });
        self.last_edit = None;
        self.clamp_cursors();
        self.changed(0);
        self.dirty = true;
        self.reveal = true;
        self.refresh_search();
    }

    fn after_edit(&mut self) {
        self.dirty = true;
        self.reveal = true;
        for c in &mut self.cursors {
            c.want = None;
        }
        self.merge_cursors();
        self.refresh_search();
    }

    /// Cursor indices ordered bottom-to-top, so edits don't shift unprocessed cursors.
    fn order_desc(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.cursors.len()).collect();
        idx.sort_by(|&a, &b| self.cursors[b].range().0.cmp(&self.cursors[a].range().0));
        idx
    }

    fn merge_cursors(&mut self) {
        if self.cursors.len() < 2 {
            return;
        }
        let primary = *self.primary();
        let mut cs = self.cursors.clone();
        cs.sort_by_key(|c| c.range().0);
        let mut out: Vec<Cursor> = Vec::new();
        for c in cs {
            if let Some(last) = out.last_mut() {
                let (ls, le) = last.range();
                let (cs_, ce) = c.range();
                if cs_ < le || (cs_ == le && (c.is_empty() || last.is_empty())) || c.range() == (ls, le) {
                    let (ns, ne) = (ls.min(cs_), le.max(ce));
                    let forward = last.head >= last.anchor;
                    *last = if forward { Cursor { anchor: ns, head: ne, want: None } } else { Cursor { anchor: ne, head: ns, want: None } };
                    continue;
                }
            }
            out.push(c);
        }
        // Keep the primary last.
        if let Some(i) = out.iter().position(|c| c.range().0 <= primary.head && primary.head <= c.range().1) {
            let p = out.remove(i);
            out.push(p);
        }
        self.cursors = out;
    }

    // ---- editing operations ----

    pub fn insert_text(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.push_undo(if text.chars().count() == 1 { EditKind::Insert } else { EditKind::Other });
        let pieces: Vec<&str> = text.split('\n').collect();
        // Pasting one line per cursor distributes the lines.
        let distribute = self.cursors.len() > 1 && pieces.len() == self.cursors.len();
        let mut order: Vec<usize> = (0..self.cursors.len()).collect();
        order.sort_by_key(|&i| self.cursors[i].range().0);
        let assignment: HashMap<usize, usize> = order.iter().enumerate().map(|(k, &i)| (i, k)).collect();
        for i in self.order_desc() {
            let (s, e) = self.cursors[i].range();
            let piece = if distribute { pieces[assignment[&i]] } else { text.as_str() };
            let ne = self.replace(s, e, piece);
            self.cursors[i] = Cursor::at(ne);
        }
        self.after_edit();
    }

    fn type_char(&mut self, ch: char) {
        let auto = settings::auto_close();
        self.push_undo(EditKind::Insert);
        for i in self.order_desc() {
            let c = self.cursors[i];
            let (s, e) = c.range();
            let next = self.char_at(e);
            let prev = self.char_before(s);
            // Wrap a selection in brackets/quotes.
            if auto && !c.is_empty() {
                if let Some(close) = lang::closing(ch) {
                    let inner = self.text_range((s, e));
                    self.replace(s, e, &format!("{ch}{inner}{close}"));
                    let start = (s.0, s.1 + 1);
                    let end = self.pos_of_byte(self.byte_of(start) + inner.len());
                    self.cursors[i] = Cursor { anchor: start, head: end, want: None };
                    continue;
                }
            }
            // Type over an auto-inserted closer.
            if auto && c.is_empty() && lang::is_closer(ch) && next == Some(ch) {
                self.cursors[i] = Cursor::at((s.0, s.1 + 1));
                continue;
            }
            let pair = lang::closing(ch).filter(|_| auto && c.is_empty()).filter(|_| {
                let next_ok = next.is_none_or(|n| n.is_whitespace() || lang::is_closer(n) || matches!(n, ',' | ';' | ':'));
                let quote = matches!(ch, '"' | '\'' | '`');
                next_ok && !(quote && prev.is_some_and(|p| p.is_alphanumeric() || p == ch || p == '\\'))
            });
            let ne = match pair {
                Some(close) => {
                    let end = self.replace(s, e, &format!("{ch}{close}"));
                    (end.0, end.1 - 1)
                }
                None => self.replace(s, e, ch.encode_utf8(&mut [0; 4])),
            };
            self.cursors[i] = Cursor::at(ne);
            if ch == '>' && auto && lang::closes_tags(&self.path) {
                let before: String = self.lines[ne.0].chars().take(ne.1).collect();
                if let Some(name) = lang::open_tag_name(&before) {
                    self.replace(ne, ne, &format!("</{name}>"));
                    self.cursors[i] = Cursor::at(ne);
                }
            }
        }
        self.after_edit();
    }

    fn newline(&mut self) {
        self.push_undo(EditKind::Other);
        for i in self.order_desc() {
            let (s, e) = self.cursors[i].range();
            let line = &self.lines[s.0];
            let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').take(s.1).collect();
            let before = line.chars().take(s.1).collect::<String>();
            let trimmed = before.trim_end();
            let opens = trimmed.ends_with(['{', '[', '(']) || (trimmed.ends_with(':') && self.path.extension().is_some_and(|x| x == "py"));
            let next = self.char_at(e);
            let closes = opens && next.is_some_and(|n| matches!(n, '}' | ']' | ')'));
            let inner = if opens { format!("{indent}{}", self.indent_unit) } else { indent.clone() };
            let text = if closes { format!("\n{inner}\n{indent}") } else { format!("\n{inner}") };
            self.replace(s, e, &text);
            self.cursors[i] = Cursor::at((s.0 + 1, inner.chars().count()));
        }
        self.after_edit();
    }

    fn backspace(&mut self) {
        self.push_undo(EditKind::Delete);
        for i in self.order_desc() {
            let c = self.cursors[i];
            let (s, e) = c.range();
            if !c.is_empty() {
                let ne = self.replace(s, e, "");
                self.cursors[i] = Cursor::at(ne);
                continue;
            }
            let p = c.head;
            if p.1 == 0 {
                if p.0 > 0 {
                    let prev = (p.0 - 1, self.line_chars(p.0 - 1));
                    let ne = self.replace(prev, p, "");
                    self.cursors[i] = Cursor::at(ne);
                }
                continue;
            }
            let before: String = self.lines[p.0].chars().take(p.1).collect();
            let unit_w = self.indent_unit.chars().count();
            let (from, to) = if !before.is_empty() && before.chars().all(|c| c == ' ') && unit_w > 1 && self.indent_unit != "\t" {
                let remove = match p.1 % unit_w {
                    0 => unit_w,
                    r => r,
                };
                ((p.0, p.1 - remove), p)
            } else if let (Some(prev), Some(next)) = (self.char_before(p), self.char_at(p)) {
                if settings::auto_close() && lang::closing(prev) == Some(next) {
                    ((p.0, p.1 - 1), (p.0, p.1 + 1))
                } else {
                    ((p.0, p.1 - 1), p)
                }
            } else {
                ((p.0, p.1 - 1), p)
            };
            let ne = self.replace(from, to, "");
            self.cursors[i] = Cursor::at(ne);
        }
        self.after_edit();
    }

    fn delete_forward(&mut self) {
        self.push_undo(EditKind::Delete);
        for i in self.order_desc() {
            let c = self.cursors[i];
            let (s, e) = c.range();
            let (from, to) = if !c.is_empty() {
                (s, e)
            } else if e.1 < self.line_chars(e.0) {
                (e, (e.0, e.1 + 1))
            } else if e.0 + 1 < self.lines.len() {
                (e, (e.0 + 1, 0))
            } else {
                continue;
            };
            let ne = self.replace(from, to, "");
            self.cursors[i] = Cursor::at(ne);
        }
        self.after_edit();
    }

    /// Unique line blocks covered by the cursors, bottom first.
    fn line_blocks(&self) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = self
            .cursors
            .iter()
            .map(|c| {
                let ((sl, _), (el, ec)) = c.range();
                (sl, if ec == 0 && el > sl { el - 1 } else { el })
            })
            .collect();
        blocks.sort();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for b in blocks {
            match merged.last_mut() {
                Some(m) if b.0 <= m.1 + 1 => m.1 = m.1.max(b.1),
                _ => merged.push(b),
            }
        }
        merged.reverse();
        merged
    }

    fn indent_lines(&mut self, dedent: bool) {
        let multi_line = self.cursors.iter().any(|c| c.range().0.0 != c.range().1.0);
        if !dedent && !multi_line {
            let unit = self.indent_unit.clone();
            return self.insert_text(&unit);
        }
        self.push_undo(EditKind::Other);
        let unit = self.indent_unit.clone();
        for (a, b) in self.line_blocks() {
            for l in (a..=b).rev() {
                if dedent {
                    let n = if self.lines[l].starts_with(&unit) {
                        unit.chars().count()
                    } else {
                        self.lines[l].chars().take_while(|c| *c == ' ').count().min(unit.chars().count())
                    };
                    if n > 0 {
                        self.replace((l, 0), (l, n), "");
                    }
                } else if !self.lines[l].trim().is_empty() {
                    self.replace((l, 0), (l, 0), &unit);
                }
            }
        }
        self.after_edit();
    }

    pub fn toggle_comment(&mut self) -> bool {
        let Some(comment) = lang::comment_for(&self.path) else { return false };
        self.push_undo(EditKind::Other);
        for (a, b) in self.line_blocks() {
            match comment {
                lang::Comment::Line(tok) => {
                    let body: Vec<usize> = (a..=b).filter(|&l| !self.lines[l].trim().is_empty()).collect();
                    let all = !body.is_empty() && body.iter().all(|&l| self.lines[l].trim_start().starts_with(tok));
                    let indent = body.iter().map(|&l| self.first_non_ws(l)).min().unwrap_or(0);
                    for &l in body.iter().rev() {
                        if all {
                            let at = self.first_non_ws(l);
                            let rest: String = self.lines[l].chars().skip(at + tok.chars().count()).collect();
                            let n = tok.chars().count() + usize::from(rest.starts_with(' '));
                            self.replace((l, at), (l, at + n), "");
                        } else {
                            self.replace((l, indent), (l, indent), &format!("{tok} "));
                        }
                    }
                }
                lang::Comment::Block(open, close) => {
                    let first = self.first_non_ws(a);
                    let last_len = self.line_chars(b);
                    let text = self.text_range(((a, first), (b, last_len)));
                    let trimmed = text.trim();
                    if trimmed.starts_with(open) && trimmed.ends_with(close) {
                        let inner = trimmed[open.len()..trimmed.len() - close.len()].trim().to_string();
                        self.replace((a, first), (b, last_len), &inner);
                    } else {
                        self.replace((a, first), (b, last_len), &format!("{open} {text} {close}"));
                    }
                }
            }
        }
        self.after_edit();
        true
    }

    fn move_lines(&mut self, up: bool) {
        let blocks = self.line_blocks();
        if blocks.iter().any(|&(a, b)| if up { a == 0 } else { b + 1 >= self.lines.len() }) {
            return;
        }
        self.push_undo(EditKind::Other);
        self.folds.clear();
        let ordered: Vec<(usize, usize)> = if up { blocks.iter().rev().copied().collect() } else { blocks };
        for (a, b) in ordered {
            let delta: isize = if up { -1 } else { 1 };
            if up {
                let line = self.lines.remove(a - 1);
                self.lines.insert(b, line);
            } else {
                let line = self.lines.remove(b + 1);
                self.lines.insert(a, line);
            }
            for c in &mut self.cursors {
                for p in [&mut c.anchor, &mut c.head] {
                    if p.0 >= a && p.0 <= b {
                        p.0 = (p.0 as isize + delta) as usize;
                    }
                }
            }
            self.changed(a.saturating_sub(1));
        }
        self.after_edit();
    }

    fn duplicate_lines(&mut self, up: bool) {
        self.push_undo(EditKind::Other);
        for (a, b) in self.line_blocks() {
            let block: Vec<String> = self.lines[a..=b].to_vec();
            let n = block.len();
            let end = (b, self.line_chars(b));
            self.replace(end, end, &format!("\n{}", block.join("\n")));
            if !up {
                for c in &mut self.cursors {
                    for p in [&mut c.anchor, &mut c.head] {
                        if p.0 >= a && p.0 <= b {
                            p.0 += n;
                        }
                    }
                }
            }
        }
        self.after_edit();
    }

    fn delete_lines(&mut self) -> String {
        self.push_undo(EditKind::Other);
        let mut removed = Vec::new();
        for (a, b) in self.line_blocks() {
            removed.push(self.lines[a..=b].join("\n") + "\n");
            if self.lines.len() == b - a + 1 {
                self.replace((0, 0), (b, self.line_chars(b)), "");
            } else if b + 1 < self.lines.len() {
                self.replace((a, 0), (b + 1, 0), "");
            } else {
                let prev = (a - 1, self.line_chars(a - 1));
                self.replace(prev, (b, self.line_chars(b)), "");
            }
        }
        for c in &mut self.cursors {
            *c = Cursor::at((c.head.0, 0));
        }
        self.clamp_cursors();
        self.after_edit();
        removed.reverse();
        removed.concat()
    }

    /// Apply LSP text edits given as ((line, utf16), (line, utf16), text).
    pub fn apply_lsp_edits(&mut self, mut edits: Vec<((usize, usize), (usize, usize), String)>) {
        if edits.is_empty() {
            return;
        }
        self.push_undo(EditKind::Other);
        edits.sort_by(|a, b| b.0.cmp(&a.0));
        for ((sl, s16), (el, e16), text) in edits {
            let sl = sl.min(self.lines.len() - 1);
            let el = el.min(self.lines.len() - 1);
            let s = (sl, self.char_col_from_utf16(sl, s16));
            let e = (el, self.char_col_from_utf16(el, e16));
            self.replace(s, e, &text);
        }
        self.clamp_cursors();
        self.after_edit();
    }

    // ---- cursors & movement ----

    fn move_all(&mut self, extend: bool, f: impl Fn(&Doc, &Cursor) -> Pos) {
        for i in 0..self.cursors.len() {
            let c = self.cursors[i];
            let p = f(self, &c);
            let want = c.want;
            self.cursors[i].head = p;
            if !extend {
                self.cursors[i].anchor = p;
            }
            self.cursors[i].want = want;
        }
        self.reveal = true;
        self.merge_cursors();
    }

    fn left_of(&self, p: Pos) -> Pos {
        if p.1 > 0 {
            (p.0, p.1 - 1)
        } else if p.0 > 0 {
            let l = self.view_prev_line(p.0);
            (l, self.line_chars(l))
        } else {
            p
        }
    }

    fn right_of(&self, p: Pos) -> Pos {
        if p.1 < self.line_chars(p.0) {
            (p.0, p.1 + 1)
        } else if let Some(l) = self.view_next_line(p.0) {
            (l, 0)
        } else {
            p
        }
    }

    fn word_left(&self, p: Pos) -> Pos {
        if p.1 == 0 {
            return self.left_of(p);
        }
        let chars: Vec<char> = self.lines[p.0].chars().collect();
        let mut i = p.1;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let word = i > 0 && is_word(chars[i - 1]);
        while i > 0 && !chars[i - 1].is_whitespace() && is_word(chars[i - 1]) == word {
            i -= 1;
        }
        (p.0, i)
    }

    fn word_right(&self, p: Pos) -> Pos {
        let chars: Vec<char> = self.lines[p.0].chars().collect();
        if p.1 >= chars.len() {
            return self.right_of(p);
        }
        let mut i = p.1;
        let word = is_word(chars[i]);
        while i < chars.len() && !chars[i].is_whitespace() && is_word(chars[i]) == word {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        (p.0, i)
    }

    fn vertical(&mut self, rows: isize, extend: bool) {
        for i in 0..self.cursors.len() {
            let c = self.cursors[i];
            let want = c.want.unwrap_or_else(|| self.view_col(c.head));
            let p = self.view_move_rows(c.head, want, rows);
            self.cursors[i].head = p;
            if !extend {
                self.cursors[i].anchor = p;
            }
            self.cursors[i].want = Some(want);
        }
        self.reveal = true;
        self.merge_cursors();
    }

    pub fn goto(&mut self, line: usize, col: Option<usize>, end_line: Option<usize>) {
        let l = line.saturating_sub(1).min(self.lines.len() - 1);
        let c = match col {
            Some(c) => c.saturating_sub(1),
            None => self.first_non_ws(l),
        };
        let p = self.clamp((l, c));
        self.cursors = vec![Cursor::at(p)];
        self.unfold_at(l);
        let end = end_line.map(|e| e.saturating_sub(1).clamp(l, self.lines.len() - 1)).unwrap_or(l);
        self.flash = Some((l, end, Instant::now()));
        self.view_center(l);
    }

    pub fn select_all(&mut self) {
        let last = self.lines.len() - 1;
        self.cursors = vec![Cursor { anchor: (0, 0), head: (last, self.line_chars(last)), want: None }];
    }

    fn word_range_at(&self, p: Pos) -> Option<(Pos, Pos)> {
        let chars: Vec<char> = self.lines[p.0].chars().collect();
        let mut s = p.1.min(chars.len());
        if (s == chars.len() || !is_word(chars[s])) && s > 0 && is_word(chars[s - 1]) {
            s -= 1;
        }
        if s >= chars.len() || !is_word(chars[s]) {
            return None;
        }
        let mut e = s;
        while s > 0 && is_word(chars[s - 1]) {
            s -= 1;
        }
        while e < chars.len() && is_word(chars[e]) {
            e += 1;
        }
        Some(((p.0, s), (p.0, e)))
    }

    pub fn word_at_cursor(&self) -> Option<String> {
        self.word_range_at(self.primary().head).map(|r| self.text_range(r))
    }

    /// Ctrl+D: select the word, then add the next occurrence as a new cursor.
    pub fn add_next_occurrence(&mut self) -> bool {
        let p = *self.primary();
        if p.is_empty() {
            let Some((s, e)) = self.word_range_at(p.head) else { return false };
            let last = self.cursors.len() - 1;
            self.cursors[last] = Cursor { anchor: s, head: e, want: None };
            return true;
        }
        let needle = self.text_range(p.range());
        if needle.contains('\n') {
            return false;
        }
        let start = self.cursors.iter().map(|c| c.range().1).max().unwrap_or(p.head);
        let n = self.lines.len();
        for k in 0..=n {
            let l = (start.0 + k) % n;
            let from = if k == 0 { self.byte_idx(l, start.1) } else { 0 };
            if let Some(b) = self.lines[l][from..].find(&needle).map(|b| b + from) {
                let sc = self.lines[l][..b].chars().count();
                let r = ((l, sc), (l, sc + needle.chars().count()));
                if self.cursors.iter().any(|c| c.range() == r) {
                    return false;
                }
                self.cursors.push(Cursor { anchor: r.0, head: r.1, want: None });
                self.reveal = true;
                return true;
            }
        }
        false
    }

    pub fn add_cursor_vertical(&mut self, down: bool) {
        let p = *self.primary();
        let want = self.view_col(p.head);
        let np = self.view_move_rows(p.head, want, if down { 1 } else { -1 });
        if np.0 != p.head.0 {
            self.cursors.push(Cursor { anchor: np, head: np, want: Some(want) });
            self.reveal = true;
            self.merge_cursors();
        }
    }

    pub fn select_all_matches(&mut self) -> usize {
        let Some(search) = &self.search else { return 0 };
        if search.matches.is_empty() {
            return 0;
        }
        self.cursors = search.matches.iter().map(|&(s, e)| Cursor { anchor: s, head: e, want: None }).collect();
        self.cursors.len()
    }

    fn collapse_cursors(&mut self) {
        if self.cursors.len() > 1 {
            let p = *self.primary();
            self.cursors = vec![p];
        } else {
            let h = self.primary().head;
            self.cursors = vec![Cursor::at(h)];
        }
    }

    // ---- structure-aware helpers ----

    pub fn selection_bytes(&self) -> (usize, usize) {
        let (s, e) = self.primary().range();
        (self.byte_of(s), self.byte_of(e))
    }

    pub fn expand_to(&mut self, (start, end): (usize, usize)) {
        self.sel_stack.push(self.cursors.clone());
        self.cursors = vec![Cursor { anchor: self.pos_of_byte(start), head: self.pos_of_byte(end), want: None }];
        self.reveal = true;
    }

    pub fn shrink_selection(&mut self) -> bool {
        match self.sel_stack.pop() {
            Some(c) => {
                self.cursors = c;
                self.reveal = true;
                true
            }
            None => false,
        }
    }

    pub fn utf16_col(&self) -> usize {
        let h = self.primary().head;
        self.lines[h.0].chars().take(h.1).map(char::len_utf16).sum()
    }

    pub fn char_col_from_utf16(&self, line: usize, utf16: usize) -> usize {
        let Some(l) = self.lines.get(line) else { return 0 };
        let mut units = 0;
        for (i, c) in l.chars().enumerate() {
            if units >= utf16 {
                return i;
            }
            units += c.len_utf16();
        }
        l.chars().count()
    }

    /// Tree-sitter outline, refreshed lazily after edits settle.
    pub fn outline(&mut self) -> &[Symbol] {
        let stale = self.outline.0 != self.edits;
        let settled = self.edited_at.is_none_or(|t| t.elapsed().as_millis() > 400);
        if stale && (settled || self.outline.0 == u64::MAX) {
            let symbols = crate::symbols::outline(&self.path, &self.text());
            self.outline = (self.edits, symbols);
        }
        &self.outline.1
    }

    /// Symbols enclosing a line, outermost first.
    pub fn scopes_at(&mut self, line: usize) -> Vec<Symbol> {
        let mut scopes: Vec<Symbol> = self.outline().iter().filter(|s| s.line <= line && line <= s.end_line).cloned().collect();
        scopes.sort_by_key(|s| (s.line, std::cmp::Reverse(s.end_line)));
        scopes
    }

    // ---- search & replace ----

    pub fn set_search(&mut self, query: &str, opts: SearchOpts) {
        let scope = if opts.in_selection {
            self.search.as_ref().and_then(|s| s.scope).or_else(|| self.has_selection().then(|| self.primary().range()))
        } else {
            None
        };
        let mut search = Search { opts, matches: Vec::new(), current: None, error: None, scope, re: None };
        if !query.is_empty() {
            let mut pat = if opts.regex { query.to_string() } else { regex::escape(query) };
            if opts.word {
                pat = format!(r"\b(?:{pat})\b");
            }
            // Smart case unless "match case" is on.
            if !opts.case && !query.chars().any(char::is_uppercase) {
                pat = format!("(?i){pat}");
            }
            match Regex::new(&pat) {
                Ok(re) => search.re = Some(re),
                Err(e) => search.error = Some(e.to_string().lines().last().unwrap_or("invalid regex").to_string()),
            }
        }
        self.search = Some(search);
        self.refresh_search();
        self.select_nearest_match();
    }

    fn refresh_search(&mut self) {
        let Some(search) = &mut self.search else { return };
        search.matches.clear();
        let Some(re) = &search.re else { return };
        for (l, line) in self.lines.iter().enumerate() {
            for m in re.find_iter(line) {
                if m.start() == m.end() {
                    continue;
                }
                let s = (l, line[..m.start()].chars().count());
                let e = (l, s.1 + line[m.start()..m.end()].chars().count());
                if search.scope.is_some_and(|(a, b)| s < a || e > b) {
                    continue;
                }
                search.matches.push((s, e));
                if search.matches.len() >= 20_000 {
                    return;
                }
            }
        }
        search.current = search.current.filter(|&c| c < search.matches.len());
    }

    fn select_nearest_match(&mut self) {
        let head = self.primary().range().0;
        let Some(search) = &mut self.search else { return };
        if let Some(i) = search.matches.iter().position(|m| m.0 >= head).or((!search.matches.is_empty()).then_some(0)) {
            search.current = Some(i);
            let (s, e) = search.matches[i];
            self.view_center(s.0);
            self.unfold_at(s.0);
            self.highlight_match(s, e);
        }
    }

    fn highlight_match(&mut self, s: Pos, e: Pos) {
        self.cursors = vec![Cursor { anchor: s, head: e, want: None }];
        self.reveal = true;
    }

    pub fn find_step(&mut self, forward: bool) -> bool {
        let head = self.primary().range();
        let Some(search) = &mut self.search else { return false };
        let n = search.matches.len();
        if n == 0 {
            return false;
        }
        let i = if forward {
            search.matches.iter().position(|m| m.0 >= head.1 && *m != head).unwrap_or(0)
        } else {
            search.matches.iter().rposition(|m| m.1 <= head.0 && *m != head).unwrap_or(n - 1)
        };
        search.current = Some(i);
        let (s, e) = search.matches[i];
        self.unfold_at(s.0);
        self.highlight_match(s, e);
        true
    }

    fn expand_replacement(&self, matched: &str, replacement: &str) -> String {
        match self.search.as_ref().and_then(|s| s.re.as_ref()).filter(|_| self.search.as_ref().is_some_and(|s| s.opts.regex)) {
            Some(re) => re.replace(matched, replacement).into_owned(),
            None => replacement.to_string(),
        }
    }

    pub fn replace_current(&mut self, replacement: &str) -> bool {
        let Some(search) = &self.search else { return false };
        let sel = self.primary().range();
        let Some(&(s, e)) = search.matches.iter().find(|&&m| m == sel) else {
            return self.find_step(true);
        };
        self.push_undo(EditKind::Other);
        let text = self.expand_replacement(&self.text_range((s, e)), replacement);
        let ne = self.replace(s, e, &text);
        self.cursors = vec![Cursor::at(ne)];
        self.after_edit();
        self.find_step(true);
        true
    }

    pub fn replace_all(&mut self, replacement: &str) -> usize {
        let Some(search) = &self.search else { return 0 };
        let matches = search.matches.clone();
        if matches.is_empty() {
            return 0;
        }
        self.push_undo(EditKind::Other);
        for &(s, e) in matches.iter().rev() {
            let text = self.expand_replacement(&self.text_range((s, e)), replacement);
            self.replace(s, e, &text);
        }
        self.clamp_cursors();
        self.after_edit();
        matches.len()
    }

    pub fn clear_search(&mut self) {
        self.search = None;
    }

    // ---- folding ----

    fn indent_of(&self, line: usize) -> Option<usize> {
        let l = &self.lines[line];
        if l.trim().is_empty() {
            return None;
        }
        Some(self.display_col(line, self.first_non_ws(line)))
    }

    /// Indentation-based fold range starting at `line`, if any.
    pub fn fold_range(&self, line: usize) -> Option<(usize, usize)> {
        let base = self.indent_of(line)?;
        let mut end = None;
        for l in line + 1..self.lines.len() {
            match self.indent_of(l) {
                None => continue,
                Some(i) if i > base => end = Some(l),
                Some(_) => break,
            }
        }
        end.map(|e| (line, e))
    }

    pub fn toggle_fold(&mut self, line: usize) -> bool {
        if self.folds.remove(&line) {
            self.hidden_key = (u64::MAX, 0);
            return true;
        }
        // Fold the innermost range containing the line.
        let Some((start, end)) = (0..=line).rev().find_map(|l| self.fold_range(l).filter(|&(_, e)| e >= line)) else { return false };
        self.folds.insert(start);
        for c in &mut self.cursors {
            if c.head.0 > start && c.head.0 <= end {
                *c = Cursor::at((start, 0));
            }
        }
        self.hidden_key = (u64::MAX, 0);
        self.reveal = true;
        true
    }

    pub fn fold_all(&mut self) {
        let starts: Vec<usize> = (0..self.lines.len()).filter(|&l| self.indent_of(l) == Some(0) && self.fold_range(l).is_some()).collect();
        self.folds.extend(starts);
        let p = self.primary().head;
        self.cursors = vec![Cursor::at((self.view_visible_line(p.0), 0))];
        self.hidden_key = (u64::MAX, 0);
    }

    pub fn unfold_all(&mut self) {
        self.folds.clear();
        self.hidden_key = (u64::MAX, 0);
    }

    fn unfold_at(&mut self, line: usize) {
        let before = self.folds.len();
        let folds: Vec<usize> = self.folds.iter().copied().collect();
        for f in folds {
            if let Some((s, e)) = self.fold_range(f) {
                if line > s && line <= e {
                    self.folds.remove(&f);
                }
            }
        }
        if self.folds.len() != before {
            self.hidden_key = (u64::MAX, 0);
        }
    }

    pub fn is_folded(&self, line: usize) -> bool {
        self.folds.contains(&line)
    }

    fn update_hidden(&mut self) {
        let key = (self.edits, self.folds.len());
        if self.hidden_key == key {
            return;
        }
        self.hidden_key = key;
        let mut ranges: Vec<(usize, usize)> = self.folds.iter().filter_map(|&f| self.fold_range(f)).map(|(s, e)| (s + 1, e)).collect();
        ranges.sort();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for r in ranges {
            match merged.last_mut() {
                Some(m) if r.0 <= m.1 + 1 => m.1 = m.1.max(r.1),
                _ => merged.push(r),
            }
        }
        let valid: BTreeSet<usize> = self.folds.iter().copied().filter(|&f| self.fold_range(f).is_some()).collect();
        self.folds = valid;
        self.hidden = merged;
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        let i = self.hidden.partition_point(|r| r.1 < line);
        self.hidden.get(i).is_some_and(|r| r.0 <= line)
    }

    // ---- keys ----

    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        self.update_hidden();
        let page = self.view.height.max(2) as isize - 1;
        if !matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown) {
            for c in &mut self.cursors {
                c.want = None;
            }
        }
        match key.code {
            KeyCode::Char('z') if ctrl && shift => self.restore(false),
            KeyCode::Char('Z') if ctrl => self.restore(false),
            KeyCode::Char('z') if ctrl => self.restore(true),
            KeyCode::Char('y') if ctrl => self.restore(false),
            KeyCode::Char('a') if ctrl => self.select_all(),
            KeyCode::Char('d') if ctrl => {
                self.add_next_occurrence();
            }
            KeyCode::Char('l') if ctrl => {
                for i in 0..self.cursors.len() {
                    let ((sl, _), (el, _)) = self.cursors[i].range();
                    let end = if el + 1 < self.lines.len() { (el + 1, 0) } else { (el, self.line_chars(el)) };
                    self.cursors[i] = Cursor { anchor: (sl, 0), head: end, want: None };
                }
                self.merge_cursors();
            }
            KeyCode::Char('k') if ctrl => {
                let text = self.delete_lines();
                return KeyResult::Copy(text);
            }
            KeyCode::Char('/' | '7' | '_') if ctrl => {
                if !self.toggle_comment() {
                    return KeyResult::Message("no comment syntax known for this file type".into());
                }
            }
            KeyCode::Char('c') if ctrl => {
                let text = self.selected_text().unwrap_or_else(|| self.lines[self.primary().head.0].clone() + "\n");
                return KeyResult::Copy(text);
            }
            KeyCode::Char('x') if ctrl => {
                if self.selected_text().is_some() {
                    let text = self.selected_text().unwrap_or_default();
                    self.push_undo(EditKind::Other);
                    for i in self.order_desc() {
                        let (s, e) = self.cursors[i].range();
                        let ne = self.replace(s, e, "");
                        self.cursors[i] = Cursor::at(ne);
                    }
                    self.after_edit();
                    return KeyResult::Copy(text);
                }
                return KeyResult::Copy(self.delete_lines());
            }
            KeyCode::Char(c) if !ctrl && !alt => self.type_char(c),
            KeyCode::Enter if !alt => self.newline(),
            KeyCode::Backspace if !alt => self.backspace(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Tab => self.indent_lines(false),
            KeyCode::BackTab => self.indent_lines(true),
            KeyCode::Up if ctrl && alt => self.add_cursor_vertical(false),
            KeyCode::Down if ctrl && alt => self.add_cursor_vertical(true),
            KeyCode::Up if alt && shift => self.duplicate_lines(true),
            KeyCode::Down if alt && shift => self.duplicate_lines(false),
            KeyCode::Up if alt => self.move_lines(true),
            KeyCode::Down if alt => self.move_lines(false),
            KeyCode::Left if ctrl => self.move_all(shift, |d, c| d.word_left(c.head)),
            KeyCode::Right if ctrl => self.move_all(shift, |d, c| d.word_right(c.head)),
            KeyCode::Left => {
                if !shift && self.cursors.iter().any(|c| !c.is_empty()) {
                    self.move_all(false, |_, c| c.range().0);
                } else {
                    self.move_all(shift, |d, c| d.left_of(c.head));
                }
            }
            KeyCode::Right => {
                if !shift && self.cursors.iter().any(|c| !c.is_empty()) {
                    self.move_all(false, |_, c| c.range().1);
                } else {
                    self.move_all(shift, |d, c| d.right_of(c.head));
                }
            }
            KeyCode::Up => self.vertical(-1, shift),
            KeyCode::Down => self.vertical(1, shift),
            KeyCode::PageUp => {
                self.view_scroll(-page);
                self.vertical(-page, shift);
            }
            KeyCode::PageDown => {
                self.view_scroll(page);
                self.vertical(page, shift);
            }
            KeyCode::Home if ctrl => self.move_all(shift, |_, _| (0, 0)),
            KeyCode::End if ctrl => self.move_all(shift, |d, _| {
                let last = d.lines.len() - 1;
                (last, d.line_chars(last))
            }),
            KeyCode::Home => self.move_all(shift, |d, c| {
                let first = d.first_non_ws(c.head.0);
                (c.head.0, if c.head.1 == first { 0 } else { first })
            }),
            KeyCode::End => self.move_all(shift, |d, c| (c.head.0, d.line_chars(c.head.0))),
            KeyCode::Esc => self.collapse_cursors(),
            _ => return KeyResult::Ignored,
        }
        KeyResult::Handled
    }
}

pub fn char_width(c: char, col: usize) -> usize {
    if c == '\t' {
        let tw = settings::tab_width();
        tw - col % tw
    } else {
        c.width().unwrap_or(1).max(if c.is_control() { 1 } else { 0 })
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests;
