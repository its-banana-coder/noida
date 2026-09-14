//! The find / replace widget shown over the editor (Ctrl+F, Ctrl+H).

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Clear, Widget};

use crate::editor::{Doc, SearchOpts};
use crate::theme;

pub struct FindBar {
    pub query: String,
    pub replace: String,
    pub opts: SearchOpts,
    pub show_replace: bool,
    on_replace: bool,
}

pub enum FindResult {
    None,
    Close,
    Changed,
    Next,
    Prev,
    ReplaceOne,
    ReplaceAll,
    SelectAll,
}

impl FindBar {
    pub fn new(query: String, opts: SearchOpts, show_replace: bool) -> Self {
        Self { query, replace: String::new(), opts, show_replace, on_replace: false }
    }

    pub fn focus_replace(&mut self) {
        self.show_replace = true;
        self.on_replace = true;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> FindResult {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let field = if self.on_replace { &mut self.replace } else { &mut self.query };
        match key.code {
            KeyCode::Esc => FindResult::Close,
            KeyCode::Char('c') if alt => {
                self.opts.case = !self.opts.case;
                FindResult::Changed
            }
            KeyCode::Char('w') if alt => {
                self.opts.word = !self.opts.word;
                FindResult::Changed
            }
            KeyCode::Char('r') if alt => {
                self.opts.regex = !self.opts.regex;
                FindResult::Changed
            }
            KeyCode::Char('l') if alt => {
                self.opts.in_selection = !self.opts.in_selection;
                FindResult::Changed
            }
            KeyCode::Char('a') if alt => FindResult::ReplaceAll,
            KeyCode::Enter if alt => FindResult::SelectAll,
            KeyCode::Char('h') if ctrl => {
                self.focus_replace();
                FindResult::None
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.show_replace = true;
                self.on_replace = !self.on_replace;
                FindResult::None
            }
            KeyCode::Enter if self.on_replace => FindResult::ReplaceOne,
            KeyCode::Enter | KeyCode::Down | KeyCode::F(3) if !key.modifiers.contains(KeyModifiers::SHIFT) => FindResult::Next,
            KeyCode::Up | KeyCode::F(3) | KeyCode::Enter => FindResult::Prev,
            KeyCode::Backspace => {
                field.pop();
                if self.on_replace { FindResult::None } else { FindResult::Changed }
            }
            KeyCode::Char('u') if ctrl => {
                field.clear();
                if self.on_replace { FindResult::None } else { FindResult::Changed }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                field.push(c);
                if self.on_replace { FindResult::None } else { FindResult::Changed }
            }
            _ => FindResult::None,
        }
    }

    pub fn paste(&mut self, text: &str) {
        let field = if self.on_replace { &mut self.replace } else { &mut self.query };
        field.push_str(text.lines().next().unwrap_or(""));
    }

    /// Draws at the top-right of the editor; returns the cursor position.
    pub fn render(&self, area: Rect, buf: &mut Buffer, doc: Option<&Doc>) -> Option<(u16, u16)> {
        let width = area.width.min(64);
        if width < 30 || area.height < 3 {
            return None;
        }
        let rows = if self.show_replace { 2 } else { 1 };
        let rect = Rect::new(area.x + area.width - width, area.y, width, rows);
        Clear.render(rect, buf);
        let bg = Style::default().bg(theme::STATUS_BG()).fg(theme::FG());
        buf.set_style(rect, bg);

        let search = doc.and_then(|d| d.search.as_ref());
        let count = match search {
            Some(s) if s.error.is_some() => "bad regex".to_string(),
            Some(s) if s.matches.is_empty() && !self.query.is_empty() => "no results".to_string(),
            Some(s) => match s.current {
                Some(i) => format!("{}/{}", i + 1, s.matches.len()),
                None => format!("{}", s.matches.len()),
            },
            None => String::new(),
        };
        let toggles = [("Aa", self.opts.case), ("W", self.opts.word), (".*", self.opts.regex), ("⊂", self.opts.in_selection)];
        let toggles_w: u16 = toggles.iter().map(|(t, _)| t.chars().count() as u16 + 1).sum();
        let right_w = toggles_w + count.chars().count() as u16 + 2;
        let field_w = width.saturating_sub(right_w + 3) as usize;

        let marker = |on: bool| if on { "›" } else { " " };
        let q_tail = tail(&self.query, field_w);
        buf.set_string(rect.x, rect.y, marker(!self.on_replace), Style::default().fg(theme::ACCENT()).bg(theme::STATUS_BG()));
        buf.set_string(rect.x + 1, rect.y, &q_tail, bg);
        if self.query.is_empty() {
            buf.set_string(rect.x + 1, rect.y, "find", Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()));
        }
        let mut x = rect.x + width - right_w;
        for (t, on) in toggles {
            let style = if on {
                Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::DIM()).bg(theme::STATUS_BG())
            };
            buf.set_string(x, rect.y, t, style);
            x += t.chars().count() as u16 + 1;
        }
        let count_style = if count == "no results" || count == "bad regex" { theme::ERROR() } else { theme::DIM() };
        buf.set_string(x + 1, rect.y, &count, Style::default().fg(count_style).bg(theme::STATUS_BG()));

        let mut cursor = (!self.on_replace).then(|| (rect.x + 1 + q_tail.chars().count() as u16, rect.y));
        if self.show_replace {
            let r_tail = tail(&self.replace, field_w);
            buf.set_string(rect.x, rect.y + 1, marker(self.on_replace), Style::default().fg(theme::ACCENT()).bg(theme::STATUS_BG()));
            buf.set_string(rect.x + 1, rect.y + 1, &r_tail, bg);
            if self.replace.is_empty() {
                buf.set_string(rect.x + 1, rect.y + 1, "replace", Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()));
            }
            let hint = "⏎ one · Alt+a all";
            buf.set_string(rect.x + width - hint.chars().count() as u16 - 1, rect.y + 1, hint, Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()));
            if self.on_replace {
                cursor = Some((rect.x + 1 + r_tail.chars().count() as u16, rect.y + 1));
            }
        }
        cursor
    }
}

fn tail(s: &str, width: usize) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(width)).collect()
}
