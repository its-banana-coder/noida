//! Tabs (preview, pin, close variants, reopen), file tree operations, the
//! outline panel and path utilities.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{App, Focus, Mode, PromptKind, groups};
use crate::actions::{Action, Ask};
use crate::editor::Doc;
use crate::picker::{Item, Kind, Picker, Target};
use crate::refs;
use crate::symbols::Symbol;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TreeMode {
    Files,
    Outline,
}

impl App {
    // ---- tabs ----

    /// Single-click opens a preview tab that the next preview replaces.
    pub(super) fn open_preview(&mut self, path: &Path) {
        if self.docs.iter().any(|d| d.path == path) {
            return self.open_path(path, None);
        }
        let reusable = (0..self.docs.len()).find(|&i| self.docs[i].preview && !self.docs[i].dirty && !self.shown_elsewhere(i));
        self.open_path(path, None);
        let Some(new_idx) = self.docs.iter().position(|d| d.path == path) else { return };
        self.docs[new_idx].preview = true;
        if let Some(old) = reusable.filter(|&o| o != new_idx) {
            self.drop_doc(old);
            self.set_active_doc(self.docs.iter().position(|d| d.path == path).unwrap_or(0));
        }
    }

    pub(super) fn pin_active_preview(&mut self) {
        if let Some(d) = self.docs.get_mut(self.groups[self.active_group].active) {
            d.preview = false;
        }
    }

    /// Remove a document without prompting (callers check dirty state).
    pub(super) fn drop_doc(&mut self, i: usize) {
        let d = self.docs.remove(i);
        self.lsp.did_close(&d.path);
        self.lsp_synced.remove(&d.path);
        self.closed.push((d.path.clone(), d.cursor_line(), d.cursor_col()));
        if self.closed.len() > 50 {
            self.closed.remove(0);
        }
        groups::on_doc_removed(&mut self.groups, &mut self.active_group, i, self.docs.len());
    }

    /// Close docs matching `pick`, skipping pinned and (unless confirmed) dirty ones.
    pub(super) fn close_where(&mut self, what: &str, pick: impl Fn(usize, &Doc) -> bool) {
        let dirty: Vec<String> = self.docs.iter().enumerate().filter(|(i, d)| pick(*i, d) && !d.pinned && d.dirty).map(|(_, d)| d.file_name()).collect();
        if !dirty.is_empty() && !self.confirmed(&format!("close-{what}")) {
            return self.error(format!("unsaved: {} — run again to discard", dirty.join(", ")));
        }
        let active = self.docs.get(self.active_doc()).map(|d| d.path.clone());
        let targets: Vec<usize> = self.docs.iter().enumerate().filter(|(i, d)| pick(*i, d) && !d.pinned).map(|(i, _)| i).collect();
        for i in targets.into_iter().rev() {
            self.drop_doc(i);
        }
        if let Some(p) = active {
            if let Some(i) = self.docs.iter().position(|d| d.path == p) {
                self.set_active_doc(i);
            }
        }
        self.refresh_marks();
    }

    pub(super) fn toggle_pin(&mut self) {
        let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return };
        doc.pinned = !doc.pinned;
        doc.preview = false;
        let pinned = doc.pinned;
        let from = self.active_doc();
        let d = self.docs.remove(from);
        let at = self.docs.iter().take_while(|d| d.pinned).count();
        self.docs.insert(at, d);
        groups::on_doc_moved(&mut self.groups, from, at);
        self.info(if pinned { "pinned tab" } else { "unpinned tab" });
    }

    pub(super) fn reopen_closed(&mut self) {
        while let Some((path, line, col)) = self.closed.pop() {
            if path.is_file() && !self.docs.iter().any(|d| d.path == path) {
                self.open_location(&path, Some(line), Some(col), None);
                self.focus = Focus::Editor;
                return;
            }
        }
        self.info("no recently closed files");
    }

    pub(super) fn recent_files(&mut self) {
        let mut seen = Vec::new();
        let mut items = Vec::new();
        let open = self.mru.iter().rev().cloned();
        let closed = self.closed.iter().rev().map(|(p, _, _)| p.clone());
        for p in open.chain(closed) {
            if seen.contains(&p) || !p.is_file() {
                continue;
            }
            let state = if self.docs.iter().any(|d| d.path == p) { "open" } else { "closed" };
            items.push(Item::new(refs::relative(&self.root, &p).into_owned(), state, Target::File { path: p.clone(), line: None, col: None }));
            seen.push(p);
        }
        self.mode = Mode::Picker(Picker::new("Recent Files", Kind::Static, items).ordered());
    }

    pub(super) fn recent_locations(&mut self) {
        let items = self
            .back
            .iter()
            .rev()
            .map(|(p, l, c)| {
                let text = self.docs.iter().find(|d| &d.path == p).and_then(|d| d.line_text(l.saturating_sub(1)).map(|t| t.trim().to_string())).unwrap_or_default();
                Item::new(format!("{}:{l}", refs::relative(&self.root, p)), text, Target::File { path: p.clone(), line: Some(*l), col: Some(*c) })
            })
            .collect();
        self.mode = Mode::Picker(Picker::new("Recent Locations", Kind::Static, items).ordered());
    }

    pub(super) fn touch_mru(&mut self) {
        let Some(p) = self.docs.get(self.active_doc()).map(|d| d.path.clone()) else { return };
        if self.mru.last() != Some(&p) {
            self.mru.retain(|x| x != &p);
            self.mru.push(p);
            if self.mru.len() > 100 {
                self.mru.remove(0);
            }
        }
    }

    pub(super) fn auto_save(&mut self) {
        if !self.settings.auto_save {
            return;
        }
        let mut saved = false;
        for d in self.docs.iter_mut().filter(|d| d.dirty) {
            if d.edited_at().is_some_and(|t| t.elapsed() > Duration::from_millis(1000)) && d.save().is_ok() {
                self.lsp.did_save(&d.path);
                saved = true;
            }
        }
        if saved {
            self.refresh_git();
        }
    }

    // ---- file tree operations ----

    /// Path the file commands act on: tree selection when the tree is focused, else the open file.
    fn subject_path(&self) -> Option<PathBuf> {
        match self.focus {
            Focus::Tree => self.tree.selected_path().map(Path::to_path_buf),
            _ => self.docs.get(self.active_doc()).map(|d| d.path.clone()),
        }
    }

    pub(super) fn prompt_new(&mut self, folder: bool) {
        let dir = if self.focus == Focus::Tree {
            self.tree.target_dir()
        } else {
            self.docs.get(self.active_doc()).and_then(|d| d.path.parent().map(Path::to_path_buf)).unwrap_or_else(|| self.root.clone())
        };
        let rel = refs::relative(&self.root, &dir).into_owned();
        let input = if rel.is_empty() || dir == self.root { String::new() } else { format!("{rel}/") };
        let kind = if folder { PromptKind::NewFolder } else { PromptKind::NewFile };
        self.mode = Mode::Prompt { kind, input };
    }

    pub(super) fn prompt_rename(&mut self) {
        let Some(path) = self.subject_path() else { return };
        self.rename_from = Some(path.clone());
        self.mode = Mode::Prompt { kind: PromptKind::Rename, input: refs::relative(&self.root, &path).into_owned() };
    }

    fn resolve_input(&self, input: &str) -> PathBuf {
        let p = PathBuf::from(input.trim());
        if p.is_absolute() { p } else { self.root.join(p) }
    }

    pub(super) fn create_entry(&mut self, input: &str, folder: bool) {
        let path = self.resolve_input(input);
        if input.trim().is_empty() || path.exists() {
            return self.error(format!("{} already exists", refs::relative(&self.root, &path)));
        }
        let result = if folder {
            std::fs::create_dir_all(&path)
        } else {
            path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, ""))
        };
        match result {
            Ok(()) => {
                self.tree.refresh();
                self.rebuild_index();
                if folder {
                    self.tree.reveal(&path.join("_"));
                    self.info(format!("created {}/", refs::relative(&self.root, &path)));
                } else {
                    self.open_path(&path, None);
                    self.focus = Focus::Editor;
                }
            }
            Err(e) => self.error(format!("create failed: {e}")),
        }
    }

    pub(super) fn rename_entry(&mut self, input: &str) {
        let Some(from) = self.rename_from.take() else { return };
        let to = self.resolve_input(input);
        if to == from {
            return;
        }
        if to.exists() {
            return self.error(format!("{} already exists", refs::relative(&self.root, &to)));
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::rename(&from, &to) {
            return self.error(format!("rename failed: {e}"));
        }
        for d in &mut self.docs {
            if let Ok(rest) = d.path.strip_prefix(&from) {
                let new = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
                self.lsp.did_close(&d.path);
                d.path = new;
                self.lsp.did_open(&d.path, &d.text());
            }
        }
        self.tree.refresh();
        self.tree.reveal(&to);
        self.rebuild_index();
        self.info(format!("moved to {}", refs::relative(&self.root, &to)));
    }

    pub(super) fn delete_entry(&mut self) {
        let Some(path) = self.subject_path() else { return };
        let rel = refs::relative(&self.root, &path).into_owned();
        if !self.confirmed(&format!("delete:{rel}")) {
            let what = if path.is_dir() { "folder and everything in it" } else { "file" };
            return self.error(format!("permanently delete {what} {rel}? press again to confirm"));
        }
        let result = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        match result {
            Ok(()) => {
                let gone: Vec<usize> = self.docs.iter().enumerate().filter(|(_, d)| d.path.starts_with(&path)).map(|(i, _)| i).collect();
                for i in gone.into_iter().rev() {
                    self.drop_doc(i);
                }
                self.tree.refresh();
                self.rebuild_index();
                self.refresh_git();
                self.info(format!("deleted {rel}"));
            }
            Err(e) => self.error(format!("delete failed: {e}")),
        }
    }

    pub(super) fn copy_path(&mut self, absolute: bool) {
        let Some(path) = self.subject_path() else { return };
        let text = if absolute { path.to_string_lossy().into_owned() } else { refs::relative(&self.root, &path).into_owned() };
        self.copy_to_host(&text);
        self.info(format!("copied {text}"));
    }

    pub(super) fn reveal_in_os(&mut self) {
        let Some(path) = self.subject_path() else { return };
        let on_path = |p: &str| std::env::var_os("PATH").is_some_and(|ps| std::env::split_paths(&ps).any(|d| d.join(p).is_file()));
        let spawn = |cmd: &mut Command| cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok();
        let ok = if on_path("wslpath") && on_path("explorer.exe") {
            let win = Command::new("wslpath").arg("-w").arg(&path).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            win.is_some_and(|w| spawn(Command::new("explorer.exe").arg(format!("/select,{w}"))))
        } else if cfg!(target_os = "macos") {
            spawn(Command::new("open").arg("-R").arg(&path))
        } else if on_path("xdg-open") {
            spawn(Command::new("xdg-open").arg(path.parent().unwrap_or(&path)))
        } else {
            false
        };
        if ok {
            self.info("opened in file manager");
        } else {
            self.error("no file manager found (tried explorer.exe, open, xdg-open)");
        }
    }

    // ---- agent helpers ----

    /// Keep the context file that `noida hook` hands to Claude on each prompt current.
    pub(super) fn write_shared_context(&mut self) {
        let Some(server) = &self.hook_server else { return };
        let text = match self.docs.get(self.active_doc()).filter(|_| self.settings.share_editor_context) {
            Some(doc) => {
                let rel = refs::relative(&self.root, &doc.path).into_owned();
                let (s, e) = doc.selected_lines();
                let sel = if doc.has_selection() {
                    if s == e { format!(", with line {s} selected") } else { format!(", with lines {s}-{e} selected") }
                } else {
                    String::new()
                };
                let others: Vec<String> = self
                    .docs
                    .iter()
                    .filter(|d| d.path != doc.path)
                    .take(5)
                    .map(|d| refs::relative(&self.root, &d.path).into_owned())
                    .collect();
                let tabs = if others.is_empty() { String::new() } else { format!(" Other open files: {}.", others.join(", ")) };
                format!(
                    "[NOIDA editor context] The user is looking at {rel} around line {}{sel} in their editor.{tabs} Use this only if it is relevant to their request.",
                    doc.cursor_line()
                )
            }
            None => String::new(),
        };
        if text != self.shared_context {
            if std::fs::write(&server.context, &text).is_ok() {
                self.shared_context = text;
            }
        }
    }

    pub(super) fn send_symbol(&mut self) {
        let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return self.info("open a file first") };
        let line = doc.cursor_line0();
        let Some(sym) = doc.scopes_at(line).pop() else { return self.info("no symbol at cursor") };
        let rel = refs::relative(&self.root, &doc.path).into_owned();
        let text = format!("@{rel}#L{}-{} ", sym.line + 1, sym.end_line + 1);
        self.set_focus(Focus::Agent);
        if let Some(a) = self.agent() {
            a.paste(&text);
        }
    }

    pub(super) fn ask_menu(&mut self) {
        let mut items: Vec<Item> = Ask::ALL.iter().map(|a| Item::new(a.label(), a.prompt(), Target::Action(Action::Ask(*a)))).collect();
        items.push(Item::new("Fix problem at cursor", "send the diagnostic to the agent", Target::Action(Action::FixProblem)));
        items.push(Item::new("Send current symbol", "@file#Lstart-end of the enclosing function/class", Target::Action(Action::SendSymbol)));
        items.push(Item::new("Send selection", "@file#Lx-y", Target::Action(Action::SendSelection)));
        items.push(Item::new("Send file", "@file", Target::Action(Action::SendFile)));
        self.mode = Mode::Picker(Picker::new("Ask Agent", Kind::Static, items).ordered());
    }

    // ---- outline panel ----

    pub(super) fn outline_symbols(&mut self) -> Vec<(usize, Symbol)> {
        let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return Vec::new() };
        let symbols = doc.outline().to_vec();
        let mut out = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for s in symbols {
            while stack.last().is_some_and(|&end| s.line > end) {
                stack.pop();
            }
            out.push((stack.len(), s.clone()));
            stack.push(s.end_line);
        }
        out
    }

    pub(super) fn outline_key(&mut self, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::KeyCode;
        let n = self.outline_symbols().len();
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.outline_selected = self.outline_selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.outline_selected = (self.outline_selected + 1).min(n.saturating_sub(1)),
            KeyCode::Enter => {
                if let Some((_, s)) = self.outline_symbols().get(self.outline_selected).cloned() {
                    if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                        doc.goto(s.line + 1, Some(s.col + 1), None);
                    }
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Tab => self.tree_mode = TreeMode::Files,
            KeyCode::Esc => self.focus = Focus::Editor,
            _ => {}
        }
    }

    pub(super) fn outline_click(&mut self, area: Rect, y: u16) {
        let i = self.outline_scroll + y.saturating_sub(area.y) as usize;
        if let Some((_, s)) = self.outline_symbols().get(i).cloned() {
            self.outline_selected = i;
            if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                doc.goto(s.line + 1, Some(s.col + 1), None);
            }
        }
    }

    pub(super) fn render_outline(&mut self, area: Rect, buf: &mut Buffer, focused: bool) {
        let items = self.outline_symbols();
        let cursor_line = self.docs.get(self.active_doc()).map(|d| d.cursor_line0());
        if items.is_empty() {
            let msg = if self.docs.is_empty() { "no file open" } else { "no symbols (supported: Rust, TS/JS, Python, Go, Java)" };
            buf.set_stringn(area.x + 1, area.y, msg, area.width as usize, Style::default().fg(theme::DIM()));
            return;
        }
        if !focused {
            // Follow the cursor: select the innermost symbol containing it.
            if let Some(line) = cursor_line {
                if let Some(i) = items.iter().rposition(|(_, s)| s.line <= line && line <= s.end_line) {
                    self.outline_selected = i;
                }
            }
        }
        self.outline_selected = self.outline_selected.min(items.len() - 1);
        let h = area.height as usize;
        if self.outline_selected < self.outline_scroll {
            self.outline_scroll = self.outline_selected;
        } else if self.outline_selected >= self.outline_scroll + h {
            self.outline_scroll = self.outline_selected + 1 - h;
        }
        for (row, (i, (depth, s))) in items.iter().enumerate().skip(self.outline_scroll).take(h).enumerate() {
            let y = area.y + row as u16;
            let mut style = Style::default().fg(theme::FG());
            if i == self.outline_selected {
                style = style.bg(if focused { theme::SELECT() } else { theme::SELECT_DIM() });
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
            }
            let icon = super::draw::symbol_icon(&s.kind);
            let text = format!("{}{} {}", "  ".repeat(*depth), icon, s.name);
            let (x, _) = buf.set_stringn(area.x, y, text, area.width as usize, style);
            let _ = x;
            let hint = format!("{}", s.line + 1);
            let hx = area.x + area.width.saturating_sub(hint.len() as u16);
            buf.set_string(hx, y, hint, style.fg(theme::DIM()).add_modifier(Modifier::DIM));
        }
    }

    pub(super) fn tree_double_click(&mut self, row: usize) -> bool {
        let now = Instant::now();
        let double = self.last_tree_click.is_some_and(|(r, t)| r == row && now.duration_since(t) < Duration::from_millis(400));
        self.last_tree_click = Some((row, now));
        double
    }
}
