//! Agent-centric views: files each agent changed with reviewed tracking, and
//! a side-by-side comparison of two agents' work.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::views::ViewResult;
use crate::changes::Version;
use crate::git::FileState;
use crate::theme;

// --------------------------------------------------------------- changes --

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub abs: PathBuf,
    /// Relative to the agent's repository (worktree or main checkout).
    pub repo_rel: String,
    /// Activity path, the key for reviewed marks.
    pub key: String,
    pub state: Option<FileState>,
    pub created: bool,
    pub version: Version,
    pub reviewed: bool,
}

#[derive(Clone, Debug)]
pub struct ChangeGroup {
    pub agent: String,
    pub repo_root: Option<PathBuf>,
    pub worktree: Option<String>,
    pub files: Vec<ChangedFile>,
}

impl ChangeGroup {
    pub fn unreviewed(&self) -> usize {
        self.files.iter().filter(|f| !f.reviewed).count()
    }
}

enum ChangeRow {
    Header(usize),
    File(usize, usize),
    Empty(usize),
    Blank,
}

pub struct AgentChangesView {
    groups: Vec<ChangeGroup>,
    rows: Vec<ChangeRow>,
    selected: usize,
    scroll: usize,
    height: usize,
    area: Rect,
}

impl AgentChangesView {
    pub fn new(groups: Vec<ChangeGroup>) -> Self {
        let mut v = Self { groups: Vec::new(), rows: Vec::new(), selected: 0, scroll: 0, height: 20, area: Rect::default() };
        v.set_groups(groups);
        v
    }

    /// Replace the data, keeping the selection on the same agent and file when possible.
    pub fn set_groups(&mut self, groups: Vec<ChangeGroup>) {
        let keep = self.current().map(|(g, f)| (g.agent.clone(), f.map(|f| f.key.clone())));
        self.groups = groups;
        self.rows.clear();
        for (gi, g) in self.groups.iter().enumerate() {
            self.rows.push(ChangeRow::Header(gi));
            if g.files.is_empty() {
                self.rows.push(ChangeRow::Empty(gi));
            }
            for fi in 0..g.files.len() {
                self.rows.push(ChangeRow::File(gi, fi));
            }
            self.rows.push(ChangeRow::Blank);
        }
        let found = keep.and_then(|(agent, key)| {
            self.rows.iter().position(|r| match r {
                ChangeRow::File(g, f) => self.groups[*g].agent == agent && key.as_ref() == Some(&self.groups[*g].files[*f].key),
                _ => false,
            })
            .or_else(|| self.rows.iter().position(|r| matches!(r, ChangeRow::Header(g) if self.groups[*g].agent == agent)))
        });
        self.selected = found.unwrap_or_else(|| self.rows.iter().position(|r| matches!(r, ChangeRow::File(..))).unwrap_or(0));
    }

    fn current(&self) -> Option<(&ChangeGroup, Option<&ChangedFile>)> {
        match self.rows.get(self.selected)? {
            ChangeRow::Header(g) | ChangeRow::Empty(g) => Some((&self.groups[*g], None)),
            ChangeRow::File(g, f) => Some((&self.groups[*g], Some(&self.groups[*g].files[*f]))),
            ChangeRow::Blank => None,
        }
    }

    fn selectable(&self, i: usize) -> bool {
        matches!(self.rows.get(i), Some(ChangeRow::Header(_) | ChangeRow::File(..)))
    }

    fn step(&mut self, delta: isize) {
        let mut i = self.selected as isize;
        loop {
            i += delta;
            if i < 0 || i as usize >= self.rows.len() {
                return;
            }
            if self.selectable(i as usize) {
                self.selected = i as usize;
                return;
            }
        }
    }

    fn review(group: &ChangeGroup, files: Vec<String>, what: &str) -> ViewResult {
        let Some(root) = group.repo_root.clone() else { return ViewResult::Message("not a git repository".into(), true) };
        if files.is_empty() {
            return ViewResult::Message(format!("{}: no unreviewed files", group.agent), false);
        }
        let title = match &group.worktree {
            Some(b) => format!("{what} · {} · {b}", group.agent),
            None => format!("{what} · {}", group.agent),
        };
        ViewResult::Review { root, title, files }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ViewResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ViewResult::Close,
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::PageDown => (0..self.height.max(1)).for_each(|_| self.step(1)),
            KeyCode::PageUp => (0..self.height.max(1)).for_each(|_| self.step(-1)),
            KeyCode::Enter => {
                if let Some((_, Some(f))) = self.current() {
                    return ViewResult::Open(f.abs.clone(), 1);
                }
            }
            KeyCode::Char('r') => {
                if let Some((g, Some(f))) = self.current() {
                    return Self::review(g, vec![f.repo_rel.clone()], "Review");
                }
            }
            KeyCode::Char('R') => {
                if let Some((g, _)) = self.current() {
                    let files = g.files.iter().filter(|f| !f.reviewed).map(|f| f.repo_rel.clone()).collect();
                    return Self::review(g, files, "Unreviewed");
                }
            }
            KeyCode::Char('v') => {
                if let Some((g, Some(f))) = self.current() {
                    return ViewResult::ToggleReviewed { agent: g.agent.clone(), path: f.key.clone(), version: f.version, reviewed: f.reviewed };
                }
            }
            _ => {}
        }
        self.reveal();
        ViewResult::None
    }

    fn reveal(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.height {
            self.scroll = self.selected + 1 - self.height;
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).clamp(0, self.rows.len().saturating_sub(1) as isize) as usize;
    }

    pub fn click(&mut self, y: u16) -> ViewResult {
        let row = self.scroll + y.saturating_sub(self.area.y) as usize;
        if self.selectable(row) {
            let again = row == self.selected;
            self.selected = row;
            if again {
                return self.handle_key(KeyEvent::from(KeyCode::Enter));
            }
        }
        ViewResult::None
    }

    pub fn status(&self) -> &'static str {
        "↑↓ select · Enter open · r review file · R review agent's unreviewed files · v toggle reviewed · Esc close"
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.area = area;
        self.height = area.height as usize;
        if self.groups.is_empty() {
            buf.set_stringn(area.x + 2, area.y + 1, "No agent tabs.", area.width as usize, Style::default().fg(theme::DIM()));
            return;
        }
        self.reveal();
        let w = area.width as usize;
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(self.height) {
            let y = area.y + (i - self.scroll) as u16;
            let full = Rect::new(area.x, y, area.width, 1);
            let selected = i == self.selected;
            let sel_bg = |s: Style| if selected { s.bg(theme::SELECT()) } else { s };
            if selected {
                buf.set_style(full, Style::default().bg(theme::SELECT()));
            }
            match row {
                ChangeRow::Header(gi) => {
                    let g = &self.groups[*gi];
                    let style = sel_bg(Style::default().fg(theme::FG()).bg(theme::STATUS_BG()).add_modifier(Modifier::BOLD));
                    buf.set_style(full, style);
                    let n = g.files.len();
                    let mut text = format!("{}: {n} file{}", g.agent, if n == 1 { "" } else { "s" });
                    let unreviewed = g.unreviewed();
                    if n > 0 {
                        text.push_str(&if unreviewed == 0 { ", all reviewed".to_string() } else { format!(", {unreviewed} unreviewed") });
                    }
                    let (x, _) = buf.set_stringn(area.x + 1, y, &text, w.saturating_sub(1), style);
                    if let Some(b) = &g.worktree {
                        buf.set_stringn(x + 2, y, format!("worktree {b}"), w.saturating_sub((x - area.x) as usize + 2), style.fg(theme::DIM()).remove_modifier(Modifier::BOLD));
                    }
                }
                ChangeRow::Empty(_) => {
                    let msg = "  no edits recorded (NOIDA sees Claude's edits via hooks; worktree agents also show their git changes)";
                    buf.set_stringn(area.x + 1, y, msg, w.saturating_sub(1), sel_bg(Style::default().fg(theme::DIM())));
                }
                ChangeRow::File(gi, fi) => {
                    let f = &self.groups[*gi].files[*fi];
                    let (mark, mark_style) = if f.reviewed { ("✓", Style::default().fg(theme::ADDED())) } else { ("○", Style::default().fg(theme::ACCENT())) };
                    let (glyph, glyph_style) = match f.state {
                        None => ("·", Style::default().fg(theme::DIM())),
                        Some(FileState::Staged) => ("✓", Style::default().fg(theme::ADDED())),
                        Some(FileState::Untracked) => ("U", Style::default().fg(theme::ADDED())),
                        Some(FileState::Conflict | FileState::Deleted) => (f.state.unwrap().glyph(), Style::default().fg(theme::ERROR())),
                        Some(s) => (s.glyph(), Style::default().fg(theme::ACCENT())),
                    };
                    buf.set_string(area.x + 3, y, mark, sel_bg(mark_style));
                    buf.set_string(area.x + 5, y, glyph, sel_bg(glyph_style));
                    let path_style = sel_bg(Style::default().fg(if f.reviewed { theme::DIM() } else { theme::FG() }));
                    let (x, _) = buf.set_stringn(area.x + 7, y, &f.repo_rel, w.saturating_sub(8), path_style);
                    let state = match f.state {
                        None => "clean",
                        Some(FileState::Staged) => "accepted",
                        Some(FileState::Modified) => "modified",
                        Some(FileState::Untracked) => "untracked",
                        Some(FileState::Deleted) => "deleted",
                        Some(FileState::Conflict) => "conflict",
                    };
                    let tag = format!("{state}{}{}", if f.created { " · new" } else { "" }, if f.reviewed { " · reviewed" } else { "" });
                    let right = area.x + area.width;
                    if x + 2 < right {
                        buf.set_stringn(x + 2, y, tag, (right - x - 2) as usize, sel_bg(Style::default().fg(theme::DIM())));
                    }
                }
                ChangeRow::Blank => {}
            }
        }
    }
}

// --------------------------------------------------------------- compare --

#[derive(Clone, Debug)]
pub struct CompareFile {
    pub path: String,
    /// (added, deleted); `None` for binary files.
    pub counts: Option<(usize, usize)>,
    pub shared: bool,
}

#[derive(Clone, Debug)]
pub struct CompareSide {
    pub agent: String,
    pub repo_root: Option<PathBuf>,
    /// "main checkout" or "worktree noida/x vs 1a2b3c4".
    pub where_: String,
    pub files: Vec<CompareFile>,
    pub commands: Vec<String>,
    pub prompt: String,
    pub note: Option<String>,
}

/// Marks paths present on both sides as shared.
pub fn mark_shared(sides: &mut [CompareSide; 2]) {
    let [a, b] = sides;
    for f in &mut a.files {
        f.shared = b.files.iter().any(|g| g.path == f.path);
    }
    for f in &mut b.files {
        f.shared = a.files.iter().any(|g| g.path == f.path);
    }
}

pub struct CompareView {
    pub title: String,
    sides: [CompareSide; 2],
    col: usize,
    sel: [usize; 2],
    scroll: [usize; 2],
    /// Per column, the file index shown on each rendered line.
    line_files: [Vec<Option<usize>>; 2],
    area: Rect,
}

impl CompareView {
    pub fn new(sides: [CompareSide; 2]) -> Self {
        let title = format!("Compare {} ⇄ {}", sides[0].agent, sides[1].agent);
        let col = if sides[0].files.is_empty() && !sides[1].files.is_empty() { 1 } else { 0 };
        Self { title, sides, col, sel: [0, 0], scroll: [0, 0], line_files: [Vec::new(), Vec::new()], area: Rect::default() }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ViewResult {
        let n = self.sides[self.col].files.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ViewResult::Close,
            KeyCode::Left | KeyCode::Char('h') => self.col = 0,
            KeyCode::Right | KeyCode::Char('l') => self.col = 1,
            KeyCode::Tab => self.col = 1 - self.col,
            KeyCode::Down | KeyCode::Char('j') => self.sel[self.col] = (self.sel[self.col] + 1).min(n.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.sel[self.col] = self.sel[self.col].saturating_sub(1),
            KeyCode::Enter => {
                let side = &self.sides[self.col];
                let Some(f) = side.files.get(self.sel[self.col]) else { return ViewResult::None };
                let Some(root) = side.repo_root.clone() else { return ViewResult::Message("not a git repository".into(), true) };
                return ViewResult::Review { root, title: format!("Review · {} · {}", side.agent, f.path), files: vec![f.path.clone()] };
            }
            _ => {}
        }
        ViewResult::None
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let c = self.col;
        self.scroll[c] = (self.scroll[c] as isize + delta).max(0) as usize;
    }

    pub fn click(&mut self, x: u16, y: u16) -> ViewResult {
        let col = if x >= self.area.x + self.area.width / 2 { 1 } else { 0 };
        self.col = col;
        let line = self.scroll[col] + y.saturating_sub(self.area.y) as usize;
        if let Some(Some(fi)) = self.line_files[col].get(line) {
            let again = self.sel[col] == *fi;
            self.sel[col] = *fi;
            if again {
                return self.handle_key(KeyEvent::from(KeyCode::Enter));
            }
        }
        ViewResult::None
    }

    pub fn status(&self) -> &'static str {
        "←→/Tab switch agent · ↑↓ select file · Enter review that file · ⇄ = changed by both · Esc close"
    }

    fn lines(side: &CompareSide, width: usize) -> Vec<(String, Style, Option<usize>)> {
        let dim = Style::default().fg(theme::DIM());
        let head = Style::default().fg(theme::FG()).add_modifier(Modifier::BOLD);
        let mut out = vec![(side.agent.clone(), head.fg(theme::BORDER_FOCUS()), None), (side.where_.clone(), dim, None), (String::new(), dim, None)];
        out.push(("Last prompt".into(), head, None));
        let prompt = side.prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        if prompt.is_empty() {
            out.push(("  (none recorded)".into(), dim, None));
        } else {
            let chars: Vec<char> = prompt.chars().collect();
            let w = width.saturating_sub(2).max(10);
            for (i, chunk) in chars.chunks(w).enumerate() {
                if i == 4 {
                    out.push(("  …".into(), dim, None));
                    break;
                }
                out.push((format!("  {}", chunk.iter().collect::<String>()), Style::default().fg(theme::FG()), None));
            }
        }
        out.push((String::new(), dim, None));
        let (add, del) = side.files.iter().filter_map(|f| f.counts).fold((0, 0), |(a, d), (fa, fd)| (a + fa, d + fd));
        out.push((format!("Files changed ({})  +{add} −{del}", side.files.len()), head, None));
        if let Some(note) = &side.note {
            out.push((format!("  {note}"), dim, None));
        }
        for (i, f) in side.files.iter().enumerate() {
            let counts = match f.counts {
                Some((a, d)) => format!("+{a} −{d}"),
                None if side.repo_root.is_none() => "?".into(),
                None => "binary".into(),
            };
            let mark = if f.shared { "⇄" } else { " " };
            let style = Style::default().fg(if f.shared { theme::ACCENT() } else { theme::FG() });
            let name_w = width.saturating_sub(counts.chars().count() + 6);
            let name: String = if f.path.chars().count() > name_w {
                let skip = f.path.chars().count() - name_w.saturating_sub(1);
                format!("…{}", f.path.chars().skip(skip).collect::<String>())
            } else {
                f.path.clone()
            };
            out.push((format!("  {mark} {name:<name_w$} {counts}"), style, Some(i)));
        }
        out.push((String::new(), dim, None));
        out.push((format!("Commands run ({})", side.commands.len()), head, None));
        if side.commands.is_empty() {
            out.push(("  (none recorded)".into(), dim, None));
        }
        let skip = side.commands.len().saturating_sub(20);
        if skip > 0 {
            out.push((format!("  … {skip} earlier"), dim, None));
        }
        for c in side.commands.iter().skip(skip) {
            out.push((format!("  $ {}", c.split_whitespace().collect::<Vec<_>>().join(" ")), Style::default().fg(theme::FG()), None));
        }
        out
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.area = area;
        if area.width < 20 || area.height == 0 {
            return;
        }
        let half = area.width / 2;
        for col in 0..2 {
            let x0 = area.x + if col == 0 { 0 } else { half + 1 };
            let w = if col == 0 { half } else { area.width - half - 1 } as usize;
            let lines = Self::lines(&self.sides[col], w.saturating_sub(1));
            let sel_line = lines.iter().position(|l| l.2 == Some(self.sel[col]));
            let h = area.height as usize;
            if let Some(sl) = sel_line {
                if sl < self.scroll[col] {
                    self.scroll[col] = sl;
                } else if sl >= self.scroll[col] + h {
                    self.scroll[col] = sl + 1 - h;
                }
            }
            self.scroll[col] = self.scroll[col].min(lines.len().saturating_sub(1));
            for (i, (text, style, file)) in lines.iter().enumerate().skip(self.scroll[col]).take(h) {
                let y = area.y + (i - self.scroll[col]) as u16;
                let mut style = *style;
                if file.is_some() && *file == Some(self.sel[col]) {
                    let bg = if col == self.col { theme::SELECT() } else { theme::SELECT_DIM() };
                    buf.set_style(Rect::new(x0, y, w as u16, 1), Style::default().bg(bg));
                    style = style.bg(bg);
                }
                buf.set_stringn(x0 + 1, y, text, w.saturating_sub(1), style);
            }
            self.line_files[col] = lines.into_iter().map(|l| l.2).collect();
        }
        for y in area.y..area.y + area.height {
            buf.set_string(area.x + half, y, "│", Style::default().fg(theme::BORDER()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(agent: &str, files: &[&str]) -> CompareSide {
        CompareSide {
            agent: agent.into(),
            repo_root: None,
            where_: String::new(),
            files: files.iter().map(|p| CompareFile { path: p.to_string(), counts: Some((1, 0)), shared: false }).collect(),
            commands: Vec::new(),
            prompt: String::new(),
            note: None,
        }
    }

    #[test]
    fn shared_files_marked_on_both_sides() {
        let mut sides = [side("a", &["x.rs", "y.rs"]), side("b", &["y.rs", "z.rs"])];
        mark_shared(&mut sides);
        let shared = |s: &CompareSide| s.files.iter().filter(|f| f.shared).map(|f| f.path.clone()).collect::<Vec<_>>();
        assert_eq!(shared(&sides[0]), ["y.rs"]);
        assert_eq!(shared(&sides[1]), ["y.rs"]);
    }

    fn group(reviewed: &[bool]) -> ChangeGroup {
        ChangeGroup {
            agent: "claude".into(),
            repo_root: Some(PathBuf::from("/r")),
            worktree: None,
            files: reviewed
                .iter()
                .enumerate()
                .map(|(i, &r)| ChangedFile {
                    abs: PathBuf::from(format!("/r/f{i}")),
                    repo_rel: format!("f{i}"),
                    key: format!("f{i}"),
                    state: Some(FileState::Modified),
                    created: false,
                    version: Version::default(),
                    reviewed: r,
                })
                .collect(),
        }
    }

    #[test]
    fn changes_view_keys() {
        let mut v = AgentChangesView::new(vec![group(&[true, false, false])]);
        // First file row is selected initially.
        assert!(matches!(v.handle_key(KeyEvent::from(KeyCode::Char('r'))), ViewResult::Review { files, .. } if files == ["f0"]));
        match v.handle_key(KeyEvent::from(KeyCode::Char('R'))) {
            ViewResult::Review { files, .. } => assert_eq!(files, ["f1", "f2"]),
            _ => panic!("expected review"),
        }
        v.handle_key(KeyEvent::from(KeyCode::Down));
        assert!(matches!(v.handle_key(KeyEvent::from(KeyCode::Char('v'))), ViewResult::ToggleReviewed { path, reviewed: false, .. } if path == "f1"));
        // Selection survives a data refresh.
        v.set_groups(vec![group(&[true, true, false])]);
        assert!(matches!(v.handle_key(KeyEvent::from(KeyCode::Char('v'))), ViewResult::ToggleReviewed { path, reviewed: true, .. } if path == "f1"));
        assert_eq!(v.groups[0].unreviewed(), 1);
    }
}
