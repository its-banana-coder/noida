//! The top menu bar.
//!
//! A terminal IDE does not need a menu bar to be usable — the command palette
//! is faster once you know it exists. The bar is there for the moment before
//! that: someone opens NOIDA for the first time, has no idea what it can do,
//! and looks at the top of the window because that is where menus live.
//!
//! Every entry is an `Action`, the same ones the palette and keybindings use,
//! so a menu item cannot drift away from what the key does.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::actions::{Action, AgentKind};
use crate::theme;

pub struct Menu {
    pub title: &'static str,
    pub items: Vec<Entry>,
}

pub enum Entry {
    Item(Action),
    Separator,
}

#[derive(Default)]
pub struct MenuBar {
    /// Which menu is open, if any.
    pub open: Option<usize>,
    pub selected: usize,
    /// Screen position of each title, for the mouse: (x, width, index).
    titles: Vec<(u16, u16, usize)>,
    /// Where the open dropdown was drawn.
    drop: Rect,
}

pub fn menus() -> Vec<Menu> {
    use Action::*;
    vec![
        Menu {
            title: "File",
            items: vec![
                Entry::Item(QuickOpen),
                Entry::Item(NewFile),
                Entry::Item(NewFolder),
                Entry::Separator,
                Entry::Item(Save),
                Entry::Item(SaveAll),
                Entry::Item(CloseFile),
                Entry::Separator,
                Entry::Item(RecentFiles),
                Entry::Item(OpenSettings),
                Entry::Item(Quit),
            ],
        },
        Menu {
            title: "Edit",
            items: vec![
                Entry::Item(Find),
                Entry::Item(FindReplace),
                Entry::Item(SearchWorkspace),
                Entry::Item(ReplaceWorkspace),
                Entry::Separator,
                Entry::Item(ToggleComment),
                Entry::Item(AddCursorBelow),
                Entry::Item(SelectAllOccurrences),
                Entry::Item(FormatDocument),
            ],
        },
        Menu {
            title: "Go",
            items: vec![
                Entry::Item(GoToLine),
                Entry::Item(GoToSymbol),
                Entry::Item(GoToProjectSymbol),
                Entry::Item(GoToDefinition),
                Entry::Item(FindReferences),
                Entry::Separator,
                Entry::Item(JumpToRef),
                Entry::Item(GoBack),
                Entry::Item(RecentLocations),
            ],
        },
        Menu {
            title: "Run",
            items: vec![
                Entry::Item(RunProject),
                Entry::Item(RunTests),
                Entry::Separator,
                Entry::Item(Problems),
                Entry::Item(FixProblem),
            ],
        },
        Menu {
            title: "Agent",
            items: vec![
                Entry::Item(NewAgent(AgentKind::Claude)),
                Entry::Item(NewAgent(AgentKind::Codex)),
                Entry::Item(NewAgent(AgentKind::Shell)),
                Entry::Separator,
                Entry::Item(SendSelection),
                Entry::Item(AskMenu),
                Entry::Item(Ask(crate::actions::Ask::Review)),
                Entry::Separator,
                Entry::Item(AgentActivity),
                Entry::Item(AgentChanges),
                Entry::Item(Sessions),
                Entry::Item(BrowseHistory),
                Entry::Item(NewWorktreeAgent(AgentKind::Claude)),
            ],
        },
        Menu {
            title: "Git",
            items: vec![
                Entry::Item(ReviewChanges),
                Entry::Item(AcceptAllChanges),
                Entry::Item(Commit),
                Entry::Separator,
                Entry::Item(SwitchBranch),
                Entry::Item(NewBranch),
                Entry::Item(GitLog),
                Entry::Item(FileHistory),
            ],
        },
        Menu {
            title: "View",
            items: vec![
                Entry::Item(ToggleTree),
                Entry::Item(ToggleOutline),
                Entry::Item(SplitEditor),
                Entry::Item(ToggleSplit),
                Entry::Item(Zoom),
                Entry::Separator,
                Entry::Item(ToggleWordWrap),
                Entry::Item(ToggleMarkdownPreview),
                Entry::Item(ToggleTheme),
            ],
        },
        Menu {
            title: "Help",
            items: vec![Entry::Item(Keys), Entry::Item(CommandPalette)],
        },
    ]
}

impl MenuBar {
    /// The action under the cursor, if the highlighted row is one.
    fn action_at(&self, menus: &[Menu], menu: usize, row: usize) -> Option<Action> {
        match menus.get(menu)?.items.get(row)? {
            Entry::Item(a) => Some(*a),
            Entry::Separator => None,
        }
    }

    pub fn close(&mut self) {
        self.open = None;
        self.selected = 0;
    }

    pub fn toggle(&mut self) {
        match self.open {
            Some(_) => self.close(),
            None => {
                self.open = Some(0);
                self.selected = 0;
            }
        }
    }

    /// Move to the next selectable row, skipping separators.
    fn step(&mut self, menus: &[Menu], delta: isize) {
        let Some(m) = self.open else { return };
        let len = menus[m].items.len();
        if len == 0 {
            return;
        }
        for _ in 0..len {
            self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
            if matches!(menus[m].items.get(self.selected), Some(Entry::Item(_))) {
                return;
            }
        }
    }

    /// Returns an action when the user picked one; `None` otherwise.
    pub fn handle_key(&mut self, menus: &[Menu], key: ratatui::crossterm::event::KeyEvent) -> Option<Action> {
        use ratatui::crossterm::event::KeyCode;
        let m = self.open?;
        match key.code {
            KeyCode::Esc => self.close(),
            KeyCode::Left => {
                self.open = Some((m + menus.len() - 1) % menus.len());
                self.selected = 0;
                self.step(menus, 0);
                if !matches!(menus[self.open?].items.first(), Some(Entry::Item(_))) {
                    self.step(menus, 1);
                }
            }
            KeyCode::Right => {
                self.open = Some((m + 1) % menus.len());
                self.selected = 0;
                if !matches!(menus[self.open?].items.first(), Some(Entry::Item(_))) {
                    self.step(menus, 1);
                }
            }
            KeyCode::Down => self.step(menus, 1),
            KeyCode::Up => self.step(menus, -1),
            KeyCode::Enter => {
                let action = self.action_at(menus, m, self.selected);
                if action.is_some() {
                    self.close();
                }
                return action;
            }
            _ => {}
        }
        None
    }

    /// A click on the bar row: open, switch or close a menu.
    pub fn click_title(&mut self, x: u16) -> bool {
        let Some(&(_, _, idx)) = self.titles.iter().find(|(tx, w, _)| x >= *tx && x < *tx + *w) else {
            self.close();
            return false;
        };
        if self.open == Some(idx) {
            self.close();
        } else {
            self.open = Some(idx);
            self.selected = 0;
        }
        true
    }

    /// A click inside the open dropdown.
    pub fn click_item(&mut self, menus: &[Menu], x: u16, y: u16) -> Option<Action> {
        let m = self.open?;
        if x < self.drop.x || x >= self.drop.x + self.drop.width || y < self.drop.y || y >= self.drop.y + self.drop.height {
            return None;
        }
        let row = (y - self.drop.y) as usize;
        let action = self.action_at(menus, m, row);
        if action.is_some() {
            self.close();
        }
        action
    }


    pub fn render(&mut self, menus: &[Menu], area: Rect, buf: &mut Buffer) {
        let bar = Style::default().fg(theme::FG()).bg(theme::STATUS_BG());
        buf.set_style(area, bar);
        self.titles.clear();
        let mut x = area.x + 1;
        for (i, menu) in menus.iter().enumerate() {
            let label = format!(" {} ", menu.title);
            let w = label.chars().count() as u16;
            if x + w >= area.x + area.width {
                break;
            }
            let style = if self.open == Some(i) {
                Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD)
            } else {
                bar
            };
            buf.set_stringn(x, area.y, &label, w as usize, style);
            self.titles.push((x, w, i));
            x += w;
        }
        // A hint, while there is room for it.
        let hint = "F10 menu · Alt+x commands";
        let hx = area.x + area.width.saturating_sub(hint.len() as u16 + 1);
        if hx > x {
            buf.set_stringn(hx, area.y, hint, hint.len(), bar.fg(theme::DIM()));
        }
    }

    /// Draw the open dropdown over whatever is beneath it.
    pub fn render_dropdown(&mut self, menus: &[Menu], area: Rect, buf: &mut Buffer) {
        let Some(m) = self.open else {
            self.drop = Rect::default();
            return;
        };
        let menu = &menus[m];
        let rows: Vec<(String, String, bool)> = menu
            .items
            .iter()
            .map(|e| match e {
                Entry::Item(a) => {
                    let (label, key) = a.describe().unwrap_or_else(|| (format!("{a:?}"), ""));
                    (label, key.to_string(), true)
                }
                Entry::Separator => (String::new(), String::new(), false),
            })
            .collect();
        let width = rows.iter().map(|(l, k, _)| l.chars().count() + k.chars().count() + 6).max().unwrap_or(20).max(24) as u16;
        let x = self.titles.get(m).map(|t| t.0).unwrap_or(area.x).min(area.x + area.width.saturating_sub(width));
        let height = (rows.len() as u16).min(area.height.saturating_sub(area.y + 1));
        let rect = Rect::new(x, area.y + 1, width.min(area.width), height);
        self.drop = rect;

        let bg = Style::default().fg(theme::FG()).bg(theme::STATUS_BG());
        // Painting a style leaves whatever characters were underneath, so the
        // dropdown has to blank its own area or the panes show through it.
        let blank = " ".repeat(rect.width as usize);
        for y in rect.y..rect.y + rect.height {
            buf.set_stringn(rect.x, y, &blank, rect.width as usize, bg);
        }
        for (i, (label, key, is_item)) in rows.iter().enumerate().take(height as usize) {
            let y = rect.y + i as u16;
            if !is_item {
                let line = "─".repeat(rect.width.saturating_sub(2) as usize);
                buf.set_stringn(rect.x + 1, y, &line, rect.width as usize, bg.fg(theme::DIM()));
                continue;
            }
            let style = if i == self.selected {
                Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS())
            } else {
                bg
            };
            buf.set_style(Rect::new(rect.x, y, rect.width, 1), style);
            buf.set_stringn(rect.x + 1, y, label, rect.width.saturating_sub(2) as usize, style);
            if !key.is_empty() {
                let kx = rect.x + rect.width.saturating_sub(key.chars().count() as u16 + 1);
                if kx > rect.x + label.chars().count() as u16 + 2 {
                    buf.set_stringn(kx, y, key, key.len(), style.fg(theme::DIM()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_entry_is_a_real_action_with_a_label() {
        for menu in menus() {
            assert!(!menu.title.is_empty());
            for entry in &menu.items {
                if let Entry::Item(a) = entry {
                    let described = a.describe();
                    assert!(described.is_some(), "{a:?} in {} has no label", menu.title);
                    let (label, _) = described.unwrap();
                    assert!(!label.is_empty(), "{a:?} has an empty label");
                }
            }
        }
    }

    #[test]
    fn keyboard_navigation_skips_separators() {
        let menus = menus();
        let mut bar = MenuBar::default();
        assert_eq!(bar.open, None);
        bar.toggle();
        assert_eq!(bar.open, Some(0), "F10 opens the first menu");

        // Down from the first item must never land on a separator.
        let file = &menus[0];
        let sep_rows: Vec<usize> = file.items.iter().enumerate().filter(|(_, e)| matches!(e, Entry::Separator)).map(|(i, _)| i).collect();
        assert!(!sep_rows.is_empty(), "this menu has separators to skip");
        for _ in 0..file.items.len() + 2 {
            bar.step(&menus, 1);
            assert!(!sep_rows.contains(&bar.selected), "landed on a separator at row {}", bar.selected);
        }

        // Left and right move between menus and wrap.
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        bar.handle_key(&menus, KeyEvent::from(KeyCode::Right));
        assert_eq!(bar.open, Some(1));
        bar.handle_key(&menus, KeyEvent::from(KeyCode::Left));
        bar.handle_key(&menus, KeyEvent::from(KeyCode::Left));
        assert_eq!(bar.open, Some(menus.len() - 1), "left from the first wraps to the last");

        // Esc closes.
        bar.handle_key(&menus, KeyEvent::from(KeyCode::Esc));
        assert_eq!(bar.open, None);
    }

    #[test]
    fn enter_returns_the_action_and_closes() {
        let menus = menus();
        let mut bar = MenuBar::default();
        bar.toggle();
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let picked = bar.handle_key(&menus, KeyEvent::from(KeyCode::Enter));
        assert_eq!(picked, Some(Action::QuickOpen), "the first File entry");
        assert_eq!(bar.open, None, "picking closes the menu");

        // Enter on a separator picks nothing and leaves the menu open.
        let mut bar = MenuBar::default();
        bar.toggle();
        bar.selected = menus[0].items.iter().position(|e| matches!(e, Entry::Separator)).unwrap();
        assert_eq!(bar.handle_key(&menus, KeyEvent::from(KeyCode::Enter)), None);
        assert_eq!(bar.open, Some(0), "still open");
    }

    #[test]
    fn the_run_menu_offers_running_and_testing() {
        let menus = menus();
        let run = menus.iter().find(|m| m.title == "Run").expect("a Run menu");
        let actions: Vec<Action> = run.items.iter().filter_map(|e| match e {
            Entry::Item(a) => Some(*a),
            Entry::Separator => None,
        }).collect();
        assert!(actions.contains(&Action::RunProject), "Run: start the project");
        assert!(actions.contains(&Action::RunTests), "Run: the tests");
    }
}
