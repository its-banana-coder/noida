//! Generic fuzzy picker used by quick open, the command palette, symbols,
//! references, problems, branches and history.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Clear, Widget};

use crate::actions::Action;
use crate::theme;

#[derive(Clone, Debug)]
pub enum Target {
    File { path: PathBuf, line: Option<usize>, col: Option<usize> },
    Action(Action),
    Branch(String),
    /// A commit hash, limited to `paths` (file history) unless empty.
    Commit { hash: String, paths: Vec<String> },
    History(usize),
    AgentTab(usize),
    CodeAction(usize),
    Resume(crate::actions::AgentKind, String),
    Transcript(crate::actions::AgentKind, String, PathBuf, String),
    /// A command to start in its own terminal tab.
    Run(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub hint: String,
    pub target: Target,
}

impl Item {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, target: Target) -> Self {
        Self { label: label.into(), detail: detail.into(), hint: String::new(), target }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// Items are regenerated from the file index on every query change.
    Files,
    Static,
}

pub enum Outcome {
    None,
    Cancel,
    Accept(Target),
    QueryChanged,
}

pub struct Picker {
    pub title: String,
    pub kind: Kind,
    pub query: String,
    items: Vec<Item>,
    shown: Vec<usize>,
    pub selected: usize,
    /// Keep the given order instead of ranking by score (e.g. references).
    pub keep_order: bool,
}

impl Picker {
    pub fn new(title: impl Into<String>, kind: Kind, items: Vec<Item>) -> Self {
        let mut p = Self {
            title: title.into(),
            kind,
            query: String::new(),
            items,
            shown: Vec::new(),
            selected: 0,
            keep_order: false,
        };
        p.refilter();
        p
    }

    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self.refilter();
        self
    }

    pub fn ordered(mut self) -> Self {
        self.keep_order = true;
        self.refilter();
        self
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.refilter();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        if self.kind == Kind::Files || q.is_empty() {
            self.shown = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(i64, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| {
                    let label = fuzzy_score(&q, &it.label).map(|s| s + 30);
                    let detail = fuzzy_score(&q, &format!("{} {}", it.label, it.detail));
                    label.max(detail).map(|s| (s, i))
                })
                .collect();
            if !self.keep_order {
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            }
            self.shown = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = self.selected.min(self.shown.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&Item> {
        self.shown.get(self.selected).map(|&i| &self.items[i])
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.shown.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => {
                return match self.current() {
                    Some(it) => Outcome::Accept(it.target.clone()),
                    None => Outcome::Cancel,
                };
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('n') if ctrl => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(last),
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                self.refilter();
                return Outcome::QueryChanged;
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.selected = 0;
                self.refilter();
                return Outcome::QueryChanged;
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.selected = 0;
                self.refilter();
                return Outcome::QueryChanged;
            }
            _ => {}
        }
        Outcome::None
    }

    pub fn paste(&mut self, text: &str) {
        self.query.push_str(text.trim());
        self.selected = 0;
        self.refilter();
    }

    /// Returns the cursor position.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> (u16, u16) {
        let w = (area.width * 3 / 5).clamp(50.min(area.width), 110.min(area.width));
        let h = (self.shown.len() as u16 + 3).clamp(4, (area.height * 3 / 5).max(4));
        let popup = Rect::new(area.x + (area.width - w) / 2, area.y + area.height / 8, w, h.min(area.height));
        Clear.render(popup, buf);
        buf.set_style(popup, Style::default().bg(theme::STATUS_BG()));
        let count = format!(" {} ", self.shown.len());
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::BORDER_FOCUS()))
            .title(format!(" {} ", self.title))
            .title_bottom(ratatui::text::Line::from(count).right_aligned())
            .render(popup, buf);
        let body = Rect::new(popup.x + 1, popup.y + 1, popup.width.saturating_sub(2), popup.height.saturating_sub(2));
        if body.width < 4 || body.height == 0 {
            return (popup.x, popup.y);
        }
        buf.set_stringn(body.x, body.y, format!("› {}", self.query), body.width as usize, Style::default().fg(theme::FG()));
        let list_h = body.height.saturating_sub(1) as usize;
        let offset = self.selected.saturating_sub(list_h.saturating_sub(1));
        for (row, &i) in self.shown.iter().skip(offset).take(list_h).enumerate() {
            let item = &self.items[i];
            let y = body.y + 1 + row as u16;
            let selected = offset + row == self.selected;
            let base = if selected {
                Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS())
            } else {
                Style::default().fg(theme::FG())
            };
            if selected {
                buf.set_style(Rect::new(body.x, y, body.width, 1), base);
            }
            let hint_w = item.hint.chars().count() as u16;
            let text_end = body.x + body.width.saturating_sub(hint_w + 1);
            let (x, _) = buf.set_stringn(body.x + 1, y, &item.label, (text_end.saturating_sub(body.x + 1)) as usize, base.add_modifier(Modifier::BOLD));
            if !item.detail.is_empty() && x + 1 < text_end {
                let dim = if selected { base } else { Style::default().fg(theme::DIM()) };
                buf.set_stringn(x + 1, y, &item.detail, (text_end - x - 1) as usize, dim);
            }
            if hint_w > 0 && hint_w + 2 < body.width {
                let dim = if selected { base } else { Style::default().fg(theme::DIM()) };
                buf.set_string(body.x + body.width - hint_w, y, &item.hint, dim);
            }
        }
        if self.shown.is_empty() && body.height > 1 {
            buf.set_string(body.x + 1, body.y + 1, "no matches", Style::default().fg(theme::DIM()));
        }
        (body.x + 2 + self.query.chars().count() as u16, body.y)
    }
}

/// Subsequence fuzzy match with bonuses for word starts and consecutive runs.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let cand = candidate.to_lowercase();
    let bytes = cand.as_bytes();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for qc in query.bytes() {
        let found = bytes.get(pos..)?.iter().position(|&b| b == qc)? + pos;
        score += 10;
        if last == Some(found.wrapping_sub(1)) {
            score += 15;
        }
        if found == 0 || matches!(bytes[found - 1], b'/' | b'_' | b'-' | b'.' | b' ' | b':') {
            score += 20;
        }
        last = Some(found);
        pos = found + 1;
    }
    let name_start = cand.rfind('/').map_or(0, |i| i + 1);
    if cand[name_start..].contains(query) {
        score += 60;
    }
    if last.is_some_and(|l| l >= name_start) {
        score += 10;
    }
    Some(score - candidate.len() as i64 / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_filename() {
        let a = fuzzy_score("app", "src/app.rs").unwrap();
        let b = fuzzy_score("app", "a/p/p/other.rs").unwrap();
        assert!(a > b);
        assert!(fuzzy_score("xyz", "src/app.rs").is_none());
    }

    #[test]
    fn filters_static_items() {
        let items = vec![
            Item::new("Open File", "", Target::Action(Action::QuickOpen)),
            Item::new("Git: Review Changes", "", Target::Action(Action::ReviewChanges)),
        ];
        let mut p = Picker::new("Commands", Kind::Static, items);
        for c in "review".chars() {
            p.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(p.len(), 1);
        assert_eq!(p.current().unwrap().label, "Git: Review Changes");
    }
}
