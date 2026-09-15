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
        f.buffer_mut().set_style(area, Style::default().bg(theme::BG()).fg(theme::FG()));

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
        // Views take the whole editor area; otherwise each group gets an equal half.
        let mut group_blocks = [Rect::default(); 2];
        if self.view.is_none() && self.groups.len() == 2 && editor_block.width >= 40 {
            let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(editor_block);
            group_blocks = [left, right];
        } else {
            group_blocks[self.active_group] = editor_block;
        }

        self.rects = Rects {
            main,
            tree_block,
            tree: inner(tree_block),
            editor_block,
            editor: inner(editor_block),
            group_blocks,
            groups: [inner(group_blocks[0]), inner(group_blocks[1])],
            agent_area,
            agent_blocks,
            agents: [inner(agent_blocks[0]), inner(agent_blocks[1])],
            doc_tabs: [Vec::new(), Vec::new()],
            agent_tabs: [Vec::new(), Vec::new()],
            agent_closes: [Vec::new(), Vec::new()],
            agent_new: [None, None],
        };

        let mut cursor = None;
        let buf = f.buffer_mut();

        if show_tree {
            let on = Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD);
            let off = Style::default().fg(theme::DIM());
            let outline = self.tree_mode == super::TreeMode::Outline;
            let title = Line::from(vec![
                Span::raw(" "),
                Span::styled(" FILES ", if outline { off } else { on }),
                Span::raw(" "),
                Span::styled(" OUTLINE ", if outline { on } else { off }),
                Span::raw(" "),
            ]);
            pane(buf, tree_block, title, self.focus == Focus::Tree);
            if outline {
                self.render_outline(self.rects.tree, buf, self.focus == Focus::Tree);
            } else {
                let open = self.doc().map(|d| d.path.clone());
                self.tree.render(self.rects.tree, buf, self.focus == Focus::Tree, open.as_deref(), &self.activity.touched);
            }
        }

        if show_editor {
            cursor = self.draw_editor(buf).or(cursor);
        }

        self.refs = [Vec::new(), Vec::new()];
        if show_agent {
            let total_refs = self.draw_agents(buf, &mut cursor);
            let _ = total_refs;
        }

        self.render_popup(buf, self.rects.editor, self.last_cursor);
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

    fn draw_editor(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        // Render the focused group last: a doc shown in both groups keeps the
        // layout (mouse mapping) of the focused one.
        let active = self.active_group;
        let mut cursor = None;
        for g in [1 - active, active] {
            if g < self.groups.len() && self.rects.group_blocks[g].width > 0 {
                let c = self.draw_group(buf, g);
                if g == active {
                    cursor = c;
                }
            }
        }
        cursor
    }

    fn draw_group(&mut self, buf: &mut Buffer, g: usize) -> Option<(u16, u16)> {
        let block = self.rects.group_blocks[g];
        let is_active = g == self.active_group;
        let shown = self.groups[g].active;
        let mut spans = vec![Span::raw(" ")];
        let mut x = block.x + 2;
        let active_style = Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD);
        if let Some(view) = &self.view {
            let title = match view {
                View::Review(v) => format!(" ± {} ", v.title()),
                View::Activity(v) => format!(" ⏱ {} ", v.title),
                View::Search(_) => " ⌕ Search ".to_string(),
                View::AgentChanges(_) => " ✓ Agent Changes ".to_string(),
                View::Compare(v) => format!(" ⇄ {} ", v.title),
                View::Transcript(v) => format!(" » {} ", v.title.chars().take(40).collect::<String>()),
            };
            x += title.chars().count() as u16 + 1;
            spans.push(Span::styled(title, active_style));
            spans.push(Span::raw(" "));
        }
        // Scroll the tab strip so the active tab stays visible.
        let labels: Vec<String> = self
            .docs
            .iter()
            .map(|d| format!(" {}{}{} ", if d.pinned { "▪ " } else { "" }, d.file_name(), if d.dirty { " ●" } else { "" }))
            .collect();
        let avail = block.width.saturating_sub(x - block.x + 2);
        let mut first = 0;
        let width_of = |from: usize, to: usize| labels[from..=to].iter().map(|l| l.chars().count() as u16 + 1).sum::<u16>();
        if !labels.is_empty() {
            let active = shown.min(labels.len() - 1);
            while first < active && width_of(first, active) + 2 > avail {
                first += 1;
            }
        }
        if first > 0 {
            spans.push(Span::styled("‹ ", Style::default().fg(theme::DIM())));
            x += 2;
        }
        for (i, d) in self.docs.iter().enumerate().skip(first) {
            let label = labels[i].clone();
            let w = label.chars().count() as u16;
            if x + w > block.x + block.width.saturating_sub(2) {
                spans.push(Span::styled("›", Style::default().fg(theme::DIM())));
                break;
            }
            self.rects.doc_tabs[g].push((x, x + w, i));
            x += w + 1;
            let mut style = match (i == shown && self.view.is_none(), is_active) {
                (true, true) => active_style,
                (true, false) => Style::default().fg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD),
                _ => Style::default().fg(theme::DIM()),
            };
            if d.preview {
                style = style.add_modifier(Modifier::ITALIC);
            }
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        if self.docs.is_empty() && self.view.is_none() {
            spans.push(Span::raw("EDITOR "));
        }
        pane(buf, block, Line::from(spans), is_active && self.focus == Focus::Editor);
        let area = self.rects.groups[g];
        let editor_focused = is_active && self.focus == Focus::Editor;
        let focused = editor_focused && matches!(self.mode, Mode::Normal);
        match &mut self.view {
            Some(View::Review(v)) => {
                v.render(area, buf);
                return None;
            }
            Some(View::Activity(v)) => {
                v.render(area, buf);
                return None;
            }
            Some(View::AgentChanges(v)) => {
                v.render(area, buf);
                return None;
            }
            Some(View::Compare(v)) => {
                v.render(area, buf);
                return None;
            }
            Some(View::Transcript(v)) => {
                v.render(area, buf);
                return None;
            }
            Some(View::Search(v)) => {
                let c = v.render(area, buf);
                return if editor_focused { c } else { None };
            }
            None => {}
        }
        let root = self.root.clone();
        let (breadcrumbs, sticky) = (self.settings.breadcrumbs, self.settings.sticky_scroll);
        let focused = focused || (editor_focused && matches!(self.mode, Mode::Find(_)));
        match self.docs.get_mut(shown) {
            Some(doc) if doc.md_preview => {
                if area.height < 2 || area.width < 10 {
                    return None;
                }
                let head = Rect::new(area.x, area.y, area.width, 1);
                buf.set_style(head, Style::default().bg(theme::STATUS_BG()));
                let title = format!(" {} · preview ", refs::relative(&root, &doc.path));
                buf.set_stringn(head.x, head.y, &title, head.width as usize, Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()));
                let hint = "Alt+m edit ";
                buf.set_string(head.x + head.width.saturating_sub(hint.len() as u16), head.y, hint, Style::default().fg(theme::ACCENT()).bg(theme::STATUS_BG()));
                let body = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), area.height - 1);
                let lines = doc.markdown_lines(body.width);
                let max_scroll = lines.len().saturating_sub(body.height as usize);
                doc.md_scroll = doc.md_scroll.min(max_scroll);
                for (row, line) in lines.iter().skip(doc.md_scroll).take(body.height as usize).enumerate() {
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
                if lines.len() > body.height as usize {
                    let pct = (doc.md_scroll * 100) / max_scroll.max(1);
                    let tag = format!(" {pct}% ");
                    buf.set_string(head.x + head.width.saturating_sub(hint.len() as u16 + tag.len() as u16 + 1), head.y, tag, Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()));
                }
                None
            }
            Some(doc) => {
                let mut text_area = area;
                if breadcrumbs && area.height > 4 {
                    draw_breadcrumbs(buf, Rect::new(area.x, area.y, area.width, 1), &root, doc);
                    text_area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
                }
                let diags = self.lsp.diagnostics.get(&doc.path).map(Vec::as_slice).unwrap_or(&[]);
                let c = doc.render(text_area, buf, focused, &self.syntax, diags, sticky);
                if !is_active {
                    return None;
                }
                self.last_cursor = c;
                let doc_ref: &crate::editor::Doc = doc;
                if let Mode::Find(bar) = &self.mode {
                    return bar.render(text_area, buf, Some(doc_ref));
                }
                if editor_focused { c } else { None }
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
            let labels: Vec<String> = self
                .agents
                .iter()
                .map(|a| match self.unreviewed.get(&a.name) {
                    Some(&n) if n > 0 => format!(" {} {} ○{n} ", a.status_glyph(), a.label()),
                    _ => format!(" {} {} ", a.status_glyph(), a.label()),
                })
                .collect();
            // Scroll the strip so the pane's agent stays visible; keep room for "+".
            let avail = block.width.saturating_sub(10) as usize;
            let tab_w = |i: usize| labels[i].chars().count() + 3;
            let active = self.slots[slot].min(labels.len().saturating_sub(1));
            let mut first = 0;
            while first < active && (first..=active).map(tab_w).sum::<usize>() > avail {
                first += 1;
            }
            if first > 0 {
                spans.push(Span::styled("‹ ", Style::default().fg(theme::DIM())));
                x += 2;
            }
            let mut used = 0;
            let mut truncated = false;
            for (i, a) in self.agents.iter().enumerate().skip(first) {
                if used + tab_w(i) > avail && i > active {
                    truncated = true;
                    break;
                }
                used += tab_w(i);
                let _ = a;
                let label = labels[i].clone();
                let w = label.chars().count() as u16;
                self.rects.agent_tabs[slot].push((x, x + w, i));
                let style = if i == self.slots[slot] {
                    Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::DIM())
                };
                spans.push(Span::styled(label, style));
                // Close button right after the label.
                self.rects.agent_closes[slot].push((x + w, i));
                spans.push(Span::styled("✕", style.remove_modifier(Modifier::BOLD)));
                spans.push(Span::raw("  "));
                x += w + 3;
            }
            if truncated {
                spans.push(Span::styled("› ", Style::default().fg(theme::DIM())));
                x += 2;
            }
            // New session button.
            self.rects.agent_new[slot] = Some(x);
            spans.push(Span::styled(" + ", Style::default().fg(theme::ACCENT()).add_modifier(Modifier::BOLD)));
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
                    if let Mode::AgentFind(bar) = &self.mode {
                        if bar.agent_id == agent.id {
                            *cursor = bar.render(area, buf);
                        }
                    }
                } else {
                    let msg = format!("{} is not running — press Alt+3 or click here to start", agent.name);
                    buf.set_stringn(area.x + 1, area.y + 1, msg, area.width as usize, Style::default().fg(theme::DIM()));
                }
            }
            self.refs[slot] = refs;
        }
        total
    }

    fn draw_status(&self, buf: &mut Buffer, area: Rect) {
        buf.set_style(area, Style::default().bg(theme::STATUS_BG()).fg(theme::DIM()));
        buf.set_string(area.x, area.y, " NOIDA ", Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD));
        let x = area.x + 8;
        let width = area.width.saturating_sub(8) as usize;

        let (left, color, strong) = match &self.mode {
            Mode::Hints { typed } => (
                format!("jump: type a label{}  ·  Enter = newest  ·  Esc = cancel", if typed.is_empty() { String::new() } else { format!(" [{typed}]") }),
                theme::ACCENT(),
                false,
            ),
            Mode::Picker(_) => ("type to filter  ·  ↑↓ select  ·  Enter open  ·  Esc cancel".into(), theme::FG(), false),
            Mode::Find(_) => ("Enter/↓ next · ↑ previous · Tab replace field · Alt+c case · Alt+w word · Alt+r regex · Alt+l in selection · Alt+Enter select all".into(), theme::FG(), false),
            Mode::AgentFind(_) => ("find in agent output: Enter/↓ next · ↑ previous · Esc close · capitals make it case-sensitive".into(), theme::FG(), false),
            Mode::Prompt { kind, input } => {
                let label = match kind {
                    PromptKind::GotoLine => "go to line[:col]",
                    PromptKind::NewFile => "new file path",
                    PromptKind::NewFolder => "new folder path",
                    PromptKind::Rename => "rename / move to",
                    PromptKind::RenameSymbol => "rename symbol to",
                    PromptKind::RenameAgent => "rename agent tab (empty = automatic title)",
                    PromptKind::NewBranch => "new branch name",
                    PromptKind::Commit => "commit message",
                    PromptKind::WorktreeName(_) => "worktree task name",
                    PromptKind::SendToPair(..) => "prompt for both agents",
                };
                (format!("{label}: {input}▏"), theme::ACCENT(), false)
            }
            Mode::Normal if self.leader => (
                "Ctrl+] … press a shortcut key: x commands · o open · j jump · g sessions · s send · e ask · r review · / search · 1/2/3 focus · Esc cancel".into(),
                theme::ACCENT(),
                true,
            ),
            Mode::Normal => match (&self.message, &self.banner) {
                (Some((m, _, err)), _) => (m.clone(), if *err { theme::ERROR() } else { theme::ACCENT() }, false),
                (None, Some(b)) => (b.text.clone(), if b.error { theme::ERROR() } else { theme::ADDED() }, true),
                (None, None) => (
                    match (self.focus, &self.view) {
                        (Focus::Editor, Some(View::Review(v))) => v.status(),
                        (Focus::Editor, Some(View::Activity(v))) => v.status().into(),
                        (Focus::Editor, Some(View::Search(v))) => v.status(),
                        (Focus::Editor, Some(View::AgentChanges(v))) => v.status().into(),
                        (Focus::Editor, Some(View::Compare(v))) => v.status().into(),
                        (Focus::Editor, Some(View::Transcript(v))) => v.status().into(),
                        (Focus::Tree, _) => "↑↓ move  ⏎ open  ← collapse  R refresh  │  Alt+x commands  Alt+o files  Alt+g sessions  Alt+q quit".into(),
                        (Focus::Editor, None) => "^S save  ^F find  F12 definition  Alt+l symbols  │  Alt+s send  Alt+e ask  Alt+r review  Alt+x commands".into(),
                        (Focus::Agent, _) => "Alt+j jump to ref  Alt+? find  drag to copy  Alt+g sessions  Alt+n next  Alt+v split  │  Alt+r review  Alt+x commands".into(),
                    },
                    theme::DIM(),
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
            Style::default().fg(theme::HINT_FG()).bg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        let max_left = width.saturating_sub(rw + 1);
        let shown: String = left.chars().take(max_left).collect();
        buf.set_string(x, area.y, if strong { format!(" {shown} ") } else { shown }, style);
        if rw < width {
            buf.set_string(area.x + area.width - rw as u16, area.y, right, Style::default().fg(theme::DIM()));
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
    let color = if focused { theme::BORDER_FOCUS() } else { theme::BORDER() };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
        .render(area, buf);
}

fn draw_breadcrumbs(buf: &mut Buffer, area: Rect, root: &std::path::Path, doc: &mut crate::editor::Doc) {
    let rel = refs::relative(root, &doc.path).into_owned();
    let mut parts: Vec<(String, Style)> = rel.split('/').map(|p| (p.to_string(), Style::default().fg(theme::DIM()))).collect();
    if let Some(last) = parts.last_mut() {
        last.1 = Style::default().fg(theme::FG());
    }
    let line = doc.cursor_line0();
    for s in doc.scopes_at(line) {
        parts.push((format!("{} {}", symbol_icon(&s.kind), s.name), Style::default().fg(theme::ACCENT())));
    }
    let mut x = area.x + 1;
    let end = area.x + area.width;
    for (i, (text, style)) in parts.iter().enumerate() {
        if i > 0 {
            if x + 3 >= end {
                break;
            }
            buf.set_string(x, area.y, " › ", Style::default().fg(theme::BORDER()));
            x += 3;
        }
        let (nx, _) = buf.set_stringn(x, area.y, text, end.saturating_sub(x) as usize, *style);
        x = nx;
    }
}

pub(super) fn symbol_icon(kind: &str) -> &'static str {
    match kind {
        "class" | "struct" => "◇",
        "interface" | "trait" => "◈",
        "method" | "function" => "ƒ",
        "module" => "▣",
        "macro" => "!",
        _ => "·",
    }
}

fn draw_welcome(buf: &mut Buffer, area: Rect) {
    let lines = [
        ("NOIDA", Style::default().fg(theme::BORDER_FOCUS()).add_modifier(Modifier::BOLD)),
        ("Navigation-Oriented IDE for Developer Agents", Style::default().fg(theme::DIM())),
        ("", Style::default()),
        ("Alt+x            command palette", Style::default().fg(theme::FG())),
        ("Alt+o / Ctrl+P   open file", Style::default().fg(theme::FG())),
        ("click a path     in the agent pane to open it", Style::default().fg(theme::FG())),
        ("Alt+j            jump to a file ref by label", Style::default().fg(theme::FG())),
        ("Alt+s / Alt+e    send selection / ask the agent", Style::default().fg(theme::FG())),
        ("Alt+g            switch or resume agent sessions", Style::default().fg(theme::FG())),
        ("Alt+r            review agent changes", Style::default().fg(theme::FG())),
        ("Alt+1/2/3        files / editor / agent", Style::default().fg(theme::FG())),
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
