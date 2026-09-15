//! Full-pane views shown in place of the editor: change review (diffs with
//! accept/reject per hunk, staged changes, read-only commits), agent activity
//! timelines and search results.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::activity::{EntryKind, Turn, clock};
use crate::git::{FileDiff, HunkOp, Repo};
use crate::theme;

pub enum ViewResult {
    None,
    Close,
    Open(PathBuf, usize),
    OpenAt(PathBuf, usize, usize),
    Message(String, bool),
    /// Files on disk changed (reload docs, refresh git).
    Changed(String),
    ReviewTurn(usize),
    /// Resume a past conversation in a new agent tab.
    Resume(crate::actions::AgentKind, String),
    /// Review the unstaged diff of `files` (repo-relative) in the repo at `root`.
    Review { root: PathBuf, title: String, files: Vec<String> },
    ToggleReviewed { agent: String, path: String, version: crate::changes::Version, reviewed: bool },
}

// ---------------------------------------------------------------- review --

enum Row {
    Meta(usize),
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    Blank,
}

/// What a `ReviewView` shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewMode {
    /// Working tree changes: accept (stage) or reject (revert).
    Unstaged,
    /// The index: unstage hunks or files.
    Staged,
    /// A read-only commit, limited to `paths` unless empty.
    Commit { hash: String, paths: Vec<String> },
}

pub struct ReviewView {
    title: String,
    repo: Repo,
    mode: ReviewMode,
    only: Option<Vec<String>>,
    /// Commit metadata shown above the diff (commit mode only).
    meta: Vec<String>,
    files: Vec<FileDiff>,
    rows: Vec<Row>,
    /// Flat list of (file, hunk-or-file-level) selection targets.
    targets: Vec<(usize, Option<usize>)>,
    selected: usize,
    scroll: usize,
    height: usize,
    confirm: Option<(char, Instant)>,
    area: Rect,
}

impl ReviewView {
    pub fn new(title: String, repo: Repo, only: Option<Vec<String>>) -> Result<Self, String> {
        Self::with_mode(title, repo, ReviewMode::Unstaged, only)
    }

    /// Read-only view of a commit's diff (limited to `paths` unless empty).
    pub fn commit(title: String, repo: Repo, hash: String, paths: Vec<String>) -> Result<Self, String> {
        Self::with_mode(title, repo, ReviewMode::Commit { hash, paths }, None)
    }

    fn with_mode(title: String, repo: Repo, mode: ReviewMode, only: Option<Vec<String>>) -> Result<Self, String> {
        let mut v = Self {
            title,
            repo,
            mode,
            only,
            meta: Vec::new(),
            files: Vec::new(),
            rows: Vec::new(),
            targets: Vec::new(),
            selected: 0,
            scroll: 0,
            height: 20,
            confirm: None,
            area: Rect::default(),
        };
        v.reload()?;
        Ok(v)
    }

    pub fn title(&self) -> String {
        match self.mode {
            ReviewMode::Unstaged => format!("{} · Unstaged", self.title),
            ReviewMode::Staged => format!("{} · Staged", self.title),
            ReviewMode::Commit { .. } => self.title.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let mut files = match &self.mode {
            ReviewMode::Unstaged => self.repo.diff(),
            ReviewMode::Staged => self.repo.diff_staged(),
            ReviewMode::Commit { hash, paths } => self.repo.show(hash, paths).map(|s| {
                self.meta = s.meta;
                s.files
            }),
        }
        .map_err(|e| e.to_string())?;
        if let Some(only) = &self.only {
            files.retain(|f| only.iter().any(|o| o == &f.path));
        }
        self.files = files;
        self.rows.clear();
        self.targets.clear();
        if !self.meta.is_empty() {
            self.rows.extend((0..self.meta.len()).map(Row::Meta));
            self.rows.push(Row::Blank);
        }
        for (fi, f) in self.files.iter().enumerate() {
            self.rows.push(Row::File(fi));
            if f.hunks.is_empty() {
                self.targets.push((fi, None));
            }
            for (hi, h) in f.hunks.iter().enumerate() {
                self.targets.push((fi, Some(hi)));
                self.rows.push(Row::Hunk(fi, hi));
                for li in 0..h.lines.len() {
                    self.rows.push(Row::Line(fi, hi, li));
                }
            }
            self.rows.push(Row::Blank);
        }
        self.selected = self.selected.min(self.targets.len().saturating_sub(1));
        Ok(())
    }

    fn target_row(&self) -> usize {
        let Some(&(fi, hi)) = self.targets.get(self.selected) else { return 0 };
        self.rows
            .iter()
            .position(|r| match (r, hi) {
                (Row::Hunk(f, h), Some(sel)) => *f == fi && *h == sel,
                (Row::File(f), None) => *f == fi,
                _ => false,
            })
            .unwrap_or(0)
    }

    fn reveal(&mut self) {
        let row = self.target_row();
        if row < self.scroll + 1 || row + 3 > self.scroll + self.height {
            self.scroll = row.saturating_sub(self.height / 4);
        }
    }

    fn toggle_staged(&mut self) -> ViewResult {
        let (mode, label) = match self.mode {
            ReviewMode::Unstaged => (ReviewMode::Staged, "staged"),
            ReviewMode::Staged => (ReviewMode::Unstaged, "unstaged"),
            ReviewMode::Commit { .. } => return self.wrong_mode(),
        };
        self.mode = mode;
        self.selected = 0;
        self.scroll = 0;
        self.confirm = None;
        match self.reload() {
            Ok(()) => ViewResult::Message(format!("showing {label} changes"), false),
            Err(e) => ViewResult::Message(e, true),
        }
    }

    /// Hint for keys that don't apply in the current mode.
    fn wrong_mode(&self) -> ViewResult {
        let hint = match self.mode {
            ReviewMode::Unstaged => "u/U unstage in the staged view: press s to switch",
            ReviewMode::Staged => "staged view: u unstage hunk · U unstage file · s back to unstaged",
            ReviewMode::Commit { .. } => "commit view is read-only: Enter opens the file · Esc closes",
        };
        ViewResult::Message(hint.into(), false)
    }

    fn apply(&mut self, op: HunkOp, whole_file: bool) -> ViewResult {
        let allowed = match self.mode {
            ReviewMode::Unstaged => op != HunkOp::Unstage,
            ReviewMode::Staged => op == HunkOp::Unstage,
            ReviewMode::Commit { .. } => false,
        };
        if !allowed {
            return self.wrong_mode();
        }
        let Some(&(fi, hi)) = self.targets.get(self.selected) else { return ViewResult::None };
        let file = self.files[fi].clone();
        let result = match (whole_file, hi) {
            (true, _) | (false, None) => match op {
                HunkOp::Stage => self.repo.stage_file(&file.path),
                HunkOp::Revert => self.repo.revert_file(&file),
                HunkOp::Unstage => self.repo.unstage_file(&file.path),
            },
            (false, Some(h)) => self.repo.apply_hunk(&file, h, op),
        };
        if let Err(e) = result {
            return ViewResult::Message(format!("{e:#}"), true);
        }
        let _ = self.reload();
        let verb = match op {
            HunkOp::Stage => "accepted",
            HunkOp::Revert => "rejected",
            HunkOp::Unstage => "unstaged",
        };
        let what = if whole_file || hi.is_none() { file.path.clone() } else { format!("hunk in {}", file.path) };
        ViewResult::Changed(format!("{verb} {what}"))
    }

    /// Destructive actions need a second press within 2 seconds.
    fn confirmed(&mut self, key: char) -> bool {
        let ok = self.confirm.is_some_and(|(k, t)| k == key && t.elapsed() < Duration::from_secs(2));
        self.confirm = if ok { None } else { Some((key, Instant::now())) };
        ok
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ViewResult {
        let n = self.targets.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ViewResult::Close,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('n') => {
                self.selected = (self.selected + 1).min(n.saturating_sub(1));
                self.reveal();
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('p') => {
                self.selected = self.selected.saturating_sub(1);
                self.reveal();
            }
            KeyCode::Tab => {
                if let Some(&(fi, _)) = self.targets.get(self.selected) {
                    if let Some(i) = self.targets.iter().position(|&(f, _)| f > fi) {
                        self.selected = i;
                        self.reveal();
                    }
                }
            }
            KeyCode::PageDown => self.scroll = (self.scroll + self.height.saturating_sub(2)).min(self.rows.len().saturating_sub(1)),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.height.saturating_sub(2)),
            KeyCode::Char('a') => return self.apply(HunkOp::Stage, false),
            KeyCode::Char('A') => return self.apply(HunkOp::Stage, true),
            KeyCode::Char('u') => return self.apply(HunkOp::Unstage, false),
            KeyCode::Char('U') => return self.apply(HunkOp::Unstage, true),
            KeyCode::Char('s') => return self.toggle_staged(),
            KeyCode::Char('x' | 'X') if self.mode != ReviewMode::Unstaged => return self.wrong_mode(),
            KeyCode::Char('x') => {
                if !self.confirmed('x') {
                    return ViewResult::Message("press x again to discard this hunk from disk".into(), true);
                }
                return self.apply(HunkOp::Revert, false);
            }
            KeyCode::Char('X') => {
                if !self.confirmed('X') {
                    return ViewResult::Message("press X again to discard ALL changes in this file".into(), true);
                }
                return self.apply(HunkOp::Revert, true);
            }
            KeyCode::Char('r') => {
                return match self.reload() {
                    Ok(()) => ViewResult::Message("refreshed".into(), false),
                    Err(e) => ViewResult::Message(e, true),
                };
            }
            KeyCode::Enter => {
                if let Some(&(fi, hi)) = self.targets.get(self.selected) {
                    let line = hi.map_or(1, |h| self.files[fi].hunks[h].new_start.max(1));
                    let path = self.repo.root.join(&self.files[fi].path);
                    if matches!(self.mode, ReviewMode::Commit { .. }) && !path.is_file() {
                        return ViewResult::Message(format!("{} no longer exists in the working tree", self.files[fi].path), true);
                    }
                    return ViewResult::Open(path, line);
                }
            }
            _ => {}
        }
        ViewResult::None
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).clamp(0, self.rows.len().saturating_sub(1) as isize) as usize;
    }

    pub fn click(&mut self, y: u16) {
        let row = self.scroll + y.saturating_sub(self.area.y) as usize;
        let target = self.rows.get(row).and_then(|r| match r {
            Row::File(f) => self.targets.iter().position(|&(tf, _)| tf == *f),
            Row::Hunk(f, h) | Row::Line(f, h, _) => self.targets.iter().position(|&t| t == (*f, Some(*h))),
            Row::Meta(_) | Row::Blank => None,
        });
        if let Some(t) = target {
            self.selected = t;
        }
    }

    pub fn status(&self) -> String {
        let counts = format!("{} files · {} hunks", self.files.len(), self.targets.len());
        let keys = match self.mode {
            ReviewMode::Unstaged => "a accept · x reject · A/X whole file · s staged · Enter open · Tab next file · r refresh · Esc close",
            ReviewMode::Staged => "u unstage · U unstage file · s unstaged · Enter open · Tab next file · r refresh · Esc close",
            ReviewMode::Commit { .. } => "read-only · Enter open file · Tab next file · Esc close",
        };
        format!("{counts}   {keys}")
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.area = area;
        self.height = area.height as usize;
        if self.files.is_empty() && self.meta.is_empty() {
            let msg = match self.mode {
                ReviewMode::Unstaged => "No unstaged changes. Everything is accepted. Press s for staged changes.",
                ReviewMode::Staged => "No staged changes. Press s for unstaged changes.",
                ReviewMode::Commit { .. } => "This commit has no changes.",
            };
            buf.set_stringn(area.x + 2, area.y + 1, msg, area.width.saturating_sub(2) as usize, Style::default().fg(theme::DIM()));
            return;
        }
        let sel = self.targets.get(self.selected).copied();
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(self.height) {
            let y = area.y + (i - self.scroll) as u16;
            let full = Rect::new(area.x, y, area.width, 1);
            let w = area.width as usize;
            match row {
                Row::Meta(mi) => {
                    let text = &self.meta[*mi];
                    let style = if *mi == 0 {
                        Style::default().fg(theme::ACCENT()).add_modifier(Modifier::BOLD)
                    } else if text.starts_with("Author:") || text.starts_with("Date:") {
                        Style::default().fg(theme::DIM())
                    } else {
                        Style::default().fg(theme::FG())
                    };
                    buf.set_stringn(area.x + 1, y, text.replace('\t', "    "), w.saturating_sub(1), style);
                }
                Row::File(fi) => {
                    let f = &self.files[*fi];
                    let (add, del) = f.counts();
                    let selected = sel == Some((*fi, None));
                    let tag = match f.renamed_from() {
                        _ if f.untracked => "new".to_string(),
                        _ if f.binary => "binary".to_string(),
                        Some(from) => format!("renamed from {from}"),
                        None => String::new(),
                    };
                    let style = Style::default().fg(theme::FG()).bg(if selected { theme::SELECT() } else { theme::STATUS_BG() }).add_modifier(Modifier::BOLD);
                    buf.set_style(full, style);
                    let (x, _) = buf.set_stringn(area.x + 1, y, &f.path, w.saturating_sub(16), style);
                    buf.set_string(x + 2, y, format!("+{add}"), style.fg(theme::ADDED()));
                    buf.set_string(x + 4 + add.to_string().len() as u16, y, format!("-{del} {tag}"), style.fg(theme::ERROR()));
                }
                Row::Hunk(fi, hi) => {
                    let selected = sel == Some((*fi, Some(*hi)));
                    let h = &self.files[*fi].hunks[*hi];
                    let style = if selected {
                        Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS())
                    } else {
                        Style::default().fg(theme::DIR())
                    };
                    if selected {
                        buf.set_style(full, style);
                    }
                    buf.set_stringn(area.x + 1, y, &h.header, w.saturating_sub(1), style);
                }
                Row::Line(fi, hi, li) => {
                    let text = &self.files[*fi].hunks[*hi].lines[*li];
                    let selected = sel == Some((*fi, Some(*hi)));
                    let (fg, bg) = match text.chars().next() {
                        Some('+') => (theme::ADDED(), Some(theme::ADDED_BG())),
                        Some('-') => (theme::ERROR(), Some(theme::REMOVED_BG())),
                        _ => (theme::FG(), None),
                    };
                    if let Some(bg) = bg {
                        buf.set_style(full, Style::default().bg(bg));
                    }
                    let bar = if selected { "▌" } else { " " };
                    buf.set_string(area.x, y, bar, Style::default().fg(theme::BORDER_FOCUS()));
                    let display = text.replace('\t', "    ");
                    buf.set_stringn(area.x + 1, y, display, w.saturating_sub(1), Style::default().fg(fg));
                }
                Row::Blank => {}
            }
        }
    }
}

// -------------------------------------------------------------- activity --

pub struct ActivityView {
    pub title: String,
    turns: Vec<Turn>,
    rows: Vec<(String, Style, Option<String>, Option<usize>)>,
    selected: usize,
    scroll: usize,
    height: usize,
    root: PathBuf,
    area: Rect,
}

impl ActivityView {
    pub fn new(title: String, root: PathBuf, turns: Vec<Turn>) -> Self {
        let mut v = Self { title, turns: Vec::new(), rows: Vec::new(), selected: 0, scroll: 0, height: 20, root, area: Rect::default() };
        v.set_turns(turns);
        v
    }

    pub fn set_turns(&mut self, turns: Vec<Turn>) {
        self.turns = turns;
        self.rows.clear();
        let dim = Style::default().fg(theme::DIM());
        for (ti, t) in self.turns.iter().enumerate().rev() {
            let status = if t.ended.is_some() { "✓" } else { "⠿" };
            let head = format!("{status} {}  {}  {}", t.agent, clock(t.started), if t.prompt.is_empty() { "(no prompt)" } else { &t.prompt });
            self.rows.push((head, Style::default().fg(theme::FG()).add_modifier(Modifier::BOLD).bg(theme::STATUS_BG()), None, Some(ti)));
            let mut groups: Vec<(&str, Vec<String>, ratatui::style::Color)> = Vec::new();
            groups.push(("Modified", t.edited.iter().cloned().collect(), theme::ACCENT()));
            groups.push(("Created", t.created.iter().cloned().collect(), theme::ADDED()));
            groups.push(("Read", t.read.iter().cloned().collect(), theme::LINK()));
            for (label, files, color) in groups {
                if files.is_empty() {
                    continue;
                }
                self.rows.push((format!("  {label} ({})", files.len()), dim, None, Some(ti)));
                for f in files {
                    self.rows.push((format!("    {f}"), Style::default().fg(color), Some(f), Some(ti)));
                }
            }
            if !t.commands.is_empty() {
                self.rows.push((format!("  Commands ({})", t.commands.len()), dim, None, Some(ti)));
                for c in &t.commands {
                    self.rows.push((format!("    $ {c}"), Style::default().fg(theme::FG()), None, Some(ti)));
                }
            }
            self.rows.push(("  Timeline".into(), dim, None, Some(ti)));
            for e in &t.entries {
                let color = match e.kind {
                    EntryKind::Prompt => theme::BORDER_FOCUS(),
                    EntryKind::Edit => theme::ACCENT(),
                    EntryKind::Create => theme::ADDED(),
                    EntryKind::Permission => theme::ERROR(),
                    EntryKind::Finished => theme::ADDED(),
                    EntryKind::Command => theme::FG(),
                    _ => theme::DIM(),
                };
                let text: String = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
                self.rows.push((format!("    {}  {text}", clock(e.at)), Style::default().fg(color), e.path.clone(), Some(ti)));
            }
            self.rows.push((String::new(), dim, None, None));
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ViewResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ViewResult::Close,
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::PageDown => self.selected = (self.selected + self.height).min(self.rows.len().saturating_sub(1)),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(self.height),
            KeyCode::Enter => {
                if let Some((_, _, Some(path), _)) = self.rows.get(self.selected) {
                    return ViewResult::Open(self.root.join(path), 1);
                }
            }
            KeyCode::Char('r') => {
                if let Some((_, _, _, Some(ti))) = self.rows.get(self.selected) {
                    return ViewResult::ReviewTurn(*ti);
                }
            }
            _ => {}
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.height {
            self.scroll = self.selected + 1 - self.height;
        }
        ViewResult::None
    }

    pub fn turn(&self, i: usize) -> Option<&Turn> {
        self.turns.get(i)
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).clamp(0, self.rows.len().saturating_sub(1) as isize) as usize;
    }

    pub fn click(&mut self, y: u16) -> ViewResult {
        let row = self.scroll + y.saturating_sub(self.area.y) as usize;
        if row < self.rows.len() {
            let again = row == self.selected;
            self.selected = row;
            if again {
                return self.handle_key(KeyEvent::from(KeyCode::Enter));
            }
        }
        ViewResult::None
    }

    pub fn status(&self) -> &'static str {
        "↑↓ select · Enter open file · r review this turn's changes · Esc close"
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.area = area;
        self.height = area.height as usize;
        if self.rows.is_empty() {
            let msg = "No agent activity yet. Agents started by NOIDA report reads, edits and commands here.";
            buf.set_stringn(area.x + 2, area.y + 1, msg, area.width as usize, Style::default().fg(theme::DIM()));
            return;
        }
        for (i, (text, style, _, _)) in self.rows.iter().enumerate().skip(self.scroll).take(self.height) {
            let y = area.y + (i - self.scroll) as u16;
            let mut style = *style;
            if i == self.selected {
                style = style.bg(theme::SELECT());
                buf.set_style(Rect::new(area.x, y, area.width, 1), Style::default().bg(theme::SELECT()));
            } else if style.bg.is_some() {
                buf.set_style(Rect::new(area.x, y, area.width, 1), Style::default().bg(theme::STATUS_BG()));
            }
            buf.set_stringn(area.x + 1, y, text, area.width.saturating_sub(1) as usize, style);
        }
    }
}

// ------------------------------------------------------------ transcript --

/// Read-only rendering of a past Claude or Codex conversation.
pub struct TranscriptView {
    pub title: String,
    pub kind: crate::actions::AgentKind,
    pub id: String,
    markdown: String,
    cache: Option<(u16, Vec<crate::markdown::MdLine>)>,
    scroll: usize,
    height: usize,
}

impl TranscriptView {
    pub fn new(title: String, kind: crate::actions::AgentKind, id: String, markdown: String) -> Self {
        Self { title, kind, id, markdown, cache: None, scroll: 0, height: 20 }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ViewResult {
        let page = self.height.saturating_sub(2).max(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ViewResult::Close,
            KeyCode::Char('r') | KeyCode::Enter => return ViewResult::Resume(self.kind, self.id.clone()),
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll += page,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = usize::MAX / 2,
            _ => {}
        }
        ViewResult::None
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).max(0) as usize;
    }

    pub fn status(&self) -> &'static str {
        "↑↓ PgUp/PgDn scroll · r resume this conversation in a new tab · Esc close"
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height < 3 {
            return;
        }
        let body = Rect::new(area.x + 1, area.y, area.width - 2, area.height);
        if self.cache.as_ref().is_none_or(|(w, _)| *w != body.width) {
            self.cache = Some((body.width, crate::markdown::render(&self.markdown, body.width as usize)));
        }
        let lines = &self.cache.as_ref().unwrap().1;
        self.height = body.height as usize;
        self.scroll = self.scroll.min(lines.len().saturating_sub(self.height));
        for (row, line) in lines.iter().skip(self.scroll).take(self.height).enumerate() {
            let mut x = body.x;
            let y = body.y + row as u16;
            for (text, style) in line {
                let end = body.x + body.width;
                if x >= end {
                    break;
                }
                x = buf.set_stringn(x, y, text, (end - x) as usize, *style).0;
            }
        }
    }
}
