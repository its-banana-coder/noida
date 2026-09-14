//! Top-level application state, input routing and layout.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use ratatui::Frame;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};

use crate::agent::{Agent, HINT_KEYS, PtyEvent, hint_label};
use crate::editor::{Doc, KeyResult, Syntax};
use crate::refs::{self, FileRef, Resolver};
use crate::theme;
use crate::tree::{Activate, Tree};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Editor,
    Agent,
}

enum Mode {
    Normal,
    Hints { typed: String },
    QuickOpen { query: String, results: Vec<String>, selected: usize },
    Prompt { kind: PromptKind, input: String },
}

#[derive(Clone, Copy)]
enum PromptKind {
    GotoLine,
    Find,
}

#[derive(PartialEq)]
enum Drag {
    None,
    Editor,
    AgentDivider,
    TreeDivider,
}

#[derive(Default)]
struct Rects {
    main: Rect,
    tree_block: Rect,
    tree: Rect,
    editor_block: Rect,
    editor: Rect,
    agent_block: Rect,
    agent: Rect,
    doc_tabs: Vec<(u16, u16, usize)>,
    agent_tabs: Vec<(u16, u16, usize)>,
}

pub struct App {
    root: PathBuf,
    syntax: Syntax,
    tree: Tree,
    docs: Vec<Doc>,
    active_doc: usize,
    agents: Vec<Agent>,
    active_agent: usize,
    focus: Focus,
    mode: Mode,
    show_tree: bool,
    zoom: bool,
    tree_width: u16,
    agent_pct: u16,
    rects: Rects,
    resolver: Resolver,
    refs: Vec<FileRef>,
    hover: Option<(u16, u16)>,
    drag: Drag,
    message: Option<(String, Instant, bool)>,
    back: Vec<(PathBuf, usize, usize)>,
    last_find: String,
    tx: Sender<PtyEvent>,
    file_index: Option<(Vec<String>, Instant)>,
    last_disk_check: Instant,
    last_tree_refresh: Instant,
    quit_armed: Option<Instant>,
    pub quit: bool,
    pub host_out: Vec<u8>,
}

impl App {
    pub fn new(root: PathBuf, agents: Vec<(String, String)>, tx: Sender<PtyEvent>) -> Self {
        let agents: Vec<Agent> = agents
            .iter()
            .enumerate()
            .map(|(i, (name, cmd))| Agent::new(i, name, cmd))
            .collect();
        Self {
            syntax: Syntax::load(),
            tree: Tree::new(root.clone()),
            resolver: Resolver::new(root.clone()),
            root,
            docs: Vec::new(),
            active_doc: 0,
            agents,
            active_agent: 0,
            focus: Focus::Agent,
            mode: Mode::Normal,
            show_tree: true,
            zoom: false,
            tree_width: 30,
            agent_pct: 45,
            rects: Rects::default(),
            refs: Vec::new(),
            hover: None,
            drag: Drag::None,
            message: None,
            back: Vec::new(),
            last_find: String::new(),
            tx,
            file_index: None,
            last_disk_check: Instant::now(),
            last_tree_refresh: Instant::now(),
            quit_armed: None,
            quit: false,
            host_out: Vec::new(),
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now(), true));
    }

    fn agent(&mut self) -> Option<&mut Agent> {
        self.agents.get_mut(self.active_agent)
    }

    pub fn ensure_agent_started(&mut self) {
        let root = self.root.clone();
        let tx = self.tx.clone();
        if let Some(a) = self.agent() {
            if !a.started() {
                a.start(&root, tx);
            }
        }
    }

    pub fn open_path(&mut self, path: &Path, line: Option<usize>) {
        self.open_location(path, line, None, None);
    }

    // ---- events ----

    pub fn on_pty(&mut self, ev: PtyEvent) {
        match ev {
            PtyEvent::Output(id, bytes) => {
                if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
                    let host = a.process(&bytes);
                    self.host_out.extend(host);
                }
            }
            PtyEvent::Exited(id) => {
                if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
                    a.on_exit();
                }
            }
        }
    }

    /// Periodic work: reload files changed by agents, refresh the tree.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if self.last_disk_check.elapsed() > Duration::from_millis(500) {
            self.last_disk_check = Instant::now();
            for i in 0..self.docs.len() {
                if let Some(msg) = self.docs[i].check_disk(&self.syntax) {
                    self.info(msg);
                    changed = true;
                }
            }
        }
        if self.last_tree_refresh.elapsed() > Duration::from_secs(2) {
            self.last_tree_refresh = Instant::now();
            self.tree.refresh();
            changed = true;
        }
        if self.message.as_ref().is_some_and(|(_, t, _)| t.elapsed() > Duration::from_secs(4)) {
            self.message = None;
            changed = true;
        }
        changed || self.docs.get(self.active_doc).is_some_and(Doc::flash_active)
    }

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) => self.on_mouse(m),
            Event::Paste(text) => self.on_paste(&text),
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::QuickOpen { query, .. } => {
                query.push_str(text.trim());
                self.update_quick_open();
            }
            Mode::Prompt { input, .. } => input.push_str(text.trim()),
            _ => match self.focus {
                Focus::Agent => {
                    if let Some(a) = self.agent() {
                        a.paste(text);
                    }
                }
                Focus::Editor => {
                    if let Some(d) = self.docs.get_mut(self.active_doc) {
                        d.insert_text(text);
                    }
                }
                Focus::Tree => {}
            },
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if !matches!(self.mode, Mode::Normal) {
            return self.on_mode_key(key);
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if alt && !ctrl {
            if let KeyCode::Char(c) = key.code {
                if self.on_global(c) {
                    return;
                }
            }
        }
        if self.quit_armed.is_some() {
            self.quit_armed = None;
        }

        if self.focus != Focus::Agent && ctrl {
            match key.code {
                KeyCode::Char('q') => return self.request_quit(),
                KeyCode::Char('p') => return self.start_quick_open(),
                _ => {}
            }
        }

        match self.focus {
            Focus::Tree => self.tree_key(key),
            Focus::Editor => self.editor_key(key),
            Focus::Agent => {
                self.ensure_agent_started();
                if let Some(a) = self.agent() {
                    a.send_key(key);
                }
            }
        }
    }

    /// Alt-shortcuts that work from every pane. Returns true if consumed.
    fn on_global(&mut self, c: char) -> bool {
        match c {
            '1' => {
                self.show_tree = true;
                self.set_focus(Focus::Tree);
            }
            '2' => self.set_focus(Focus::Editor),
            '3' => self.set_focus(Focus::Agent),
            '0' => {
                self.show_tree = !self.show_tree;
                if !self.show_tree && self.focus == Focus::Tree {
                    self.focus = Focus::Editor;
                }
            }
            'j' => self.start_hints(),
            'o' => self.start_quick_open(),
            's' => self.send_selection(),
            'n' => {
                if !self.agents.is_empty() {
                    self.active_agent = (self.active_agent + 1) % self.agents.len();
                    self.set_focus(Focus::Agent);
                }
            }
            'z' => self.zoom = !self.zoom,
            ',' => self.agent_pct = (self.agent_pct + 5).min(85),
            '.' => self.agent_pct = self.agent_pct.saturating_sub(5).max(15),
            '-' => self.go_back(),
            'q' => self.request_quit(),
            _ => return false,
        }
        true
    }

    fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        if f == Focus::Agent {
            self.ensure_agent_started();
        }
    }

    fn request_quit(&mut self) {
        let dirty: Vec<String> = self.docs.iter().filter(|d| d.dirty).map(Doc::file_name).collect();
        let armed = self.quit_armed.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        if dirty.is_empty() || armed {
            self.quit = true;
        } else {
            self.quit_armed = Some(Instant::now());
            self.error(format!("unsaved: {} — press again to quit anyway", dirty.join(", ")));
        }
    }

    fn tree_key(&mut self, key: KeyEvent) {
        let page = self.rects.tree.height.max(2) as isize - 1;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.tree.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.tree.move_by(1),
            KeyCode::PageUp => self.tree.move_by(-page),
            KeyCode::PageDown => self.tree.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.tree.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.tree.move_by(isize::MAX / 2),
            KeyCode::Left | KeyCode::Char('h') => self.tree.collapse(),
            KeyCode::Char('r') => {
                self.tree.refresh();
                self.info("tree refreshed");
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Activate::Open(p) = self.tree.expand() {
                    self.open_path(&p, None);
                }
            }
            KeyCode::Enter => {
                if let Activate::Open(p) = self.tree.activate() {
                    self.open_path(&p, None);
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Tab => self.focus = Focus::Editor,
            _ => {}
        }
    }

    fn editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('g') if ctrl => {
                self.mode = Mode::Prompt { kind: PromptKind::GotoLine, input: String::new() };
                return;
            }
            KeyCode::Char('f') if ctrl => {
                self.mode = Mode::Prompt { kind: PromptKind::Find, input: String::new() };
                return;
            }
            KeyCode::F(3) => {
                let q = self.last_find.clone();
                self.find(&q);
                return;
            }
            KeyCode::Char('w') if ctrl => return self.close_doc(),
            KeyCode::PageUp if ctrl => return self.cycle_doc(-1),
            KeyCode::PageDown if ctrl => return self.cycle_doc(1),
            _ => {}
        }
        let Some(doc) = self.docs.get_mut(self.active_doc) else {
            if key.code == KeyCode::Tab || key.code == KeyCode::Esc {
                self.focus = Focus::Tree;
            }
            return;
        };
        match doc.handle_key(key) {
            KeyResult::Copy(text) => {
                self.copy_to_host(&text);
                self.info("copied to clipboard");
            }
            KeyResult::Message(m) if m.starts_with("save failed") => self.error(m),
            KeyResult::Message(m) => self.info(m),
            KeyResult::Handled | KeyResult::Ignored => {}
        }
    }

    fn copy_to_host(&mut self, text: &str) {
        self.host_out.extend_from_slice(b"\x1b]52;c;");
        self.host_out.extend_from_slice(base64(text.as_bytes()).as_bytes());
        self.host_out.push(0x07);
    }

    fn cycle_doc(&mut self, delta: isize) {
        if self.docs.is_empty() {
            return;
        }
        let n = self.docs.len() as isize;
        self.active_doc = ((self.active_doc as isize + delta).rem_euclid(n)) as usize;
        let path = self.docs[self.active_doc].path.clone();
        self.tree.reveal(&path);
    }

    fn close_doc(&mut self) {
        let Some(doc) = self.docs.get(self.active_doc) else { return };
        let armed = self.quit_armed.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        if doc.dirty && !armed {
            self.quit_armed = Some(Instant::now());
            let name = doc.file_name();
            return self.error(format!("{name} has unsaved changes — Ctrl+W again to discard"));
        }
        self.quit_armed = None;
        self.docs.remove(self.active_doc);
        self.active_doc = self.active_doc.min(self.docs.len().saturating_sub(1));
    }

    fn on_mode_key(&mut self, key: KeyEvent) {
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Normal => {}
            Mode::Hints { mut typed } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(r) = self.refs.last().cloned() {
                        self.open_ref(&r);
                    }
                }
                KeyCode::Char(c) if HINT_KEYS.contains(c) => {
                    typed.push(c);
                    let total = self.refs.len();
                    let hit = (0..total).find(|&i| hint_label(i, total) == typed);
                    if let Some(i) = hit {
                        let r = self.refs[i].clone();
                        self.open_ref(&r);
                    } else if (0..total).any(|i| hint_label(i, total).starts_with(&typed)) {
                        self.mode = Mode::Hints { typed };
                    }
                }
                _ => {}
            },
            Mode::QuickOpen { mut query, results, mut selected } => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => return,
                    KeyCode::Enter => {
                        let (typed_path, line) = refs::split_line_suffix(&query);
                        let target = results.get(selected).cloned().or_else(|| {
                            Some(typed_path.to_string()).filter(|p| self.root.join(p).is_file())
                        });
                        if let Some(rel) = target {
                            let p = self.root.join(rel);
                            self.open_path(&p, line);
                            self.focus = Focus::Editor;
                        }
                        return;
                    }
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Char('p') if ctrl => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(results.len().saturating_sub(1)),
                    KeyCode::Char('n') if ctrl => selected = (selected + 1).min(results.len().saturating_sub(1)),
                    KeyCode::Backspace => {
                        query.pop();
                    }
                    KeyCode::Char(c) if !ctrl => query.push(c),
                    _ => {}
                }
                self.mode = Mode::QuickOpen { query, results, selected };
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace) && !ctrl {
                    self.update_quick_open();
                }
            }
            Mode::Prompt { kind, mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => match kind {
                    PromptKind::GotoLine => {
                        let mut parts = input.split(':').map(|s| s.trim().parse::<usize>().ok());
                        if let (Some(Some(line)), Some(doc)) = (parts.next(), self.docs.get_mut(self.active_doc)) {
                            let col = parts.next().flatten();
                            doc.goto(line, col, None);
                        }
                    }
                    PromptKind::Find => {
                        let q = if input.is_empty() { self.last_find.clone() } else { input };
                        self.find(&q);
                    }
                },
                KeyCode::Backspace => {
                    input.pop();
                    self.mode = Mode::Prompt { kind, input };
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    self.mode = Mode::Prompt { kind, input };
                }
                _ => self.mode = Mode::Prompt { kind, input },
            },
        }
    }

    fn find(&mut self, query: &str) {
        if query.is_empty() {
            return;
        }
        self.last_find = query.to_string();
        let Some(doc) = self.docs.get_mut(self.active_doc) else { return };
        if !doc.find(query) {
            self.error(format!("not found: {query}"));
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let r = &self.rects;
        let (tree, editor, agent, agent_block, tree_block, main) = (r.tree, r.editor, r.agent, r.agent_block, r.tree_block, r.main);

        self.hover = inside(agent).then(|| (y - agent.y, x - agent.x));

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if !matches!(self.mode, Mode::Normal) {
                    self.mode = Mode::Normal;
                }
                if agent_block.width > 0 && x == agent_block.x && y >= agent_block.y + 1 {
                    self.drag = Drag::AgentDivider;
                } else if tree_block.width > 0 && x + 1 == tree_block.x + tree_block.width && y >= tree_block.y + 1 {
                    self.drag = Drag::TreeDivider;
                } else if y == agent_block.y && inside(agent_block) {
                    if let Some(&(_, _, i)) = self.rects.agent_tabs.iter().find(|(a, b, _)| x >= *a && x < *b) {
                        self.active_agent = i;
                    }
                    self.set_focus(Focus::Agent);
                } else if y == self.rects.editor_block.y && inside(self.rects.editor_block) {
                    if let Some(&(_, _, i)) = self.rects.doc_tabs.iter().find(|(a, b, _)| x >= *a && x < *b) {
                        self.active_doc = i;
                        let p = self.docs[i].path.clone();
                        self.tree.reveal(&p);
                    }
                    self.focus = Focus::Editor;
                } else if inside(tree) {
                    self.focus = Focus::Tree;
                    if let Some(i) = self.tree.row_at(tree, y) {
                        self.tree.selected = i;
                        if let Activate::Open(p) = self.tree.activate() {
                            self.open_path(&p, None);
                        }
                    }
                } else if inside(editor) {
                    self.focus = Focus::Editor;
                    if let Some(doc) = self.docs.get_mut(self.active_doc) {
                        doc.click(editor, x, y, m.modifiers.contains(KeyModifiers::SHIFT));
                        self.drag = Drag::Editor;
                    }
                } else if inside(agent) {
                    let (row, col) = (y - agent.y, x - agent.x);
                    if let Some(r) = self.refs.iter().find(|r| r.contains(row, col)).cloned() {
                        self.open_ref(&r);
                    } else {
                        self.set_focus(Focus::Agent);
                        if let Some(a) = self.agents.get_mut(self.active_agent) {
                            a.forward_mouse(m, agent);
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Drag::Editor => {
                    if let Some(doc) = self.docs.get_mut(self.active_doc) {
                        doc.drag(editor, x, y);
                    }
                }
                Drag::AgentDivider if main.width > 0 => {
                    let right = main.x + main.width;
                    self.agent_pct = ((right.saturating_sub(x)) as u32 * 100 / main.width as u32).clamp(15, 85) as u16;
                }
                Drag::TreeDivider => self.tree_width = (x.saturating_sub(main.x) + 1).clamp(12, 80),
                _ => {
                    if inside(agent) {
                        if let Some(a) = self.agents.get_mut(self.active_agent) {
                            a.forward_mouse(m, agent);
                        }
                    }
                }
            },
            MouseEventKind::Up(_) => {
                self.drag = Drag::None;
                if inside(agent) {
                    if let Some(a) = self.agents.get_mut(self.active_agent) {
                        a.forward_mouse(m, agent);
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta: isize = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                if inside(tree) {
                    self.tree.scroll(delta);
                } else if inside(editor) {
                    if let Some(doc) = self.docs.get_mut(self.active_doc) {
                        doc.scroll(delta);
                    }
                } else if inside(agent) {
                    if let Some(a) = self.agents.get_mut(self.active_agent) {
                        if !a.forward_mouse(m, agent) {
                            a.scroll(-delta);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---- navigation ----

    fn open_ref(&mut self, r: &FileRef) {
        self.open_location(&r.path.clone(), r.line, r.col, r.end_line);
    }

    fn open_location(&mut self, path: &Path, line: Option<usize>, col: Option<usize>, end_line: Option<usize>) {
        if path.is_dir() {
            self.show_tree = true;
            self.tree.reveal(&path.join("_"));
            return self.info(format!("revealed {}", refs::relative(&self.root, path)));
        }
        if let Some(doc) = self.docs.get(self.active_doc) {
            let here = (doc.path.clone(), doc.cursor_line(), doc.cursor_col());
            if doc.path != path || line.is_some_and(|l| l != here.1) {
                self.back.push(here);
                if self.back.len() > 100 {
                    self.back.remove(0);
                }
            }
        }
        let idx = match self.docs.iter().position(|d| d.path == path) {
            Some(i) => i,
            None => match Doc::open(path, &self.syntax) {
                Ok(doc) => {
                    self.docs.push(doc);
                    self.docs.len() - 1
                }
                Err(e) => return self.error(format!("cannot open {}: {e}", refs::relative(&self.root, path))),
            },
        };
        self.active_doc = idx;
        if let Some(l) = line {
            self.docs[idx].goto(l, col, end_line);
        }
        if self.zoom && self.focus == Focus::Agent {
            self.zoom = false;
        }
        self.tree.reveal(path);
        let rel = refs::relative(&self.root, path).to_string();
        self.info(match line {
            Some(l) => format!("{rel}:{l}"),
            None => rel,
        });
    }

    fn go_back(&mut self) {
        let Some((path, line, col)) = self.back.pop() else {
            return self.info("no previous location");
        };
        let len = self.back.len();
        self.open_location(&path, Some(line), Some(col), None);
        self.back.truncate(len);
    }

    fn start_hints(&mut self) {
        if self.zoom && self.focus != Focus::Agent {
            self.zoom = false;
        }
        if self.refs.is_empty() {
            return self.info("no file references visible in the agent pane");
        }
        self.mode = Mode::Hints { typed: String::new() };
    }

    fn send_selection(&mut self) {
        let Some(doc) = self.docs.get(self.active_doc) else {
            return self.info("open a file first");
        };
        let rel = refs::relative(&self.root, &doc.path).to_string();
        let (s, e) = doc.selected_lines();
        let text = if s == e { format!("@{rel}#L{s} ") } else { format!("@{rel}#L{s}-{e} ") };
        self.set_focus(Focus::Agent);
        if let Some(a) = self.agent() {
            a.paste(&text);
        }
    }

    fn start_quick_open(&mut self) {
        let stale = self.file_index.as_ref().is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(10));
        if stale {
            let files: Vec<String> = WalkBuilder::new(&self.root)
                .hidden(false)
                .require_git(false)
                .filter_entry(|e| e.file_name() != ".git")
                .build()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .take(200_000)
                .map(|e| refs::relative(&self.root, e.path()).into_owned())
                .collect();
            self.file_index = Some((files, Instant::now()));
        }
        self.mode = Mode::QuickOpen { query: String::new(), results: Vec::new(), selected: 0 };
        self.update_quick_open();
    }

    fn update_quick_open(&mut self) {
        let Mode::QuickOpen { query, results, selected } = &mut self.mode else { return };
        let Some((files, _)) = &self.file_index else { return };
        let (q, _) = refs::split_line_suffix(query);
        let q = q.to_lowercase();
        if q.is_empty() {
            // Open buffers first, then everything else.
            let open: Vec<String> = self.docs.iter().rev().map(|d| refs::relative(&self.root, &d.path).into_owned()).collect();
            *results = open.iter().cloned().chain(files.iter().filter(|f| !open.contains(f)).take(200).cloned()).collect();
        } else {
            let mut scored: Vec<(i64, &String)> = files.iter().filter_map(|f| fuzzy_score(&q, f).map(|s| (s, f))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())));
            *results = scored.into_iter().take(200).map(|(_, f)| f.clone()).collect();
        }
        *selected = 0;
    }

    // ---- drawing ----

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
        let agent_block = if show_agent { it.next().unwrap() } else { Rect::default() };

        self.rects = Rects {
            main,
            tree_block,
            tree: inner(tree_block),
            editor_block,
            editor: inner(editor_block),
            agent_block,
            agent: inner(agent_block),
            doc_tabs: Vec::new(),
            agent_tabs: Vec::new(),
        };

        let mut cursor = None;
        let buf = f.buffer_mut();

        if show_tree {
            pane(buf, tree_block, Line::from(" FILES "), self.focus == Focus::Tree);
            let open = self.docs.get(self.active_doc).map(|d| d.path.clone());
            self.tree.render(self.rects.tree, buf, self.focus == Focus::Tree, open.as_deref());
        }

        if show_editor {
            let mut spans = vec![Span::raw(" ")];
            let mut x = editor_block.x + 2;
            for (i, d) in self.docs.iter().enumerate() {
                let label = format!(" {}{} ", d.file_name(), if d.dirty { " ●" } else { "" });
                let w = label.chars().count() as u16;
                self.rects.doc_tabs.push((x, x + w, i));
                x += w + 1;
                let style = if i == self.active_doc {
                    Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::DIM)
                };
                spans.push(Span::styled(label, style));
                spans.push(Span::raw(" "));
            }
            if self.docs.is_empty() {
                spans.push(Span::raw("EDITOR "));
            }
            pane(buf, editor_block, Line::from(spans), self.focus == Focus::Editor);
            let area = self.rects.editor;
            match self.docs.get_mut(self.active_doc) {
                Some(doc) => {
                    let c = doc.render(area, buf, self.focus == Focus::Editor, &self.syntax);
                    if self.focus == Focus::Editor {
                        cursor = c;
                    }
                }
                None => draw_welcome(buf, area),
            }
        }

        self.refs.clear();
        if show_agent {
            let mut spans = vec![Span::raw(" ")];
            let mut x = agent_block.x + 2;
            for (i, a) in self.agents.iter().enumerate() {
                let dot = if a.running() { "●" } else { "○" };
                let label = format!(" {dot} {} ", a.name);
                let w = label.chars().count() as u16;
                self.rects.agent_tabs.push((x, x + w, i));
                x += w + 1;
                let style = if i == self.active_agent {
                    Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::DIM)
                };
                spans.push(Span::styled(label, style));
                spans.push(Span::raw(" "));
            }
            pane(buf, agent_block, Line::from(spans), self.focus == Focus::Agent);
            let area = self.rects.agent;
            let hints = matches!(self.mode, Mode::Hints { .. });
            let focused = self.focus == Focus::Agent && matches!(self.mode, Mode::Normal);
            if let Some(agent) = self.agents.get_mut(self.active_agent) {
                agent.resize(area.height, area.width);
                if agent.started() {
                    self.refs = refs::scan_screen(agent.parser.screen(), &mut self.resolver);
                    let c = agent.render(area, buf, focused, &self.refs, self.hover, hints);
                    if self.focus == Focus::Agent {
                        cursor = c;
                    }
                } else {
                    let msg = format!("{} is not running — press Alt+3 or click here to start", agent.name);
                    buf.set_stringn(area.x + 1, area.y + 1, msg, area.width as usize, Style::default().fg(theme::DIM));
                }
            }
        }

        self.draw_status(buf, status);

        if let Mode::QuickOpen { query, results, selected } = &self.mode {
            cursor = Some(draw_quick_open(buf, area, query, results, *selected));
        } else if let Mode::Prompt { .. } = self.mode {
            cursor = None;
        }

        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    fn draw_status(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        buf.set_style(area, Style::default().bg(theme::STATUS_BG).fg(theme::DIM));
        buf.set_string(area.x, area.y, " NOIDA ", Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD));
        let x = area.x + 8;
        let width = area.width.saturating_sub(8) as usize;

        let left = match &self.mode {
            Mode::Hints { typed } => (format!("jump to file: type a label{}  ·  Enter = newest  ·  Esc = cancel", if typed.is_empty() { String::new() } else { format!(" [{typed}]") }), theme::ACCENT),
            Mode::QuickOpen { .. } => ("type to filter  ·  ↑↓ select  ·  Enter open  ·  path:line jumps  ·  Esc cancel".into(), theme::FG),
            Mode::Prompt { kind, input } => {
                let label = match kind {
                    PromptKind::GotoLine => "go to line[:col]",
                    PromptKind::Find => "find",
                };
                (format!("{label}: {input}▏"), theme::ACCENT)
            }
            Mode::Normal => match &self.message {
                Some((m, _, err)) => (m.clone(), if *err { theme::ERROR } else { theme::ACCENT }),
                None => (
                    match self.focus {
                        Focus::Tree => "↑↓ move  ⏎ open  ← collapse  r refresh  │  Alt+2 editor  Alt+3 agent  Alt+o files  Alt+q quit",
                        Focus::Editor => "^S save  ^F find  ^G line  ^P files  ^W close  │  Alt+s send to agent  Alt+- back  Alt+3 agent",
                        Focus::Agent => "Alt+j jump to file ref  click refs  Alt+n next agent  │  Alt+2 editor  Alt+1 files  Alt+z zoom",
                    }
                    .into(),
                    theme::DIM,
                ),
            },
        };

        let right = match self.docs.get(self.active_doc) {
            Some(d) if self.focus == Focus::Editor => format!(
                " Ln {}, Col {}  {}  {} ",
                d.cursor_line(),
                d.cursor_col(),
                d.line_count(),
                d.syntax_name
            ),
            _ => String::new(),
        };
        let rw = right.chars().count();
        buf.set_stringn(x, area.y, &left.0, width.saturating_sub(rw + 1), Style::default().fg(left.1));
        if rw < width {
            buf.set_string(area.x + area.width - rw as u16, area.y, right, Style::default().fg(theme::DIM));
        }
    }
}

fn inner(r: Rect) -> Rect {
    if r.width < 2 || r.height < 2 {
        return Rect::default();
    }
    Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2)
}

fn pane(buf: &mut ratatui::buffer::Buffer, area: Rect, title: Line, focused: bool) {
    let color = if focused { theme::BORDER_FOCUS } else { theme::BORDER };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
        .render(area, buf);
}

fn draw_welcome(buf: &mut ratatui::buffer::Buffer, area: Rect) {
    let lines = [
        ("NOIDA", Style::default().fg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD)),
        ("Navigation-Oriented IDE for Developer Agents", Style::default().fg(theme::DIM)),
        ("", Style::default()),
        ("Alt+o / Ctrl+P   open file", Style::default().fg(theme::FG)),
        ("click a path     in the agent pane to open it", Style::default().fg(theme::FG)),
        ("Alt+j            jump to a file ref by label", Style::default().fg(theme::FG)),
        ("Alt+s            send selection to the agent", Style::default().fg(theme::FG)),
        ("Alt+1/2/3        files / editor / agent", Style::default().fg(theme::FG)),
        ("Alt+q            quit", Style::default().fg(theme::FG)),
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

fn draw_quick_open(buf: &mut ratatui::buffer::Buffer, area: Rect, query: &str, results: &[String], selected: usize) -> (u16, u16) {
    let w = (area.width * 3 / 5).clamp(40.min(area.width), 100.min(area.width));
    let h = (results.len() as u16 + 3).clamp(4, (area.height * 3 / 5).max(4));
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + area.height / 8, w, h.min(area.height));
    Clear.render(popup, buf);
    buf.set_style(popup, Style::default().bg(theme::STATUS_BG));
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(" Open file ")
        .render(popup, buf);
    let body = inner(popup);
    buf.set_stringn(body.x, body.y, format!("› {query}"), body.width as usize, Style::default().fg(theme::FG));
    let list_h = body.height.saturating_sub(1) as usize;
    let offset = selected.saturating_sub(list_h.saturating_sub(1));
    for (row, (i, item)) in results.iter().enumerate().skip(offset).take(list_h).enumerate() {
        let y = body.y + 1 + row as u16;
        let style = if i == selected {
            Style::default().fg(theme::HINT_FG).bg(theme::BORDER_FOCUS)
        } else {
            Style::default().fg(theme::FG)
        };
        if i == selected {
            buf.set_style(Rect::new(body.x, y, body.width, 1), style);
        }
        let (dir, name) = item.rsplit_once('/').map_or(("", item.as_str()), |(d, n)| (d, n));
        buf.set_stringn(body.x + 1, y, name, body.width as usize - 1, style.add_modifier(Modifier::BOLD));
        let nx = body.x + 2 + name.chars().count() as u16;
        if !dir.is_empty() && nx < body.x + body.width {
            let dim = if i == selected { style } else { Style::default().fg(theme::DIM) };
            buf.set_stringn(nx, y, dir, (body.x + body.width - nx) as usize, dim);
        }
    }
    if results.is_empty() && body.height > 1 {
        buf.set_string(body.x + 1, body.y + 1, "no matches", Style::default().fg(theme::DIM));
    }
    (body.x + 2 + query.chars().count() as u16, body.y)
}

/// Subsequence fuzzy match with bonuses for word starts and consecutive runs.
fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let cand = candidate.to_lowercase();
    let bytes = cand.as_bytes();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for qc in query.bytes() {
        let found = bytes[pos..].iter().position(|&b| b == qc)? + pos;
        score += 10;
        if last == Some(found.wrapping_sub(1)) {
            score += 15;
        }
        if found == 0 || matches!(bytes[found - 1], b'/' | b'_' | b'-' | b'.' | b' ') {
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

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64() {
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64(b"hi!"), "aGkh");
        assert_eq!(base64(b"h"), "aA==");
    }

    #[test]
    fn fuzzy_prefers_filename() {
        let a = fuzzy_score("app", "src/app.rs").unwrap();
        let b = fuzzy_score("app", "a/p/p/other.rs").unwrap();
        assert!(a > b);
        assert!(fuzzy_score("xyz", "src/app.rs").is_none());
    }
}
