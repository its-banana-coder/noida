//! Layout and rendering.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};

use super::{App, Focus, Mode, PromptKind, Rects, View};
use crate::refs;
use crate::theme;

impl App {
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let [main, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        f.buffer_mut().set_style(area, Style::default().bg(theme::BG).fg(theme::FG));

        let (show_tree, show_editor, show_agent) = if self.zoom {
            (self.focus == Focus::Tree, self.focus == Focus::Editor, self.focus == Focus::Agent)
        } else {
            (self.show_tree, true, !self.agents.is_empty())
        };
        let mut constraints = Vec::new();
        if show_tree {
            constraints.push(if self.zoom { Constraint::Fill(1) } else { Constraint::Length(self.tree_width) });
        }
        if show_editor {
            constraints.push(Constraint::Fill(1));
        }
        if show_agent {
            constraints.push(if self.zoom { Constraint::Fill(1) } else { Constraint::Percentage(self.agent_pct) });
        }
        let chunks = Layout::horizontal(constraints).split(main);
        let mut it = chunks.iter().copied();
        let tree_block = if show_tree { it.next().unwrap() } else { Rect::default() };
        let editor_block = if show_editor { it.next().unwrap() } else { Rect::default() };
        let agent_area = if show_agent { it.next().unwrap() } else { Rect::default() };
        let agent_blocks = if self.split && agent_area.height > 6 {
            let [top, bottom] = Layout::vertical([Constraint::Percentage(self.split_pct), Constraint::Fill(1)]).areas(agent_area);
            [top, bottom]
        } else {
            [agent_area, Rect::default()]
        };

        self.rects = Rects {
            main,
            tree_block,
            tree: inner(tree_block),
            editor_block,
            editor: inner(editor_block),
            agent_area,
            agent_blocks,
            agents: [inner(agent_blocks[0]), inner(agent_blocks[1])],
            doc_tabs: Vec::new(),
            agent_tabs: [Vec::new(), Vec::new()],
        };

        let mut cursor = None;
        let buf = f.buffer_mut();

        if show_tree {
            pane(buf, tree_block, Line::from(" FILES "), self.focus == Focus::Tree);
            let open = self.doc().map(|d| d.path.clone());
            self.tree.render(self.rects.tree, buf, self.focus == Focus::Tree, open.as_deref(), &self.activity.touched);
        }

        if show_editor {
            cursor = self.draw_editor(buf, editor_block).or(cursor);
        }

        self.refs = [Vec::new(), Vec::new()];
        if show_agent {
            let total_refs = self.draw_agents(buf, &mut cursor);
            let _ = total_refs;
        }

        self.draw_status(buf, status);

        if let Mode::Picker(p) = &self.mode {
            cursor = Some(p.render(area, buf));
        } else if let Mode::Prompt { .. } = self.mode {
            cursor = None;
        }
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    fn draw_editor(&mut self, buf: &mut Buffer, block: Rect) -> Option<(u16, u16)> {
        let mut spans = vec![Span::raw(" ")];
        let mut x = block.x + 2;
        let active_style = Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD);
        if let Some(view) = &self.view {
            let title = match view {
                View::Review(v) => format!(" ± {} ", v.title),
                View::Activity(v) => format!(" ⏱ {} ", v.title),
            };
            x += title.chars().count() as u16 + 1;
            spans.push(Span::styled(title, active_style));
            spans.push(Span::raw(" "));
        }
        for (i, d) in self.docs.iter().enumerate() {
            let label = format!(" {}{} ", d.file_name(), if d.dirty { " ●" } else { "" });
            let w = label.chars().count() as u16;
            self.rects.doc_tabs.push((x, x + w, i));
            x += w + 1;
            let style = if i == self.active_doc && self.view.is_none() { active_style } else { Style::default().fg(theme::DIM) };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        if self.docs.is_empty() && self.view.is_none() {
            spans.push(Span::raw("EDITOR "));
        }
        pane(buf, block, Line::from(spans), self.focus == Focus::Editor);
        let area = self.rects.editor;
        let focused = self.focus == Focus::Editor && matches!(self.mode, Mode::Normal);
        match &mut self.view {
            Some(View::Review(v)) => {
                v.render(area, buf);
                return None;
            }
            Some(View::Activity(v)) => {
                v.render(area, buf);
                return None;
            }
            None => {}
        }
        match self.docs.get_mut(self.active_doc) {
            Some(doc) => {
                let diags = self.lsp.diagnostics.get(&doc.path).map(Vec::as_slice).unwrap_or(&[]);
                let c = doc.render(area, buf, focused, &self.syntax, diags);
                if self.focus == Focus::Editor { c } else { None }
            }
            None => {
                draw_welcome(buf, area);
                None
            }
        }
    }

    fn draw_agents(&mut self, buf: &mut Buffer, cursor: &mut Option<(u16, u16)>) -> usize {
        let slots = if self.split { 2 } else { 1 };
        let hints = matches!(self.mode, Mode::Hints { .. });
        // Scan first so hint labels can be numbered across both panes.
        for slot in 0..slots {
            let area = self.rects.agents[slot];
            let idx = self.slots[slot];
            let Some(agent) = self.agents.get_mut(idx) else { continue };
            agent.resize(area.height, area.width);
            if agent.started() {
                let cwd = agent.cwd.clone();
                let symbols = self.symbols.clone();
                if !self.resolvers.contains_key(&cwd) {
                    let _ = self.resolver(&cwd);
                }
                let resolver = self.resolvers.get_mut(&cwd).unwrap();
                let screen = self.agents[idx].parser.screen();
                self.refs[slot] = refs::scan_screen(screen, resolver, Some(&symbols));
            }
        }
        let total = self.refs[0].len() + self.refs[1].len();
        for slot in 0..slots {
            let block = self.rects.agent_blocks[slot];
            let area = self.rects.agents[slot];
            let mut spans = vec![Span::raw(" ")];
            let mut x = block.x + 2;
            for (i, a) in self.agents.iter().enumerate() {
                let label = format!(" {} {} ", a.status_glyph(), a.name);
                let w = label.chars().count() as u16;
                self.rects.agent_tabs[slot].push((x, x + w, i));
                x += w + 1;
                let style = if i == self.slots[slot] {
                    Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::DIM)
                };
                spans.push(Span::styled(label, style));
                spans.push(Span::raw(" "));
            }
            let slot_focused = self.focus == Focus::Agent && (!self.split || self.active_slot == slot);
            pane(buf, block, Line::from(spans), slot_focused);
            let idx = self.slots[slot];
            let focused = slot_focused && matches!(self.mode, Mode::Normal);
            let hover = self.hover.filter(|(s, _, _)| *s == slot).map(|(_, r, c)| (r, c));
            let base = if slot == 0 { 0 } else { self.refs[0].len() };
            let refs = std::mem::take(&mut self.refs[slot]);
            if let Some(agent) = self.agents.get_mut(idx) {
                if agent.started() {
                    let c = agent.render(area, buf, focused, &refs, hover, hints.then_some((base, total)));
                    if slot_focused {
                        *cursor = c;
                    }
                } else {
                    let msg = format!("{} is not running — press Alt+3 or click here to start", agent.name);
                    buf.set_stringn(area.x + 1, area.y + 1, msg, area.width as usize, Style::default().fg(theme::DIM));
                }
            }
            self.refs[slot] = refs;
        }
        total
    }

    fn draw_status(&self, buf: &mut Buffer, area: Rect) {
        buf.set_style(area, Style::default().bg(theme::STATUS_BG).fg(theme::DIM));
        buf.set_string(area.x, area.y, " NOIDA ", Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD));
        let x = area.x + 8;
        let width = area.width.saturating_sub(8) as usize;

        let (left, color, strong) = match &self.mode {
            Mode::Hints { typed } => (
                format!("jump: type a label{}  ·  Enter = newest  ·  Esc = cancel", if typed.is_empty() { String::new() } else { format!(" [{typed}]") }),
                theme::ACCENT,
                false,
            ),
            Mode::Picker(_) => ("type to filter  ·  ↑↓ select  ·  Enter open  ·  Esc cancel".into(), theme::FG, false),
            Mode::Prompt { kind, input } => {
                let label = match kind {
                    PromptKind::GotoLine => "go to line[:col]",
                    PromptKind::Find => "find",
                    PromptKind::NewBranch => "new branch name",
                    PromptKind::Commit => "commit message",
                    PromptKind::WorktreeName(_) => "worktree task name",
                };
                (format!("{label}: {input}▏"), theme::ACCENT, false)
            }
            Mode::Normal => match (&self.message, &self.banner) {
                (Some((m, _, err)), _) => (m.clone(), if *err { theme::ERROR } else { theme::ACCENT }, false),
                (None, Some(b)) => (b.text.clone(), if b.error { theme::ERROR } else { theme::ADDED }, true),
                (None, None) => (
                    match (self.focus, &self.view) {
                        (Focus::Editor, Some(View::Review(v))) => v.status(),
                        (Focus::Editor, Some(View::Activity(v))) => v.status().into(),
                        (Focus::Tree, _) => "↑↓ move  ⏎ open  ← collapse  R refresh  │  Alt+x commands  Alt+o files  Alt+g sessions  Alt+q quit".into(),
                        (Focus::Editor, None) => "^S save  ^F find  F12 definition  Alt+l symbols  │  Alt+s send  Alt+e ask  Alt+r review  Alt+x commands".into(),
                        (Focus::Agent, _) => "Alt+j jump to ref  Alt+g sessions  Alt+n next  Alt+v split  │  Alt+r review  Alt+a activity  Alt+x commands".into(),
                    },
                    theme::DIM,
                    false,
                ),
            },
        };

        let mut right = String::new();
        if let Some(branch) = &self.git.branch {
            let changed = self.git.changed();
            right.push_str(&format!(" ⎇ {branch}{} ", if changed > 0 { format!(" ±{changed}") } else { String::new() }));
        }
        let (errors, warnings) = self.lsp.diagnostics.values().flatten().fold((0, 0), |(e, w), d| match d.severity {
            1 => (e + 1, w),
            2 => (e, w + 1),
            _ => (e, w),
        });
        if errors + warnings > 0 {
            right.push_str(&format!(" ✗{errors} ⚠{warnings} "));
        }
        if let Some(d) = self.doc().filter(|_| self.focus == Focus::Editor && self.view.is_none()) {
            let lsp = self.lsp.server_name(&d.path).map(|s| format!(" · {s}")).unwrap_or_default();
            right.push_str(&format!(" Ln {}, Col {}  {}{lsp} ", d.cursor_line(), d.cursor_col(), d.syntax_name));
        }
        let rw = right.chars().count();
        let style = if strong {
            Style::default().fg(theme::HINT_FG).bg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        let max_left = width.saturating_sub(rw + 1);
        let shown: String = left.chars().take(max_left).collect();
        buf.set_string(x, area.y, if strong { format!(" {shown} ") } else { shown }, style);
        if rw < width {
            buf.set_string(area.x + area.width - rw as u16, area.y, right, Style::default().fg(theme::DIM));
        }
    }
}

pub fn inner(r: Rect) -> Rect {
    if r.width < 2 || r.height < 2 {
        return Rect::default();
    }
    Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2)
}

fn pane(buf: &mut Buffer, area: Rect, title: Line, focused: bool) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let color = if focused { theme::BORDER_FOCUS } else { theme::BORDER };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
        .render(area, buf);
}

fn draw_welcome(buf: &mut Buffer, area: Rect) {
    let lines = [
        ("NOIDA", Style::default().fg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD)),
        ("Navigation-Oriented IDE for Developer Agents", Style::default().fg(theme::DIM)),
        ("", Style::default()),
        ("Alt+x            command palette", Style::default().fg(theme::FG)),
        ("Alt+o / Ctrl+P   open file", Style::default().fg(theme::FG)),
        ("click a path     in the agent pane to open it", Style::default().fg(theme::FG)),
        ("Alt+j            jump to a file ref by label", Style::default().fg(theme::FG)),
        ("Alt+s / Alt+e    send selection / ask the agent", Style::default().fg(theme::FG)),
        ("Alt+g            switch or resume agent sessions", Style::default().fg(theme::FG)),
        ("Alt+r            review agent changes", Style::default().fg(theme::FG)),
        ("Alt+1/2/3        files / editor / agent", Style::default().fg(theme::FG)),
    ];
    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    let block_w = lines.iter().map(|(t, _)| t.chars().count()).max().unwrap_or(0) as u16;
    let x = area.x + area.width.saturating_sub(block_w) / 2;
    for (i, (text, style)) in lines.iter().enumerate() {
        if top + (i as u16) < area.y + area.height {
            buf.set_stringn(x, top + i as u16, text, area.width as usize, *style);
        }
    }
}
