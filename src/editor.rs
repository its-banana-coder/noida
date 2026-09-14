//! A small, fast text editor buffer with syntax highlighting.

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use syntect::highlighting::{
    FontStyle, HighlightState, Highlighter, RangedHighlightIterator, Theme,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use unicode_width::UnicodeWidthChar;

use crate::git::LineMark;
use crate::lsp::Diagnostic;
use crate::theme;

const TAB_WIDTH: usize = 4;
const MAX_HIGHLIGHT_LINES: usize = 50_000;
const MAX_HIGHLIGHT_LINE_LEN: usize = 4_000;
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

pub struct Syntax {
    pub set: SyntaxSet,
    pub theme: Theme,
}

impl Syntax {
    pub fn load() -> Self {
        Self {
            set: two_face::syntax::extra_newlines(),
            theme: two_face::theme::extra()
                .get(two_face::theme::EmbeddedThemeName::CatppuccinMocha)
                .clone(),
        }
    }
}

type Pos = (usize, usize); // (line, char index)

struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
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

pub struct Doc {
    pub path: PathBuf,
    lines: Vec<String>,
    cy: usize,
    cx: usize,
    want_col: Option<usize>,
    anchor: Option<Pos>,
    scroll_y: usize,
    scroll_x: usize,
    reveal_cursor: bool,
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
    /// Bumped on every content change (edits, undo, reload); used for LSP sync.
    pub edits: u64,
    edited_at: Option<Instant>,
    /// Git change markers per 0-based line.
    pub marks: HashMap<usize, LineMark>,
    /// Selections before each "expand selection", for shrinking back.
    sel_stack: Vec<(Option<Pos>, Pos)>,
    hl_states: Vec<(ParseState, HighlightState)>,
    hl_lines: Vec<Vec<(Style, Range<usize>)>>,
    height: usize,
    width: usize,
    gutter: u16,
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
        let mut doc = Self {
            path: path.to_path_buf(),
            lines: Vec::new(),
            cy: 0,
            cx: 0,
            want_col: None,
            anchor: None,
            scroll_y: 0,
            scroll_x: 0,
            reveal_cursor: true,
            dirty: false,
            mtime: meta.modified().ok(),
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
            height: 20,
            width: 80,
            gutter: 0,
        };
        doc.set_text(&text);
        doc.init_highlight(syntax);
        Ok(doc)
    }

    fn set_text(&mut self, text: &str) {
        self.crlf = text.contains("\r\n");
        self.trailing_newline = text.is_empty() || text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        self.lines = body
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let tabs = self.lines.iter().filter(|l| l.starts_with('\t')).count();
        let spaces = self.lines.iter().filter(|l| l.starts_with("  ")).count();
        self.indent_unit = if tabs > spaces { "\t".into() } else { "    ".into() };
        if spaces > tabs && self.lines.iter().filter(|l| l.starts_with("  ") && !l.starts_with("    ")).count() * 2 > spaces {
            self.indent_unit = "  ".into();
        }
        self.clamp_cursor();
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
        let highlighter = Highlighter::new(&syntax.theme);
        self.hl_states = vec![(
            ParseState::new(reference),
            HighlightState::new(&highlighter, ScopeStack::new()),
        )];
        self.hl_lines.clear();
    }

    fn invalidate(&mut self, from: usize) {
        self.hl_lines.truncate(from);
        self.hl_states.truncate(from + 1);
    }

    fn ensure_highlight(&mut self, upto: usize, syntax: &Syntax) {
        if self.lines.len() > MAX_HIGHLIGHT_LINES || self.hl_states.is_empty() {
            return;
        }
        let upto = upto.min(self.lines.len());
        if self.hl_lines.len() >= upto {
            return;
        }
        let highlighter = Highlighter::new(&syntax.theme);
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
        self.changed();
        self.invalidate(0);
        self.init_highlight(syntax);
        Some(format!("{name} reloaded (changed on disk)"))
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    // ---- navigation ----

    pub fn goto(&mut self, line: usize, col: Option<usize>, end_line: Option<usize>) {
        self.cy = line.saturating_sub(1).min(self.lines.len() - 1);
        self.cx = match col {
            Some(c) => c.saturating_sub(1),
            None => self.first_non_ws(self.cy),
        };
        self.anchor = None;
        self.want_col = None;
        self.clamp_cursor();
        let end = end_line
            .map(|e| e.saturating_sub(1).clamp(self.cy, self.lines.len() - 1))
            .unwrap_or(self.cy);
        self.flash = Some((self.cy, end, Instant::now()));
        self.scroll_y = self.cy.saturating_sub(self.height / 3);
        self.reveal_cursor = true;
    }

    pub fn cursor_line(&self) -> usize {
        self.cy + 1
    }

    pub fn cursor_col(&self) -> usize {
        self.cx + 1
    }

    pub fn flash_active(&self) -> bool {
        self.flash.is_some_and(|(_, _, t)| t.elapsed().as_millis() < 1500)
    }

    fn first_non_ws(&self, line: usize) -> usize {
        self.lines[line].chars().take_while(|c| c.is_whitespace()).count()
    }

    fn line_chars(&self, line: usize) -> usize {
        self.lines[line].chars().count()
    }

    fn clamp_cursor(&mut self) {
        self.cy = self.cy.min(self.lines.len().saturating_sub(1));
        self.cx = self.cx.min(self.line_chars(self.cy));
    }

    fn byte_idx(&self, line: usize, cx: usize) -> usize {
        let l = &self.lines[line];
        l.char_indices().nth(cx).map_or(l.len(), |(b, _)| b)
    }

    fn display_col(&self, line: usize, cx: usize) -> usize {
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

    fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        let c = (self.cy, self.cx);
        match a.cmp(&c) {
            std::cmp::Ordering::Less => Some((a, c)),
            std::cmp::Ordering::Greater => Some((c, a)),
            std::cmp::Ordering::Equal => None,
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let ((sl, sc), (el, ec)) = self.selection()?;
        if sl == el {
            let l = &self.lines[sl];
            return Some(l[self.byte_idx(sl, sc)..self.byte_idx(sl, ec)].to_string());
        }
        let mut out = self.lines[sl][self.byte_idx(sl, sc)..].to_string();
        for l in &self.lines[sl + 1..el] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[el][..self.byte_idx(el, ec)]);
        Some(out)
    }

    /// 1-based inclusive line range of the selection, or the cursor line.
    pub fn selected_lines(&self) -> (usize, usize) {
        match self.selection() {
            Some(((sl, _), (el, ec))) => {
                let el = if ec == 0 && el > sl { el - 1 } else { el };
                (sl + 1, el + 1)
            }
            None => (self.cy + 1, self.cy + 1),
        }
    }

    fn move_to(&mut self, pos: Pos, extend: bool) {
        if extend {
            self.anchor.get_or_insert((self.cy, self.cx));
        } else {
            self.anchor = None;
        }
        self.cy = pos.0;
        self.cx = pos.1;
        self.clamp_cursor();
        self.reveal_cursor = true;
    }

    fn vertical(&mut self, delta: isize, extend: bool) {
        let want = self.want_col.unwrap_or_else(|| self.display_col(self.cy, self.cx));
        let max = self.lines.len() as isize - 1;
        let line = (self.cy as isize + delta).clamp(0, max) as usize;
        let cx = self.cx_for_display_col(line, want);
        self.move_to((line, cx), extend);
        self.want_col = Some(want);
    }

    fn word_left(&self) -> Pos {
        if self.cx == 0 {
            return if self.cy == 0 { (0, 0) } else { (self.cy - 1, self.line_chars(self.cy - 1)) };
        }
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut i = self.cx;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let word = i > 0 && is_word(chars[i - 1]);
        while i > 0 && !chars[i - 1].is_whitespace() && is_word(chars[i - 1]) == word {
            i -= 1;
        }
        (self.cy, i)
    }

    fn word_right(&self) -> Pos {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        if self.cx >= chars.len() {
            return if self.cy + 1 < self.lines.len() { (self.cy + 1, 0) } else { (self.cy, self.cx) };
        }
        let mut i = self.cx;
        let word = is_word(chars[i]);
        while i < chars.len() && !chars[i].is_whitespace() && is_word(chars[i]) == word {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        (self.cy, i)
    }

    // ---- editing ----

    fn push_undo(&mut self, kind: EditKind) {
        let coalesce = kind != EditKind::Other
            && self
                .last_edit
                .is_some_and(|(k, t)| k == kind && t.elapsed().as_millis() < 1000);
        self.last_edit = Some((kind, Instant::now()));
        if !coalesce {
            self.undo.push(Snapshot { lines: self.lines.clone(), cursor: (self.cy, self.cx) });
            if self.undo.len() > 300 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    fn restore(&mut self, from_undo: bool) {
        let (src, dst) = if from_undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(snap) = src.pop() else { return };
        dst.push(Snapshot { lines: std::mem::replace(&mut self.lines, snap.lines), cursor: (self.cy, self.cx) });
        (self.cy, self.cx) = snap.cursor;
        self.anchor = None;
        self.last_edit = None;
        self.clamp_cursor();
        self.invalidate(0);
        self.dirty = true;
        self.reveal_cursor = true;
        self.changed();
    }

    fn delete_range(&mut self, (sl, sc): Pos, (el, ec): Pos) {
        let sb = self.byte_idx(sl, sc);
        let eb = self.byte_idx(el, ec);
        let tail = self.lines[el][eb..].to_string();
        self.lines[sl].truncate(sb);
        self.lines[sl].push_str(&tail);
        self.lines.drain(sl + 1..=el);
        self.cy = sl;
        self.cx = sc;
        self.anchor = None;
        self.invalidate(sl);
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((s, e)) => {
                self.delete_range(s, e);
                true
            }
            None => false,
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        self.push_undo(if text.len() == 1 { EditKind::Insert } else { EditKind::Other });
        self.delete_selection();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let b = self.byte_idx(self.cy, self.cx);
        let tail = self.lines[self.cy].split_off(b);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[self.cy].push_str(first);
        self.cx += first.chars().count();
        let start = self.cy;
        for part in parts {
            self.cy += 1;
            self.lines.insert(self.cy, part.to_string());
            self.cx = part.chars().count();
        }
        self.lines[self.cy].push_str(&tail);
        self.invalidate(start);
        self.after_edit();
    }

    fn newline(&mut self) {
        self.push_undo(EditKind::Other);
        self.delete_selection();
        let b = self.byte_idx(self.cy, self.cx);
        let indent: String = self.lines[self.cy]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .take(self.cx)
            .collect();
        let tail = self.lines[self.cy].split_off(b);
        let start = self.cy;
        self.cy += 1;
        self.cx = indent.chars().count();
        self.lines.insert(self.cy, indent + &tail);
        self.invalidate(start);
        self.after_edit();
    }

    fn backspace(&mut self) {
        self.push_undo(EditKind::Delete);
        if self.delete_selection() {
        } else if self.cx > 0 {
            self.delete_range((self.cy, self.cx - 1), (self.cy, self.cx));
        } else if self.cy > 0 {
            let prev = self.line_chars(self.cy - 1);
            self.delete_range((self.cy - 1, prev), (self.cy, 0));
        }
        self.after_edit();
    }

    fn delete_forward(&mut self) {
        self.push_undo(EditKind::Delete);
        if self.delete_selection() {
        } else if self.cx < self.line_chars(self.cy) {
            self.delete_range((self.cy, self.cx), (self.cy, self.cx + 1));
        } else if self.cy + 1 < self.lines.len() {
            self.delete_range((self.cy, self.cx), (self.cy + 1, 0));
        }
        self.after_edit();
    }

    fn indent(&mut self, dedent: bool) {
        let (sl, el) = match self.selection() {
            Some(((sl, _), (el, _))) if sl != el || dedent => (sl, el),
            None if dedent => (self.cy, self.cy),
            _ => {
                let unit = self.indent_unit.clone();
                return self.insert_text(&unit);
            }
        };
        self.push_undo(EditKind::Other);
        let unit = self.indent_unit.clone();
        for l in sl..=el {
            if dedent {
                let n = if self.lines[l].starts_with(&unit) {
                    unit.len()
                } else {
                    self.lines[l].chars().take_while(|c| *c == ' ').count().min(unit.len())
                };
                self.lines[l].drain(..n);
            } else if !self.lines[l].is_empty() {
                self.lines[l].insert_str(0, &unit);
            }
        }
        self.invalidate(sl);
        self.clamp_cursor();
        if let Some(a) = &mut self.anchor {
            a.1 = a.1.min(self.lines[a.0].chars().count());
        }
        self.after_edit();
    }

    fn changed(&mut self) {
        self.edits += 1;
        self.edited_at = Some(Instant::now());
        self.sel_stack.clear();
    }

    fn after_edit(&mut self) {
        self.changed();
        self.dirty = true;
        self.want_col = None;
        self.reveal_cursor = true;
        self.clamp_cursor();
    }

    pub fn find(&mut self, query: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let smart_case = query.chars().any(char::is_uppercase);
        let norm = |s: &str| if smart_case { s.to_string() } else { s.to_lowercase() };
        let q = norm(query);
        let n = self.lines.len();
        let start_byte = self.byte_idx(self.cy, self.cx);
        for k in 0..=n {
            let li = (self.cy + k) % n;
            let hay = norm(&self.lines[li]);
            let from = if k == 0 { (start_byte + 1).min(hay.len()) } else { 0 };
            let found = if k == n { hay[..start_byte.min(hay.len())].find(&q) } else { hay.get(from..).and_then(|h| h.find(&q)).map(|i| i + from) };
            if let Some(b) = found {
                // Lowercasing can shift byte offsets for some scripts; clamp to char boundaries.
                let line = &self.lines[li];
                let sc = line.get(..b).map_or(0, |s| s.chars().count());
                self.anchor = Some((li, sc));
                self.cy = li;
                self.cx = sc + query.chars().count();
                self.clamp_cursor();
                self.scroll_y = li.saturating_sub(self.height / 3);
                self.reveal_cursor = true;
                return true;
            }
        }
        false
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.height.max(2) as isize - 1;
        if !matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown) {
            self.want_col = None;
        }
        match key.code {
            KeyCode::Char('s') if ctrl => {
                return match self.save() {
                    Ok(()) => KeyResult::Message(format!("saved {}", self.file_name())),
                    Err(e) => KeyResult::Message(format!("save failed: {e}")),
                };
            }
            KeyCode::Char('z') if ctrl => self.restore(true),
            KeyCode::Char('y') if ctrl => self.restore(false),
            KeyCode::Char('a') if ctrl => {
                let last = self.lines.len() - 1;
                self.anchor = Some((0, 0));
                self.cy = last;
                self.cx = self.line_chars(last);
            }
            KeyCode::Char('c') if ctrl => {
                let text = self.selected_text().unwrap_or_else(|| self.lines[self.cy].clone() + "\n");
                return KeyResult::Copy(text);
            }
            KeyCode::Char('x') if ctrl => {
                let text = match self.selected_text() {
                    Some(t) => {
                        self.push_undo(EditKind::Other);
                        self.delete_selection();
                        t
                    }
                    None => {
                        self.push_undo(EditKind::Other);
                        let t = self.lines[self.cy].clone() + "\n";
                        if self.lines.len() > 1 {
                            self.lines.remove(self.cy);
                        } else {
                            self.lines[0].clear();
                        }
                        self.invalidate(self.cy.saturating_sub(1));
                        t
                    }
                };
                self.after_edit();
                return KeyResult::Copy(text);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert_text(c.encode_utf8(&mut [0; 4]));
            }
            KeyCode::Enter => self.newline(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Tab => self.indent(false),
            KeyCode::BackTab => self.indent(true),
            KeyCode::Left if ctrl => self.move_to(self.word_left(), shift),
            KeyCode::Right if ctrl => self.move_to(self.word_right(), shift),
            KeyCode::Left => {
                if let (Some((s, _)), false) = (self.selection(), shift) {
                    self.move_to(s, false);
                } else if self.cx > 0 {
                    self.move_to((self.cy, self.cx - 1), shift);
                } else if self.cy > 0 {
                    self.move_to((self.cy - 1, self.line_chars(self.cy - 1)), shift);
                }
            }
            KeyCode::Right => {
                if let (Some((_, e)), false) = (self.selection(), shift) {
                    self.move_to(e, false);
                } else if self.cx < self.line_chars(self.cy) {
                    self.move_to((self.cy, self.cx + 1), shift);
                } else if self.cy + 1 < self.lines.len() {
                    self.move_to((self.cy + 1, 0), shift);
                }
            }
            KeyCode::Up => self.vertical(-1, shift),
            KeyCode::Down => self.vertical(1, shift),
            KeyCode::PageUp => {
                self.scroll_y = self.scroll_y.saturating_sub(page as usize);
                self.vertical(-page, shift);
            }
            KeyCode::PageDown => {
                self.scroll_y += page as usize;
                self.vertical(page, shift);
            }
            KeyCode::Home if ctrl => self.move_to((0, 0), shift),
            KeyCode::End if ctrl => {
                let last = self.lines.len() - 1;
                self.move_to((last, self.line_chars(last)), shift);
            }
            KeyCode::Home => {
                let first = self.first_non_ws(self.cy);
                let target = if self.cx == first { 0 } else { first };
                self.move_to((self.cy, target), shift);
            }
            KeyCode::End => self.move_to((self.cy, self.line_chars(self.cy)), shift),
            KeyCode::Esc => self.anchor = None,
            _ => return KeyResult::Ignored,
        }
        KeyResult::Handled
    }

    // ---- structure-aware helpers ----

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn edited_at(&self) -> Option<Instant> {
        self.edited_at
    }

    fn byte_of(&self, (line, cx): Pos) -> usize {
        self.lines[..line].iter().map(|l| l.len() + 1).sum::<usize>() + self.byte_idx(line, cx)
    }

    fn pos_of_byte(&self, mut b: usize) -> Pos {
        for (i, l) in self.lines.iter().enumerate() {
            if b <= l.len() {
                let cx = l.get(..b).map_or(l.chars().count(), |s| s.chars().count());
                return (i, cx);
            }
            b -= l.len() + 1;
        }
        let last = self.lines.len() - 1;
        (last, self.line_chars(last))
    }

    /// Byte range of the selection (or the empty range at the cursor).
    pub fn selection_bytes(&self) -> (usize, usize) {
        match self.selection() {
            Some((s, e)) => (self.byte_of(s), self.byte_of(e)),
            None => {
                let b = self.byte_of((self.cy, self.cx));
                (b, b)
            }
        }
    }

    /// Select a byte range, remembering the previous selection for `shrink_selection`.
    pub fn expand_to(&mut self, (start, end): (usize, usize)) {
        self.sel_stack.push((self.anchor, (self.cy, self.cx)));
        self.anchor = Some(self.pos_of_byte(start));
        (self.cy, self.cx) = self.pos_of_byte(end);
        self.reveal_cursor = true;
    }

    pub fn shrink_selection(&mut self) -> bool {
        match self.sel_stack.pop() {
            Some((anchor, cursor)) => {
                self.anchor = anchor;
                (self.cy, self.cx) = cursor;
                self.reveal_cursor = true;
                true
            }
            None => false,
        }
    }

    /// Identifier under (or just before) the cursor.
    pub fn word_at_cursor(&self) -> Option<String> {
        let chars: Vec<char> = self.lines[self.cy].chars().collect();
        let mut s = self.cx.min(chars.len());
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
        Some(chars[s..e].iter().collect())
    }

    /// Cursor column in UTF-16 code units, as LSP expects.
    pub fn utf16_col(&self) -> usize {
        self.lines[self.cy].chars().take(self.cx).map(char::len_utf16).sum()
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

    pub fn cursor_line0(&self) -> usize {
        self.cy
    }

    // ---- mouse ----

    fn pos_at(&self, area: Rect, x: u16, y: u16) -> Pos {
        let line = (self.scroll_y + y.saturating_sub(area.y) as usize).min(self.lines.len() - 1);
        let col = (x.saturating_sub(area.x + self.gutter)) as usize + self.scroll_x;
        (line, self.cx_for_display_col(line, col))
    }

    pub fn click(&mut self, area: Rect, x: u16, y: u16, extend: bool) {
        let pos = self.pos_at(area, x, y);
        self.want_col = None;
        self.move_to(pos, extend);
    }

    pub fn drag(&mut self, area: Rect, x: u16, y: u16) {
        let pos = self.pos_at(area, x, y);
        self.move_to(pos, true);
    }

    pub fn scroll(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(1) as isize;
        self.scroll_y = (self.scroll_y as isize + delta).clamp(0, max) as usize;
        self.reveal_cursor = false;
    }

    // ---- rendering ----

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, focused: bool, syntax: &Syntax, diags: &[Diagnostic]) -> Option<(u16, u16)> {
        let digits = self.lines.len().to_string().len().max(3);
        self.gutter = digits as u16 + 2;
        self.height = area.height as usize;
        self.width = (area.width.saturating_sub(self.gutter)) as usize;
        if self.height == 0 || self.width == 0 {
            return None;
        }

        let cursor_col = self.display_col(self.cy, self.cx);
        if self.reveal_cursor {
            if self.cy < self.scroll_y {
                self.scroll_y = self.cy;
            } else if self.cy >= self.scroll_y + self.height {
                self.scroll_y = self.cy + 1 - self.height;
            }
            if cursor_col < self.scroll_x {
                self.scroll_x = cursor_col;
            } else if cursor_col >= self.scroll_x + self.width {
                self.scroll_x = cursor_col + 1 - self.width;
            }
        }
        self.scroll_y = self.scroll_y.min(self.lines.len().saturating_sub(1));

        self.ensure_highlight(self.scroll_y + self.height, syntax);
        let flash = self.flash.filter(|(_, _, t)| t.elapsed().as_millis() < 1500);
        let selection = self.selection();
        let base = Style::default().fg(theme::FG);

        for row in 0..self.height {
            let li = self.scroll_y + row;
            let y = area.y + row as u16;
            if li >= self.lines.len() {
                buf.set_string(area.x, y, format!("{:>digits$}  ", "~"), Style::default().fg(theme::BORDER));
                continue;
            }
            let mut line_bg = None;
            if li == self.cy && focused {
                line_bg = Some(theme::CURRENT_LINE);
            }
            if let Some((s, e, _)) = flash {
                if li >= s && li <= e {
                    line_bg = Some(theme::FLASH);
                }
            }
            if let Some(bg) = line_bg {
                buf.set_style(Rect::new(area.x, y, area.width, 1), Style::default().bg(bg));
            }
            let num_style = if li == self.cy { Style::default().fg(theme::ACCENT) } else { Style::default().fg(theme::DIM) };
            buf.set_string(area.x, y, format!("{:>digits$}  ", li + 1), num_style);
            if let Some(d) = diags.iter().filter(|d| d.line == li).min_by_key(|d| d.severity) {
                let color = if d.severity <= 1 { theme::ERROR } else if d.severity == 2 { theme::ACCENT } else { theme::DIM };
                buf.set_string(area.x + digits as u16, y, "●", Style::default().fg(color));
            }
            if let Some(mark) = self.marks.get(&li) {
                let (sym, color) = match mark {
                    LineMark::Added => ("▎", theme::ADDED),
                    LineMark::Modified => ("▎", theme::DIR),
                    LineMark::DeletedBelow => ("▁", theme::ERROR),
                };
                buf.set_string(area.x + digits as u16 + 1, y, sym, Style::default().fg(color));
            }

            let spans = self.hl_lines.get(li);
            let mut span_i = 0;
            let mut col = 0;
            let text_x = area.x + self.gutter;
            for (ci, (bi, ch)) in self.lines[li].char_indices().enumerate() {
                let w = char_width(ch, col);
                let start = col;
                col += w;
                if col <= self.scroll_x {
                    continue;
                }
                if start >= self.scroll_x + self.width {
                    break;
                }
                let mut style = base;
                if let Some(spans) = spans {
                    while span_i < spans.len() && spans[span_i].1.end <= bi {
                        span_i += 1;
                    }
                    if let Some((s, _)) = spans.get(span_i) {
                        style = *s;
                    }
                }
                if let Some((s, e)) = selection {
                    if (li, ci) >= s && (li, ci) < e {
                        style = style.bg(theme::SELECT);
                    }
                }
                let x0 = start.max(self.scroll_x) - self.scroll_x;
                if ch == '\t' {
                    for k in x0..(col - self.scroll_x).min(self.width) {
                        buf[(text_x + k as u16, y)].set_symbol(" ").set_style(style);
                    }
                } else if x0 + w <= self.width && start >= self.scroll_x {
                    let sym = if ch.is_control() { '?' } else { ch };
                    buf[(text_x + x0 as u16, y)].set_char(sym).set_style(style);
                }
            }
            // Show selected line breaks so multi-line selections read clearly.
            if let Some((s, e)) = selection {
                let eol = (li, self.line_chars(li));
                if eol >= s && eol < e && col >= self.scroll_x && col - self.scroll_x < self.width {
                    buf[(text_x + (col - self.scroll_x) as u16, y)].set_style(Style::default().bg(theme::SELECT));
                }
            }
        }

        if self.cy >= self.scroll_y && self.cy < self.scroll_y + self.height && cursor_col >= self.scroll_x && cursor_col < self.scroll_x + self.width {
            Some((area.x + self.gutter + (cursor_col - self.scroll_x) as u16, area.y + (self.cy - self.scroll_y) as u16))
        } else {
            None
        }
    }
}

fn char_width(c: char, col: usize) -> usize {
    if c == '\t' {
        TAB_WIDTH - col % TAB_WIDTH
    } else {
        c.width().unwrap_or(1).max(if c.is_control() { 1 } else { 0 })
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Doc {
        let dir = std::env::temp_dir().join(format!("noida-test-{}-{}", std::process::id(), text.len()));
        std::fs::write(&dir, text).unwrap();
        let syntax = Syntax { set: SyntaxSet::load_defaults_newlines(), theme: Theme::default() };
        let d = Doc::open(&dir, &syntax).unwrap();
        std::fs::remove_file(&dir).ok();
        d
    }

    #[test]
    fn edit_and_undo() {
        let mut d = doc("fn main() {\n    let x = 1;\n}\n");
        d.goto(2, None, None);
        assert_eq!((d.cy, d.cx), (1, 4));
        d.handle_key(KeyEvent::from(KeyCode::End));
        d.handle_key(KeyEvent::from(KeyCode::Enter));
        d.insert_text("y");
        assert_eq!(d.lines[2], "    y");
        d.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        d.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(d.lines.len(), 3);
        d.goto(2, Some(5), None);
        d.insert_text("a\nb");
        assert_eq!(d.lines[1], "    a");
        assert_eq!(d.lines[2], "blet x = 1;");
    }

    #[test]
    fn selection_and_find() {
        let mut d = doc("alpha beta\ngamma beta\n");
        assert!(d.find("beta"));
        assert_eq!(d.selected_text().as_deref(), Some("beta"));
        assert!(d.find("beta"));
        assert_eq!((d.cy, d.selected_lines()), (1, (2, 2)));
        d.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(d.selected_text().unwrap(), "alpha beta\ngamma beta");
    }
}
