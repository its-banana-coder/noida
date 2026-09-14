//! Display layout (folding, soft wrap, scrolling) and rendering.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{Cursor, Doc, Pos, Syntax, char_width, lang};
use crate::git::LineMark;
use crate::lsp::Diagnostic;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub line: usize,
    pub seg: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub struct ViewState {
    pub height: usize,
    pub width: usize,
    gutter: u16,
    area: Rect,
    rows: Vec<Row>,
    sticky: usize,
    depth: (u64, Vec<i32>),
    column_anchor: Option<(usize, usize)>,
    pending_center: Option<usize>,
}

pub enum Click {
    Text,
    Fold,
}

impl Doc {
    // ---- layout helpers ----

    /// Char offsets where each soft-wrapped segment of a line starts.
    pub(super) fn segments(&self, line: usize) -> Vec<usize> {
        let width = self.view.width;
        if !self.wrap || width < 8 {
            return vec![0];
        }
        let mut starts = vec![0];
        let mut col = 0;
        let mut seg_col = 0;
        for (i, c) in self.lines[line].chars().enumerate() {
            let w = char_width(c, col);
            if col + w - seg_col > width && i > *starts.last().unwrap() {
                starts.push(i);
                seg_col = col;
            }
            col += w;
        }
        starts
    }

    fn seg_of(&self, p: Pos) -> (usize, usize) {
        let segs = self.segments(p.0);
        let i = segs.partition_point(|&s| s <= p.1).saturating_sub(1);
        (i, segs[i])
    }

    pub(super) fn view_col(&self, p: Pos) -> usize {
        let (_, start) = self.seg_of(p);
        self.display_col(p.0, p.1) - self.display_col(p.0, start)
    }

    pub(super) fn view_next_line(&self, line: usize) -> Option<usize> {
        (line + 1..self.lines.len()).find(|&l| !self.is_hidden(l))
    }

    pub(super) fn view_prev_line(&self, line: usize) -> usize {
        (0..line).rev().find(|&l| !self.is_hidden(l)).unwrap_or(line)
    }

    pub(super) fn view_visible_line(&self, line: usize) -> usize {
        if self.is_hidden(line) { self.view_prev_line(line) } else { line }
    }

    fn next_row(&self, (line, seg): (usize, usize)) -> Option<(usize, usize)> {
        if seg + 1 < self.segments(line).len() {
            Some((line, seg + 1))
        } else {
            self.view_next_line(line).map(|l| (l, 0))
        }
    }

    fn prev_row(&self, (line, seg): (usize, usize)) -> Option<(usize, usize)> {
        if seg > 0 {
            Some((line, seg - 1))
        } else {
            let p = self.view_prev_line(line);
            (p != line).then(|| (p, self.segments(p).len() - 1))
        }
    }

    pub(super) fn view_move_rows(&self, p: Pos, want: usize, rows: isize) -> Pos {
        let (seg, _) = self.seg_of(p);
        let mut row = (self.view_visible_line(p.0), seg);
        for _ in 0..rows.unsigned_abs() {
            let next = if rows > 0 { self.next_row(row) } else { self.prev_row(row) };
            match next {
                Some(r) => row = r,
                None => {
                    // Past the first/last row: jump to the start/end of the line.
                    return if rows > 0 { (row.0, self.line_chars(row.0)) } else { (row.0, 0) };
                }
            }
        }
        let segs = self.segments(row.0);
        let start = segs[row.1];
        let end = segs.get(row.1 + 1).copied().unwrap_or(self.line_chars(row.0));
        let base = self.display_col(row.0, start);
        let mut cx = start;
        while cx < end && self.display_col(row.0, cx + 1) - base <= want {
            cx += 1;
        }
        // On a wrapped segment, the end position belongs to the next segment.
        if cx == end && end < self.line_chars(row.0) {
            cx = end.saturating_sub(1).max(start);
        }
        (row.0, cx)
    }

    pub(super) fn view_center(&mut self, line: usize) {
        if self.view.height == 0 {
            // Not rendered yet: center once the size is known.
            self.view.pending_center = Some(line);
            return;
        }
        self.update_hidden();
        let mut top = (self.view_visible_line(line), 0);
        for _ in 0..self.view.height / 3 {
            match self.prev_row(top) {
                Some(r) => top = r,
                None => break,
            }
        }
        self.top = top;
        self.reveal = true;
    }

    pub(super) fn view_scroll(&mut self, rows: isize) {
        for _ in 0..rows.unsigned_abs() {
            let next = if rows > 0 { self.next_row(self.top) } else { self.prev_row(self.top) };
            match next {
                Some(r) => self.top = r,
                None => break,
            }
        }
    }

    pub fn scroll(&mut self, delta: isize) {
        self.update_hidden();
        self.view_scroll(delta);
        self.reveal = false;
    }

    fn reveal_primary(&mut self, height: usize) {
        let head = self.primary().head;
        let line = self.view_visible_line(head.0);
        let (seg, _) = if line == head.0 { self.seg_of(head) } else { (0, 0) };
        let target = (line, seg);
        if self.top.0 >= self.lines.len() || self.is_hidden(self.top.0) {
            self.top = (self.view_visible_line(self.top.0.min(self.lines.len() - 1)), 0);
        }
        if self.top.1 >= self.segments(self.top.0).len() {
            self.top.1 = 0;
        }
        if target < self.top {
            self.top = target;
            return;
        }
        let mut row = self.top;
        for _ in 0..height.saturating_sub(1) {
            if row == target {
                return;
            }
            match self.next_row(row) {
                Some(r) => row = r,
                None => return,
            }
        }
        if row == target {
            return;
        }
        let mut top = target;
        for _ in 0..height.saturating_sub(1) {
            match self.prev_row(top) {
                Some(r) => top = r,
                None => break,
            }
        }
        self.top = top;
    }

    fn bracket_depths(&mut self) -> Option<&Vec<i32>> {
        if self.lines.len() > 20_000 {
            return None;
        }
        if self.view.depth.0 != self.edits || self.view.depth.1.len() != self.lines.len() {
            let mut depths = Vec::with_capacity(self.lines.len());
            let mut d = 0i32;
            for l in &self.lines {
                depths.push(d);
                for c in l.chars() {
                    match c {
                        '(' | '[' | '{' => d += 1,
                        ')' | ']' | '}' => d -= 1,
                        _ => {}
                    }
                }
            }
            self.view.depth = (self.edits, depths);
        }
        Some(&self.view.depth.1)
    }

    fn matching_bracket(&self) -> Option<(Pos, Pos)> {
        let h = self.primary().head;
        let (p, c) = match self.char_at(h).filter(|c| lang::bracket_pair(*c).is_some()) {
            Some(c) => (h, c),
            None => {
                let c = self.char_before(h).filter(|c| lang::bracket_pair(*c).is_some())?;
                ((h.0, h.1 - 1), c)
            }
        };
        let (open, close, forward) = lang::bracket_pair(c)?;
        let mut depth = 0i32;
        let mut budget = 200_000usize;
        if forward {
            for l in p.0..self.lines.len().min(p.0 + 3000) {
                let skip = if l == p.0 { p.1 } else { 0 };
                for (i, ch) in self.lines[l].chars().enumerate().skip(skip) {
                    budget = budget.checked_sub(1)?;
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            return Some((p, (l, i)));
                        }
                    }
                }
            }
        } else {
            for l in (p.0.saturating_sub(3000)..=p.0).rev() {
                let chars: Vec<char> = self.lines[l].chars().collect();
                let end = if l == p.0 { p.1 + 1 } else { chars.len() };
                for i in (0..end).rev() {
                    budget = budget.checked_sub(1)?;
                    if chars[i] == close {
                        depth += 1;
                    } else if chars[i] == open {
                        depth -= 1;
                        if depth == 0 {
                            return Some((p, (l, i)));
                        }
                    }
                }
            }
        }
        None
    }

    // ---- mouse ----

    fn pos_at(&self, x: u16, y: u16) -> Pos {
        let area = self.view.area;
        let row_i = (y.saturating_sub(area.y) as usize).min(self.view.rows.len().saturating_sub(1));
        let Some(row) = self.view.rows.get(row_i) else { return self.primary().head };
        let col = x.saturating_sub(area.x + self.view.gutter) as usize + if self.wrap { 0 } else { self.scroll_x };
        let base = self.display_col(row.line, row.start);
        let mut cx = row.start;
        while cx < row.end && self.display_col(row.line, cx + 1) - base <= col {
            cx += 1;
        }
        (row.line, cx)
    }

    pub fn click(&mut self, x: u16, y: u16, extend: bool, add_cursor: bool) -> Click {
        let area = self.view.area;
        if x < area.x + self.view.gutter {
            let row_i = y.saturating_sub(area.y) as usize;
            if let Some(row) = self.view.rows.get(row_i).copied() {
                if x >= area.x + self.view.gutter.saturating_sub(2) && self.toggle_fold(row.line) {
                    return Click::Fold;
                }
                self.cursors = vec![Cursor { anchor: (row.line, 0), head: self.clamp((row.line + 1, 0)), want: None }];
            }
            return Click::Text;
        }
        let p = self.pos_at(x, y);
        self.view.column_anchor = None;
        if add_cursor {
            if let Some(i) = self.cursors.iter().position(|c| c.is_empty() && c.head == p) {
                if self.cursors.len() > 1 {
                    self.cursors.remove(i);
                }
            } else {
                self.cursors.push(Cursor::at(p));
            }
        } else if extend {
            let last = self.cursors.len() - 1;
            self.cursors[last].head = p;
            self.cursors.truncate(1);
            self.cursors[0].head = p;
        } else {
            self.cursors = vec![Cursor::at(p)];
        }
        self.reveal = true;
        Click::Text
    }

    /// Drag to select; with `column`, make a rectangular multi-cursor selection.
    pub fn drag(&mut self, x: u16, y: u16, column: bool) {
        let p = self.pos_at(x, y);
        if column {
            let anchor = match self.view.column_anchor {
                Some(a) => a,
                None => {
                    let a = self.cursors.first().map_or(p, |c| c.anchor);
                    let a = (a.0, self.display_col(a.0, a.1));
                    self.view.column_anchor = Some(a);
                    a
                }
            };
            let col = self.display_col(p.0, p.1);
            let (l0, l1) = (anchor.0.min(p.0), anchor.0.max(p.0));
            self.cursors = (l0..=l1)
                .filter(|&l| !self.is_hidden(l))
                .map(|l| Cursor { anchor: (l, self.cx_for_display_col(l, anchor.1)), head: (l, self.cx_for_display_col(l, col)), want: None })
                .collect();
        } else {
            let last = self.cursors.len() - 1;
            self.cursors[last].head = p;
        }
        self.reveal = true;
    }

    // ---- rendering ----

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, focused: bool, syntax: &Syntax, diags: &[Diagnostic], sticky_scroll: bool) -> Option<(u16, u16)> {
        self.update_hidden();
        let digits = self.lines.len().to_string().len().max(3);
        let gutter = digits as u16 + 4;
        self.view.gutter = gutter;
        self.view.area = area;
        self.view.height = area.height as usize;
        self.view.width = area.width.saturating_sub(gutter) as usize;
        let (height, width) = (self.view.height, self.view.width);
        if height == 0 || width == 0 {
            return None;
        }
        if let Some(line) = self.view.pending_center.take() {
            self.view_center(line);
        }

        // Sticky scroll shows enclosing definitions of the top line.
        let mut sticky: Vec<crate::symbols::Symbol> = Vec::new();
        if sticky_scroll && height > 8 {
            let top_line = self.top.0;
            sticky = self.scopes_at(top_line).into_iter().filter(|s| s.line < top_line).collect();
            if sticky.len() > 3 {
                sticky.drain(..sticky.len() - 3);
            }
        }
        if self.reveal {
            self.reveal_primary(height.saturating_sub(sticky.len()));
            if !self.wrap {
                let head = self.primary().head;
                let col = self.display_col(head.0, head.1);
                if col < self.scroll_x {
                    self.scroll_x = col;
                } else if col >= self.scroll_x + width {
                    self.scroll_x = col + 1 - width;
                }
            }
            self.reveal = false;
        }
        if sticky_scroll && height > 8 {
            let top_line = self.top.0;
            sticky = self.scopes_at(top_line).into_iter().filter(|s| s.line < top_line).collect();
            if sticky.len() > 3 {
                sticky.drain(..sticky.len() - 3);
            }
        }
        if self.wrap {
            self.scroll_x = 0;
        }

        // Build the visible rows.
        let mut rows = Vec::with_capacity(height);
        let mut row = Some(self.top);
        while let Some((line, seg)) = row {
            if rows.len() >= height {
                break;
            }
            let segs = self.segments(line);
            let start = segs.get(seg).copied().unwrap_or(0);
            let end = segs.get(seg + 1).copied().unwrap_or(self.line_chars(line));
            rows.push(Row { line, seg, start, end });
            row = self.next_row((line, seg));
        }
        let max_line = rows.last().map_or(0, |r| r.line) + 1;
        self.ensure_highlight(max_line, syntax);
        let bracket = self.matching_bracket();
        let depths = self.bracket_depths().cloned();
        let flash = self.flash.filter(|(_, _, t)| t.elapsed().as_millis() < 1500);
        let primary = *self.primary();
        let head_line = primary.head.0;
        let search_current = self.search.as_ref().and_then(|s| s.current.map(|i| s.matches[i]));
        let base = Style::default().fg(theme::FG());
        let mut cursor_pos = None;

        for (ri, r) in rows.iter().enumerate() {
            let y = area.y + ri as u16;
            let li = r.line;
            let mut line_bg = None;
            if li == head_line && focused && self.cursors.len() == 1 {
                line_bg = Some(theme::CURRENT_LINE());
            }
            if let Some((s, e, _)) = flash {
                if li >= s && li <= e {
                    line_bg = Some(theme::FLASH());
                }
            }
            if let Some(bg) = line_bg {
                buf.set_style(Rect::new(area.x, y, area.width, 1), Style::default().bg(bg));
            }

            // Gutter: number, diagnostic, git mark, fold marker.
            if r.seg == 0 {
                let num_style = if li == head_line { Style::default().fg(theme::ACCENT()) } else { Style::default().fg(theme::DIM()) };
                buf.set_string(area.x, y, format!("{:>digits$}", li + 1), num_style);
                if let Some(d) = diags.iter().filter(|d| d.line == li).min_by_key(|d| d.severity) {
                    let color = match d.severity {
                        1 => theme::ERROR(),
                        2 => theme::WARN(),
                        _ => theme::DIM(),
                    };
                    buf.set_string(area.x + digits as u16, y, "●", Style::default().fg(color));
                }
                if let Some(mark) = self.marks.get(&li) {
                    let (sym, color) = match mark {
                        LineMark::Added => ("▎", theme::ADDED()),
                        LineMark::Modified => ("▎", theme::DIR()),
                        LineMark::DeletedBelow => ("▁", theme::ERROR()),
                    };
                    buf.set_string(area.x + digits as u16 + 1, y, sym, Style::default().fg(color));
                }
                if self.folds.contains(&li) {
                    buf.set_string(area.x + digits as u16 + 2, y, "▸", Style::default().fg(theme::ACCENT()));
                } else if li + 1 < self.lines.len() && self.indent_of(li).is_some_and(|i| self.indent_of(li + 1).is_some_and(|n| n > i)) {
                    buf.set_string(area.x + digits as u16 + 2, y, "▾", Style::default().fg(theme::BORDER()));
                }
            } else {
                buf.set_string(area.x + digits as u16 + 2, y, "↪", Style::default().fg(theme::BORDER()));
            }

            // Selections, matches and diagnostics touching this line.
            let sels: Vec<(Pos, Pos)> = self.cursors.iter().filter(|c| !c.is_empty()).map(Cursor::range).filter(|(s, e)| s.0 <= li && e.0 >= li).collect();
            let matches: Vec<(Pos, Pos)> = self
                .search
                .as_ref()
                .map(|s| {
                    let i = s.matches.partition_point(|m| m.0.0 < li);
                    s.matches[i..].iter().take_while(|m| m.0.0 == li).copied().collect()
                })
                .unwrap_or_default();
            let line_diags: Vec<&Diagnostic> = diags.iter().filter(|d| d.line == li).collect();
            let underline_until = |col: usize| -> usize {
                let chars: Vec<char> = self.lines[li].chars().collect();
                let mut e = col;
                while e < chars.len() && (chars[e].is_alphanumeric() || chars[e] == '_') {
                    e += 1;
                }
                if e == col { (col + 1).min(chars.len().max(col + 1)) } else { e }
            };

            let spans = self.hl_lines.get(li);
            let mut span_i = 0;
            let text_x = area.x + gutter;
            let seg_base = self.display_col(li, r.start);
            let mut depth = depths.as_ref().and_then(|d| d.get(li).copied()).unwrap_or(0);
            let mut col = 0usize;
            for (ci, (bi, ch)) in self.lines[li].char_indices().enumerate() {
                let w = char_width(ch, col);
                let start = col;
                col += w;
                if matches!(ch, '(' | '[' | '{') {
                    depth += 1;
                }
                let this_depth = depth;
                if matches!(ch, ')' | ']' | '}') {
                    depth -= 1;
                }
                if ci < r.start || ci >= r.end {
                    continue;
                }
                let vis = start - seg_base;
                if vis + w <= self.scroll_x {
                    continue;
                }
                if vis >= self.scroll_x + width {
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
                if depths.is_some() && matches!(ch, '(' | '[' | '{' | ')' | ']' | '}') {
                    let d = if matches!(ch, ')' | ']' | '}') { this_depth } else { this_depth - 1 };
                    style = style.fg(match d.rem_euclid(3) {
                        0 => theme::PAIR1(),
                        1 => theme::PAIR2(),
                        _ => theme::PAIR3(),
                    });
                }
                let pos = (li, ci);
                if matches.iter().any(|&(s, e)| pos >= s && pos < e) {
                    let current = search_current.is_some_and(|(s, e)| pos >= s && pos < e);
                    style = style.bg(if current { theme::ACCENT() } else { theme::MATCH_BG() });
                    if current {
                        style = style.fg(theme::HINT_FG());
                    }
                }
                if sels.iter().any(|&(s, e)| pos >= s && pos < e) {
                    style = style.bg(theme::SELECT());
                }
                if bracket.is_some_and(|(a, b)| a == pos || b == pos) {
                    style = style.bg(theme::BRACKET_BG()).add_modifier(Modifier::BOLD);
                }
                let in_diag = |d: &&&Diagnostic| {
                    let end = if d.end_line == li && d.end_col > d.col { d.end_col } else if d.end_line > li { usize::MAX } else { underline_until(d.col) };
                    ci >= d.col && ci < end
                };
                if let Some(d) = line_diags.iter().find(in_diag) {
                    let color = match d.severity {
                        1 => theme::ERROR(),
                        2 => theme::WARN(),
                        _ => theme::DIM(),
                    };
                    style = style.add_modifier(Modifier::UNDERLINED).underline_color(color);
                }
                let x0 = vis.saturating_sub(self.scroll_x);
                if ch == '\t' {
                    for k in x0..(vis + w - self.scroll_x).min(width) {
                        buf[(text_x + k as u16, y)].set_symbol(" ").set_style(style);
                    }
                } else if x0 + w <= width && vis >= self.scroll_x {
                    let sym = if ch.is_control() { '?' } else { ch };
                    buf[(text_x + x0 as u16, y)].set_char(sym).set_style(style);
                }
            }
            let row_end_col = self.display_col(li, r.end) - seg_base;
            let is_last_seg = r.end == self.line_chars(li);
            // Selected line breaks and the fold placeholder.
            if is_last_seg && row_end_col >= self.scroll_x && row_end_col - self.scroll_x < width {
                let eol = (li, r.end);
                if sels.iter().any(|&(s, e)| eol >= s && eol < e) {
                    buf[(text_x + (row_end_col - self.scroll_x) as u16, y)].set_style(Style::default().bg(theme::SELECT()));
                }
                if self.folds.contains(&li) {
                    let x = text_x + (row_end_col - self.scroll_x) as u16 + 1;
                    if x + 4 < area.x + area.width {
                        buf.set_string(x, y, " ⋯ ", Style::default().fg(theme::DIM()).bg(theme::SELECT_DIM()));
                    }
                }
            }
            // Cursors: the primary uses the terminal cursor, others are drawn.
            for c in &self.cursors {
                if c.head.0 != li || c.head.1 < r.start || (c.head.1 >= r.end && !(is_last_seg && c.head.1 == r.end)) {
                    continue;
                }
                let vis = self.display_col(li, c.head.1) - seg_base;
                if vis < self.scroll_x || vis - self.scroll_x >= width {
                    continue;
                }
                let x = text_x + (vis - self.scroll_x) as u16;
                if *c == primary {
                    cursor_pos = Some((x, y));
                } else {
                    buf[(x, y)].set_style(Style::default().add_modifier(Modifier::REVERSED));
                }
            }
        }
        for ri in rows.len()..height {
            buf.set_string(area.x, area.y + ri as u16, format!("{:>digits$}", "~"), Style::default().fg(theme::BORDER()));
        }

        // Sticky scroll overlay.
        let sticky_n = sticky.len().min(rows.len().saturating_sub(1));
        for (i, s) in sticky.iter().take(sticky_n).enumerate() {
            let y = area.y + i as u16;
            let bg = Style::default().bg(theme::STATUS_BG());
            buf.set_style(Rect::new(area.x, y, area.width, 1), bg);
            buf.set_string(area.x, y, format!("{:>digits$}", s.line + 1), Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()));
            let spans = self.hl_lines.get(s.line);
            let mut col = 0;
            for (bi, ch) in self.lines[s.line].char_indices() {
                let w = char_width(ch, col);
                if col + w > width {
                    break;
                }
                let mut style = Style::default().fg(theme::FG()).bg(theme::STATUS_BG());
                if let Some(sp) = spans.and_then(|sp| sp.iter().find(|(_, r)| r.contains(&bi))) {
                    style = sp.0.bg(theme::STATUS_BG());
                }
                if i + 1 == sticky_n {
                    style = style.add_modifier(Modifier::UNDERLINED).underline_color(theme::BORDER());
                }
                if ch != '\t' {
                    buf[(area.x + gutter + col as u16, y)].set_char(ch).set_style(style);
                }
                col += w;
            }
            if cursor_pos.is_some_and(|(_, cy)| cy == y) {
                cursor_pos = None;
            }
        }
        self.view.sticky = sticky_n;
        self.view.rows = rows;
        if focused { cursor_pos } else { None }
    }
}
