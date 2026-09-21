//! Modal (Vim) editing.
//!
//! NOIDA's natural audience is the people who already left GUI editors for the
//! terminal, and for many of them modal editing is not a preference but the
//! reason they left. This is a working subset rather than an emulation: the
//! motions, operators and registers people use without thinking, driving the
//! same editing primitives as every other key path, so undo, multi-cursor and
//! folding keep working underneath.
//!
//! What it deliberately does not try to be: macros, marks, registers by name,
//! text objects beyond `iw`/`aw`, or `:s///`. Those are worth adding only if
//! people actually miss them.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Doc, Pos};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    /// What the status bar shows, as Vim spells it.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "-- INSERT --",
            Mode::Visual => "-- VISUAL --",
            Mode::VisualLine => "-- VISUAL LINE --",
        }
    }
}

/// What the editor should do with a key the Vim layer saw.
#[derive(Debug, PartialEq)]
pub enum VimResult {
    /// Fully handled here.
    Handled,
    /// Not a Vim key: let the normal editor bindings have it.
    PassThrough,
    /// An ex command (`:w`, `:q`, `:42`) for the application to carry out.
    Ex(String),
    /// Start a search: (query, forwards).
    Search(String, bool),
    /// Text yanked or deleted, for the system clipboard.
    Yanked(String),
}

/// One pending operator, e.g. the `d` of `d2w`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Delete,
    Change,
    Yank,
}

#[derive(Default)]
pub struct Vim {
    pub mode: Mode,
    /// Digits typed so far: the `2` of `2dw`.
    count: String,
    /// Count typed between an operator and its motion: the `3` of `d3w`.
    op_count: String,
    op: Option<Op>,
    /// Waiting for the character argument of `f`, `t`, `r` or `g`.
    awaiting: Option<char>,
    /// The unnamed register, and whether it holds whole lines.
    register: (String, bool),
    /// Last `f`/`t` search, for `;` and `,`.
    last_find: Option<(char, char)>,
    /// Buffer for `:` and `/` input.
    line: Option<(char, String)>,
}

impl Vim {
    pub fn new() -> Self {
        Self::default()
    }

    /// The text being typed on the `:` or `/` line, for the status bar.
    pub fn pending_line(&self) -> Option<String> {
        self.line.as_ref().map(|(p, t)| format!("{p}{t}"))
    }

    /// A count of 0 means "not given"; Vim treats a missing count as 1.
    fn take_count(&mut self) -> usize {
        let n: usize = self.count.parse().unwrap_or(0);
        self.count.clear();
        n.max(1)
    }

    fn total_count(&mut self) -> usize {
        let a: usize = self.count.parse().unwrap_or(1).max(1);
        let b: usize = self.op_count.parse().unwrap_or(1).max(1);
        self.count.clear();
        self.op_count.clear();
        a * b
    }

    fn reset(&mut self) {
        self.count.clear();
        self.op_count.clear();
        self.op = None;
        self.awaiting = None;
    }

    pub fn handle(&mut self, doc: &mut Doc, key: KeyEvent) -> VimResult {
        // A line being typed (`:` or `/`) swallows everything until Enter.
        if self.line.is_some() {
            return self.handle_line(key);
        }
        match self.mode {
            Mode::Insert => self.insert_key(doc, key),
            Mode::Normal => self.normal_key(doc, key),
            Mode::Visual | Mode::VisualLine => self.visual_key(doc, key),
        }
    }

    fn handle_line(&mut self, key: KeyEvent) -> VimResult {
        let Some((_, text)) = &mut self.line else { return VimResult::PassThrough };
        match key.code {
            KeyCode::Esc => {
                self.line = None;
                VimResult::Handled
            }
            KeyCode::Backspace => {
                if text.pop().is_none() {
                    self.line = None;
                }
                VimResult::Handled
            }
            KeyCode::Char(c) => {
                text.push(c);
                VimResult::Handled
            }
            KeyCode::Enter => {
                let (prefix, text) = self.line.take().expect("checked above");
                match prefix {
                    ':' => VimResult::Ex(text),
                    '/' => VimResult::Search(text, true),
                    _ => VimResult::Search(text, false),
                }
            }
            _ => VimResult::Handled,
        }
    }

    fn insert_key(&mut self, doc: &mut Doc, key: KeyEvent) -> VimResult {
        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            // Vim leaves the cursor on the character before where you stopped.
            let (line, cx) = doc.head();
            if cx > 0 {
                doc.set_cursor((line, cx - 1));
            }
            return VimResult::Handled;
        }
        VimResult::PassThrough
    }

    fn normal_key(&mut self, doc: &mut Doc, key: KeyEvent) -> VimResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let KeyCode::Char(c) = key.code else {
            return match key.code {
                KeyCode::Esc => {
                    self.reset();
                    VimResult::Handled
                }
                // Arrows and the like still work: nobody is helped by breaking them.
                _ => VimResult::PassThrough,
            };
        };

        // A character argument we were waiting for (f{char}, r{char}, g{char}).
        if let Some(pending) = self.awaiting.take() {
            return self.char_arg(doc, pending, c);
        }

        if ctrl {
            return self.ctrl_key(doc, c);
        }

        // Counts: a leading 0 is the motion, not a digit.
        if c.is_ascii_digit() && !(c == '0' && self.active_count().is_empty()) {
            if self.op.is_some() {
                self.op_count.push(c);
            } else {
                self.count.push(c);
            }
            return VimResult::Handled;
        }

        // An operator repeated is linewise: dd, cc, yy.
        if let Some(op) = self.op {
            if (op == Op::Delete && c == 'd') || (op == Op::Change && c == 'c') || (op == Op::Yank && c == 'y') {
                let n = self.total_count();
                self.op = None;
                return self.line_op(doc, op, n);
            }
        }

        match c {
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' if self.op.is_none() => {
                self.enter_insert(doc, c);
                VimResult::Handled
            }
            'd' | 'c' | 'y' => {
                let op = match c {
                    'd' => Op::Delete,
                    'c' => Op::Change,
                    _ => Op::Yank,
                };
                self.op = Some(op);
                VimResult::Handled
            }
            'v' => {
                self.mode = Mode::Visual;
                doc.start_selection();
                VimResult::Handled
            }
            'V' => {
                self.mode = Mode::VisualLine;
                doc.start_selection();
                VimResult::Handled
            }
            'x' | 'X' | 's' | 'D' | 'C' | 'Y' | 'p' | 'P' | 'J' | 'u' | 'r' | 'S' => self.simple_edit(doc, c),
            ':' | '/' | '?' => {
                self.line = Some((c, String::new()));
                VimResult::Handled
            }
            'n' | 'N' => {
                doc.find_step(c == 'n');
                VimResult::Handled
            }
            _ => self.motion(doc, c),
        }
    }

    fn active_count(&self) -> &str {
        if self.op.is_some() { &self.op_count } else { &self.count }
    }

    fn ctrl_key(&mut self, doc: &mut Doc, c: char) -> VimResult {
        let page = doc.page_height().max(1);
        match c {
            'd' => self.move_lines(doc, page as isize / 2),
            'u' => self.move_lines(doc, -(page as isize / 2)),
            'f' => self.move_lines(doc, page as isize),
            'b' => self.move_lines(doc, -(page as isize)),
            // Redo; undo is `u`.
            'r' => {
                doc.restore(false);
                return VimResult::Handled;
            }
            _ => return VimResult::PassThrough,
        }
        VimResult::Handled
    }

    fn move_lines(&mut self, doc: &mut Doc, delta: isize) {
        let (line, cx) = doc.head();
        let target = (line as isize + delta).clamp(0, doc.last_line() as isize) as usize;
        doc.set_cursor((target, cx.min(doc.line_len(target))));
    }

    fn enter_insert(&mut self, doc: &mut Doc, c: char) {
        let (line, cx) = doc.head();
        match c {
            'i' => {}
            'a' => doc.set_cursor((line, (cx + 1).min(doc.line_len(line)))),
            'I' => doc.set_cursor((line, doc.first_text_col(line))),
            'A' => doc.set_cursor((line, doc.line_len(line))),
            'o' => {
                doc.set_cursor((line, doc.line_len(line)));
                doc.insert_text("\n");
            }
            'O' => {
                doc.set_cursor((line, 0));
                doc.insert_text("\n");
                doc.set_cursor((line, 0));
            }
            _ => {}
        }
        self.mode = Mode::Insert;
    }

    /// Edits that need no motion.
    fn simple_edit(&mut self, doc: &mut Doc, c: char) -> VimResult {
        let n = self.take_count();
        let (line, cx) = doc.head();
        match c {
            'x' | 's' => {
                let end = (cx + n).min(doc.line_len(line));
                let text = doc.text_between((line, cx), (line, end));
                doc.edit((line, cx), (line, end), "");
                self.register = (text.clone(), false);
                if c == 's' {
                    self.mode = Mode::Insert;
                }
                return VimResult::Yanked(text);
            }
            'X' => {
                let start = cx.saturating_sub(n);
                let text = doc.text_between((line, start), (line, cx));
                doc.edit((line, start), (line, cx), "");
                self.register = (text.clone(), false);
                return VimResult::Yanked(text);
            }
            'D' | 'C' => {
                let end = doc.line_len(line);
                let text = doc.text_between((line, cx), (line, end));
                doc.edit((line, cx), (line, end), "");
                self.register = (text.clone(), false);
                if c == 'C' {
                    self.mode = Mode::Insert;
                }
                return VimResult::Yanked(text);
            }
            'Y' => return self.line_op(doc, Op::Yank, n),
            'S' => return self.line_op(doc, Op::Change, n),
            'p' | 'P' => {
                let (text, linewise) = self.register.clone();
                if text.is_empty() {
                    return VimResult::Handled;
                }
                if linewise {
                    let at = if c == 'p' { line + 1 } else { line };
                    doc.insert_lines(at, &text.repeat(n));
                    doc.set_cursor((at.min(doc.last_line()), 0));
                } else {
                    let at = if c == 'p' { (cx + 1).min(doc.line_len(line)) } else { cx };
                    doc.set_cursor((line, at));
                    doc.insert_text(&text.repeat(n));
                }
            }
            'J' => {
                for _ in 0..n.max(1) {
                    doc.join_line(doc.head().0);
                }
            }
            'u' => doc.restore(true),
            'r' => {
                self.awaiting = Some('r');
                self.count = n.to_string();
            }
            _ => return VimResult::PassThrough,
        }
        VimResult::Handled
    }

    /// The character argument of `f`, `t`, `r` or `g`.
    fn char_arg(&mut self, doc: &mut Doc, pending: char, c: char) -> VimResult {
        match pending {
            'r' => {
                let n = self.take_count();
                let (line, cx) = doc.head();
                let end = (cx + n).min(doc.line_len(line));
                if end > cx {
                    doc.edit((line, cx), (line, end), &c.to_string().repeat(end - cx));
                    doc.set_cursor((line, end - 1));
                }
                VimResult::Handled
            }
            'g' => match c {
                'g' => {
                    let n: usize = self.count.parse().unwrap_or(1);
                    self.count.clear();
                    let from = doc.head();
                    let target = n.saturating_sub(1).min(doc.last_line());
                    // `dgg` is linewise, like `dj`.
                    if let Some(op) = self.op.take() {
                        let (a, b) = if target >= from.0 { (from.0, target) } else { (target, from.0) };
                        doc.set_cursor((a, 0));
                        return self.line_op(doc, op, b - a + 1);
                    }
                    doc.set_cursor((target, doc.first_text_col(target)));
                    self.finish_pending_op(doc, from)
                }
                _ => {
                    self.reset();
                    VimResult::Handled
                }
            },
            'f' | 'F' | 't' | 'T' => {
                self.last_find = Some((pending, c));
                self.find_char(doc, pending, c)
            }
            _ => VimResult::Handled,
        }
    }

    fn find_char(&mut self, doc: &mut Doc, kind: char, target: char) -> VimResult {
        let n = self.total_count();
        let from = doc.head();
        let (line, cx) = from;
        let text: Vec<char> = doc.line_chars_vec(line);
        let forward = kind == 'f' || kind == 't';
        let mut at = cx;
        for _ in 0..n {
            let found = if forward {
                (at + 1..text.len()).find(|&i| text[i] == target)
            } else {
                (0..at).rev().find(|&i| text[i] == target)
            };
            match found {
                Some(i) => at = i,
                None => {
                    self.op = None;
                    return VimResult::Handled;
                }
            }
        }
        // `t` stops one short of the character.
        let at = match (kind, forward) {
            ('t', true) => at.saturating_sub(1),
            ('T', false) => (at + 1).min(text.len()),
            _ => at,
        };
        // An operator with f/t includes the character it landed on.
        if self.op.is_some() && forward {
            doc.set_cursor((line, (at + 1).min(doc.line_len(line))));
            return self.finish_pending_op(doc, from);
        }
        doc.set_cursor((line, at));
        self.finish_pending_op(doc, from)
    }

    /// Move, then apply any pending operator over what was crossed.
    fn motion(&mut self, doc: &mut Doc, c: char) -> VimResult {
        // `G` means "last line" without a count and "go to line N" with one,
        // so note it before the counters are consumed.
        let given = !self.count.is_empty() || !self.op_count.is_empty();
        let n = self.total_count();
        let from = doc.head();
        let (line, cx) = from;
        let last = doc.last_line();
        match c {
            'h' => doc.set_cursor((line, cx.saturating_sub(n))),
            'l' => doc.set_cursor((line, (cx + n).min(doc.line_len(line)))),
            'j' | 'k' => {
                let delta = if c == 'j' { n as isize } else { -(n as isize) };
                let target = (line as isize + delta).clamp(0, last as isize) as usize;
                // With an operator these are linewise: `dj` takes both lines.
                if let Some(op) = self.op.take() {
                    let (a, b) = if target >= line { (line, target) } else { (target, line) };
                    doc.set_cursor((a, 0));
                    return self.line_op(doc, op, b - a + 1);
                }
                doc.set_cursor((target, cx.min(doc.line_len(target))));
            }
            '0' => doc.set_cursor((line, 0)),
            '^' => doc.set_cursor((line, doc.first_text_col(line))),
            '$' => {
                let target = (line + n - 1).min(last);
                doc.set_cursor((target, doc.line_len(target)));
            }
            'G' => {
                let target = if given { n.saturating_sub(1).min(last) } else { last };
                doc.set_cursor((target, doc.first_text_col(target)));
            }
            'g' => {
                self.awaiting = Some('g');
                self.count = if n > 1 { n.to_string() } else { String::new() };
                return VimResult::Handled;
            }
            'w' | 'W' | 'b' | 'B' | 'e' | 'E' => {
                let mut at = from;
                for _ in 0..n {
                    at = doc.word_move(at, c);
                }
                doc.set_cursor(at);
            }
            '{' | '}' => {
                let mut at = line;
                for _ in 0..n {
                    at = doc.paragraph_move(at, c == '}');
                }
                doc.set_cursor((at, 0));
            }
            'f' | 'F' | 't' | 'T' => {
                self.awaiting = Some(c);
                self.op_count = n.to_string();
                return VimResult::Handled;
            }
            ';' | ',' => {
                if let Some((kind, target)) = self.last_find {
                    let kind = if c == ';' {
                        kind
                    } else {
                        match kind {
                            'f' => 'F',
                            'F' => 'f',
                            't' => 'T',
                            _ => 't',
                        }
                    };
                    return self.find_char(doc, kind, target);
                }
                return VimResult::Handled;
            }
            _ => {
                // Not ours: drop any half-typed operator rather than acting on
                // a key the user did not mean as a motion.
                self.reset();
                return VimResult::PassThrough;
            }
        }
        self.finish_pending_op(doc, from)
    }

    /// Apply the pending operator between where the motion started and landed.
    fn finish_pending_op(&mut self, doc: &mut Doc, from: Pos) -> VimResult {
        let Some(op) = self.op.take() else {
            return VimResult::Handled;
        };
        let (start, end) = (from, doc.head());
        let (s, e) = if start <= end { (start, end) } else { (end, start) };
        let text = doc.text_between(s, e);
        self.register = (text.clone(), false);
        match op {
            Op::Yank => {
                doc.set_cursor(s);
            }
            Op::Delete | Op::Change => {
                doc.edit(s, e, "");
                if op == Op::Change {
                    self.mode = Mode::Insert;
                }
            }
        }
        VimResult::Yanked(text)
    }

    /// `dd`, `yy`, `cc` and their capitalised friends.
    fn line_op(&mut self, doc: &mut Doc, op: Op, n: usize) -> VimResult {
        let line = doc.head().0;
        let last = doc.last_line();
        let end = (line + n - 1).min(last);
        let text = doc.lines_text(line, end);
        self.register = (text.clone(), true);
        match op {
            Op::Yank => {}
            Op::Delete => {
                doc.delete_lines_range(line, end);
            }
            Op::Change => {
                // `cc` keeps the line and its indent, and opens it for typing.
                let indent = doc.indent_text(line);
                doc.delete_lines_range(line, end);
                doc.insert_line_at(line.min(doc.last_line() + 1), &indent);
                doc.set_cursor((line.min(doc.last_line()), indent.chars().count()));
                self.mode = Mode::Insert;
            }
        }
        VimResult::Yanked(text)
    }

    fn visual_key(&mut self, doc: &mut Doc, key: KeyEvent) -> VimResult {
        let KeyCode::Char(c) = key.code else {
            return match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    doc.collapse_to_head();
                    VimResult::Handled
                }
                _ => VimResult::PassThrough,
            };
        };
        match c {
            'd' | 'x' | 'y' | 'c' => {
                let linewise = self.mode == Mode::VisualLine;
                let (a, b) = doc.selection_span();
                let (s, e) = if linewise {
                    ((a.0, 0), (b.0, doc.line_len(b.0)))
                } else {
                    (a, (b.0, (b.1 + 1).min(doc.line_len(b.0))))
                };
                let text = doc.text_between(s, e);
                self.register = (text.clone(), linewise);
                self.mode = if c == 'c' { Mode::Insert } else { Mode::Normal };
                if c != 'y' {
                    if linewise && c != 'c' {
                        doc.delete_lines_range(a.0, b.0);
                    } else {
                        doc.edit(s, e, "");
                    }
                } else {
                    doc.set_cursor(s);
                }
                VimResult::Yanked(text)
            }
            'v' => {
                self.mode = if self.mode == Mode::Visual { Mode::Normal } else { Mode::Visual };
                if self.mode == Mode::Normal {
                    doc.collapse_to_head();
                }
                VimResult::Handled
            }
            'V' => {
                self.mode = if self.mode == Mode::VisualLine { Mode::Normal } else { Mode::VisualLine };
                if self.mode == Mode::Normal {
                    doc.collapse_to_head();
                }
                VimResult::Handled
            }
            'o' => {
                doc.swap_selection_ends();
                VimResult::Handled
            }
            ':' => {
                self.line = Some((':', String::new()));
                VimResult::Handled
            }
            _ => {
                // Motions extend the selection, which is what visual mode is.
                let keep = self.op.take();
                let r = if c.is_ascii_digit() && !(c == '0' && self.count.is_empty()) {
                    self.count.push(c);
                    VimResult::Handled
                } else {
                    self.motion_extending(doc, c)
                };
                self.op = keep;
                r
            }
        }
    }

    fn motion_extending(&mut self, doc: &mut Doc, c: char) -> VimResult {
        let anchor = doc.selection_anchor();
        let r = self.motion(doc, c);
        doc.set_selection(anchor, doc.head());
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Syntax;
    use std::path::Path;

    fn doc(text: &str) -> (Doc, Vim, Syntax) {
        let syntax = Syntax::load();
        let d = Doc::from_text(Path::new("t.rs"), text, &syntax);
        (d, Vim::new(), syntax)
    }

    /// Feed a key sequence the way a user types it.
    fn keys(v: &mut Vim, d: &mut Doc, seq: &str) {
        for c in seq.chars() {
            let key = match c {
                '\n' => KeyEvent::from(KeyCode::Enter),
                '\x1b' => KeyEvent::from(KeyCode::Esc),
                c => KeyEvent::from(KeyCode::Char(c)),
            };
            // Insert mode passes typing through to the editor, as the app does.
            if v.handle(d, key) == VimResult::PassThrough && v.mode == Mode::Insert {
                d.handle_key(key);
            }
        }
    }

    #[test]
    fn modes_switch_the_way_vim_does() {
        let (mut d, mut v, _s) = doc("hello\nworld\n");
        assert_eq!(v.mode, Mode::Normal, "starts in normal mode");

        keys(&mut v, &mut d, "i");
        assert_eq!(v.mode, Mode::Insert);
        assert_eq!(v.mode.label(), "-- INSERT --");
        keys(&mut v, &mut d, "X\x1b");
        assert_eq!(v.mode, Mode::Normal, "Esc returns to normal");
        assert_eq!(d.line_text(0), Some("Xhello"), "typing in insert mode inserts");

        keys(&mut v, &mut d, "v");
        assert_eq!(v.mode, Mode::Visual);
        keys(&mut v, &mut d, "\x1b");
        assert_eq!(v.mode, Mode::Normal);
        keys(&mut v, &mut d, "V");
        assert_eq!(v.mode, Mode::VisualLine);
    }

    #[test]
    fn basic_motions_move_the_cursor() {
        let (mut d, mut v, _s) = doc("alpha beta gamma\nsecond line here\nthird\n");

        keys(&mut v, &mut d, "l");
        assert_eq!(d.head(), (0, 1), "l goes right");
        keys(&mut v, &mut d, "h");
        assert_eq!(d.head(), (0, 0), "h goes left");
        keys(&mut v, &mut d, "j");
        assert_eq!(d.head().0, 1, "j goes down");
        keys(&mut v, &mut d, "k");
        assert_eq!(d.head().0, 0, "k goes up");

        keys(&mut v, &mut d, "$");
        assert_eq!(d.head(), (0, 16), "$ goes to end of line");
        keys(&mut v, &mut d, "0");
        assert_eq!(d.head(), (0, 0), "0 goes to column zero");

        keys(&mut v, &mut d, "w");
        assert_eq!(d.head(), (0, 6), "w to the next word");
        keys(&mut v, &mut d, "w");
        assert_eq!(d.head(), (0, 11), "w again");
        keys(&mut v, &mut d, "b");
        assert_eq!(d.head(), (0, 6), "b back a word");
        keys(&mut v, &mut d, "e");
        assert_eq!(d.head(), (0, 9), "e to end of word");

        keys(&mut v, &mut d, "G");
        assert_eq!(d.head().0, 2, "G to the last line");
        keys(&mut v, &mut d, "gg");
        assert_eq!(d.head().0, 0, "gg to the first line");
    }

    #[test]
    fn counts_multiply_motions() {
        let (mut d, mut v, _s) = doc("one two three four five\na\nb\nc\nd\ne\n");
        keys(&mut v, &mut d, "3w");
        assert_eq!(d.head(), (0, 14), "3w moves three words");

        keys(&mut v, &mut d, "gg");
        keys(&mut v, &mut d, "4j");
        assert_eq!(d.head().0, 4, "4j moves four lines");

        keys(&mut v, &mut d, "2G");
        assert_eq!(d.head().0, 1, "2G goes to line 2");
    }

    #[test]
    fn operators_apply_over_motions() {
        let (mut d, mut v, _s) = doc("alpha beta gamma\n");
        keys(&mut v, &mut d, "dw");
        assert_eq!(d.line_text(0), Some("beta gamma"), "dw deletes a word");

        keys(&mut v, &mut d, "d$");
        assert_eq!(d.line_text(0), Some(""), "d$ deletes to end of line");

        let (mut d, mut v, _s) = doc("alpha beta gamma\n");
        keys(&mut v, &mut d, "d2w");
        assert_eq!(d.line_text(0), Some("gamma"), "a count between operator and motion");

        let (mut d, mut v, _s) = doc("alpha beta\n");
        keys(&mut v, &mut d, "cw");
        assert_eq!(v.mode, Mode::Insert, "c leaves you in insert mode");
        assert_eq!(d.line_text(0), Some("beta"));
    }

    #[test]
    fn line_operators_take_whole_lines() {
        let (mut d, mut v, _s) = doc("one\ntwo\nthree\nfour\n");
        keys(&mut v, &mut d, "dd");
        assert_eq!(d.text(), "two\nthree\nfour", "dd deletes the line");

        keys(&mut v, &mut d, "2dd");
        assert_eq!(d.text(), "four", "2dd deletes two lines");

        let (mut d, mut v, _s) = doc("one\ntwo\nthree\n");
        keys(&mut v, &mut d, "yyp");
        assert_eq!(d.text(), "one\none\ntwo\nthree", "yy then p duplicates the line");

        let (mut d, mut v, _s) = doc("  indented\nnext\n");
        keys(&mut v, &mut d, "cc");
        assert_eq!(v.mode, Mode::Insert);
        assert_eq!(d.line_text(0), Some("  "), "cc keeps the indent");
    }

    #[test]
    fn x_s_and_r_edit_single_characters() {
        let (mut d, mut v, _s) = doc("hello\n");
        keys(&mut v, &mut d, "x");
        assert_eq!(d.line_text(0), Some("ello"), "x deletes a character");
        keys(&mut v, &mut d, "3x");
        assert_eq!(d.line_text(0), Some("o"), "3x deletes three");

        let (mut d, mut v, _s) = doc("cat\n");
        keys(&mut v, &mut d, "rb");
        assert_eq!(d.line_text(0), Some("bat"), "r replaces one character");

        let (mut d, mut v, _s) = doc("cat\n");
        keys(&mut v, &mut d, "s");
        assert_eq!(v.mode, Mode::Insert, "s substitutes and enters insert");
        assert_eq!(d.line_text(0), Some("at"));
    }

    #[test]
    fn insert_entry_keys_land_in_the_right_place() {
        let (mut d, mut v, _s) = doc("  hello\nsecond\n");
        keys(&mut v, &mut d, "A!");
        assert_eq!(d.line_text(0), Some("  hello!"), "A appends at end of line");

        keys(&mut v, &mut d, "\x1bI>");
        assert_eq!(d.line_text(0), Some("  >hello!"), "I inserts at first non-blank");

        let (mut d, mut v, _s) = doc("one\ntwo\n");
        keys(&mut v, &mut d, "onew");
        assert_eq!(d.text(), "one\nnew\ntwo", "o opens a line below");

        let (mut d, mut v, _s) = doc("one\ntwo\n");
        keys(&mut v, &mut d, "Otop");
        assert_eq!(d.text(), "top\none\ntwo", "O opens a line above");
    }

    #[test]
    fn find_character_moves_within_the_line() {
        let (mut d, mut v, _s) = doc("a,b,c,d\n");
        keys(&mut v, &mut d, "f,");
        assert_eq!(d.head(), (0, 1), "f finds the next comma");
        keys(&mut v, &mut d, ";");
        assert_eq!(d.head(), (0, 3), "; repeats the find");
        keys(&mut v, &mut d, ",");
        assert_eq!(d.head(), (0, 1), ", reverses it");
        keys(&mut v, &mut d, "0t,");
        assert_eq!(d.head(), (0, 0), "t stops before the character");
        keys(&mut v, &mut d, "$F,");
        assert_eq!(d.head(), (0, 5), "F searches backwards");
    }

    #[test]
    fn visual_mode_selects_and_operates() {
        let (mut d, mut v, _s) = doc("hello world\n");
        keys(&mut v, &mut d, "vlld");
        assert_eq!(d.line_text(0), Some("lo world"), "v then motion then d deletes the selection");
        assert_eq!(v.mode, Mode::Normal, "d leaves visual mode");

        let (mut d, mut v, _s) = doc("one\ntwo\nthree\n");
        keys(&mut v, &mut d, "Vjd");
        assert_eq!(d.text(), "three", "V selects whole lines");

        let (mut d, mut v, _s) = doc("abc\n");
        keys(&mut v, &mut d, "vly");
        assert_eq!(v.mode, Mode::Normal, "y leaves visual mode");
        assert_eq!(d.text(), "abc", "y changes nothing");
    }

    #[test]
    fn undo_and_redo() {
        let (mut d, mut v, _s) = doc("one\ntwo\n");
        keys(&mut v, &mut d, "dd");
        assert_eq!(d.text(), "two");
        keys(&mut v, &mut d, "u");
        assert_eq!(d.text(), "one\ntwo", "u undoes");
        let redo = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        v.handle(&mut d, redo);
        assert_eq!(d.text(), "two", "Ctrl+r redoes");
    }

    #[test]
    fn join_and_paste() {
        let (mut d, mut v, _s) = doc("one\ntwo\nthree\n");
        keys(&mut v, &mut d, "J");
        assert_eq!(d.line_text(0), Some("one two"), "J joins with one space");

        let (mut d, mut v, _s) = doc("abc\n");
        keys(&mut v, &mut d, "ylp");
        assert_eq!(d.line_text(0), Some("aabc"), "yl then p pastes after the cursor");
    }

    #[test]
    fn ex_and_search_lines_are_reported_to_the_app() {
        let (mut d, mut v, _s) = doc("one\ntwo\n");
        for c in ":wq".chars() {
            v.handle(&mut d, KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(v.pending_line().as_deref(), Some(":wq"), "the line shows while typing");
        assert_eq!(v.handle(&mut d, KeyEvent::from(KeyCode::Enter)), VimResult::Ex("wq".into()));

        for c in "/needle".chars() {
            v.handle(&mut d, KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(
            v.handle(&mut d, KeyEvent::from(KeyCode::Enter)),
            VimResult::Search("needle".into(), true)
        );

        // Esc abandons the line without running it.
        v.handle(&mut d, KeyEvent::from(KeyCode::Char(':')));
        v.handle(&mut d, KeyEvent::from(KeyCode::Char('q')));
        assert_eq!(v.handle(&mut d, KeyEvent::from(KeyCode::Esc)), VimResult::Handled);
        assert_eq!(v.pending_line(), None);
    }

    #[test]
    fn unknown_keys_pass_through_rather_than_being_swallowed() {
        let (mut d, mut v, _s) = doc("one\n");
        // Arrow keys and shortcuts must keep working in normal mode.
        assert_eq!(v.handle(&mut d, KeyEvent::from(KeyCode::Down)), VimResult::PassThrough);
        assert_eq!(
            v.handle(&mut d, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            VimResult::PassThrough,
            "Ctrl+S must still save"
        );
        // A half-typed operator is abandoned, not applied to the next key.
        v.handle(&mut d, KeyEvent::from(KeyCode::Char('d')));
        assert_eq!(v.handle(&mut d, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)), VimResult::PassThrough);
        assert_eq!(d.text(), "one", "nothing was deleted");
    }
}
