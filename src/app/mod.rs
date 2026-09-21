//! Top-level application state, input routing and command dispatch.

mod agent_views;
mod draw;
mod files;
mod findbar;
mod groups;
mod lsp_ui;
mod search;
mod views;

pub use search::SearchResults;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::actions::{Action, AgentKind};
use crate::activity::{Activity, Notice, Turn};
use crate::agent::{Agent, HINT_KEYS, hint_label};
use crate::changes::{self, Mark};
use crate::editor::{Click, Doc, KeyResult, SearchOpts, Syntax};
use crate::editor::vim;
use crate::keys::{self, Keymap};
use crate::settings::{self, Settings};
use crate::events::Bg;
use crate::git::{self, Repo};
use crate::hooks::{self, AgentEvent};
use crate::lsp::{self, Lsp, Request, Response};
use crate::picker::{Item, Kind, Outcome, Picker, Target};
use crate::refs::{self, FileIndex, FileRef, Resolver};
use crate::sessions;
use crate::symbols::{self, ProjectSymbols};
use crate::tree::{Activate, Tree};
use crate::workspace::{self, AgentState, DocState, Workspace};

use findbar::{AgentFindBar, FindBar, FindResult};
use groups::Group;
use files::TreeMode;
use lsp_ui::{Popup, PopupKind};
use search::SearchView;
use agent_views::{AgentChangesView, ChangeGroup, ChangedFile, CompareFile, CompareSide, CompareView};
use views::{ActivityView, ReviewView, ViewResult};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Editor,
    Agent,
}

enum Mode {
    Normal,
    Hints { typed: String },
    Picker(Picker),
    Prompt { kind: PromptKind, input: String },
    Find(FindBar),
    AgentFind(AgentFindBar),
}

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    GotoLine,
    NewFile,
    NewFolder,
    Rename,
    RenameSymbol,
    RenameAgent,
    NewBranch,
    Commit,
    WorktreeName(AgentKind),
    SendToPair(usize, usize),
}

enum View {
    Review(ReviewView),
    Activity(ActivityView),
    Search(SearchView),
    AgentChanges(AgentChangesView),
    Compare(CompareView),
    Transcript(views::TranscriptView),
    Tests(views::TestView),
}

#[derive(PartialEq)]
enum Drag {
    None,
    Editor,
    AgentDivider,
    TreeDivider,
    SplitDivider,
    AgentSelect,
}

/// Left button pressed in an agent pane that doesn't use the mouse itself.
#[derive(Clone, Copy)]
struct AgentPress {
    slot: usize,
    row: u16,
    col: u16,
    /// (absolute line, col) where the selection starts.
    anchor: (usize, u16),
    moved: bool,
}

struct Banner {
    text: String,
    error: bool,
    at: Instant,
    changed: Vec<String>,
}

#[derive(Default)]
struct Rects {
    main: Rect,
    tree_block: Rect,
    tree: Rect,
    editor_block: Rect,
    editor: Rect,
    /// Per editor group: bordered block and inner text area (zero-sized when hidden).
    group_blocks: [Rect; 2],
    groups: [Rect; 2],
    agent_area: Rect,
    agent_blocks: [Rect; 2],
    agents: [Rect; 2],
    doc_tabs: [Vec<(u16, u16, usize)>; 2],
    agent_tabs: [Vec<(u16, u16, usize)>; 2],
    agent_closes: [Vec<(u16, usize)>; 2],
    /// Column of the "+" new-session button in each agent pane title.
    agent_new: [Option<u16>; 2],
}

type Loc = (PathBuf, usize, usize);

pub struct Options {
    pub root: PathBuf,
    pub agents: Option<Vec<(String, String)>>,
    pub restore: bool,
    pub exe: PathBuf,
}

pub struct App {
    root: PathBuf,
    exe: PathBuf,
    syntax: Syntax,
    tree: Tree,
    docs: Vec<Doc>,
    groups: Vec<Group>,
    active_group: usize,
    agents: Vec<Agent>,
    next_agent_id: usize,
    slots: [usize; 2],
    split: bool,
    split_pct: u16,
    active_slot: usize,
    focus: Focus,
    mode: Mode,
    view: Option<View>,
    show_tree: bool,
    zoom: bool,
    tree_width: u16,
    agent_pct: u16,
    rects: Rects,
    resolvers: HashMap<PathBuf, Resolver>,
    refs: [Vec<FileRef>; 2],
    hover: Option<(usize, u16, u16)>,
    drag: Drag,
    agent_press: Option<AgentPress>,
    message: Option<(String, Instant, bool)>,
    banner: Option<Banner>,
    back: Vec<Loc>,
    forward: Vec<Loc>,
    last_search: Option<(String, SearchOpts)>,
    settings: Settings,
    /// Parsed `settings.keys`.
    keymap: Keymap,
    parked_search: Option<View>,
    closed: Vec<Loc>,
    mru: Vec<PathBuf>,
    rename_from: Option<PathBuf>,
    tree_mode: TreeMode,
    outline_selected: usize,
    outline_scroll: usize,
    last_tree_click: Option<(usize, Instant)>,
    popup: Option<Popup>,
    code_action_list: Vec<serde_json::Value>,
    organize_pending: bool,
    pending_format: Option<PathBuf>,
    last_cursor: Option<(u16, u16)>,
    leader: bool,
    shared_context: String,
    tx: Sender<Bg>,
    /// Modal editing state, when `"vim": true`.
    vim: vim::Vim,
    /// Identifies the newest test run, so a replaced run's late output is ignored.
    test_run: u64,
    file_index: Arc<FileIndex>,
    index_built: Option<Instant>,
    index_building: bool,
    symbols: Arc<ProjectSymbols>,
    symbols_built: Option<Instant>,
    symbols_building: bool,
    repo: Option<Repo>,
    git: git::Status,
    git_building: bool,
    last_git: Instant,
    activity: Activity,
    last_tool: HashMap<usize, String>,
    history: Vec<Turn>,
    /// Manual reviewed marks per (agent name, activity path).
    reviewed: HashMap<(String, String), Mark>,
    /// Git status of each worktree agent's checkout.
    worktree_git: HashMap<PathBuf, git::Status>,
    /// Unreviewed changed-file count per agent name, shown in agent tabs.
    unreviewed: HashMap<String, usize>,
    /// View to return to when a review opened from it closes.
    return_view: Option<View>,
    lsp: Lsp,
    lsp_synced: HashMap<PathBuf, u64>,
    pending_definition: Option<String>,
    hook_server: Option<hooks::Server>,
    resuming: HashSet<usize>,
    confirm: Option<(String, Instant)>,
    last_disk_check: Instant,
    last_tree_refresh: Instant,
    last_workspace_save: Instant,
    last_title_check: Instant,
    pub quit: bool,
    pub host_out: Vec<u8>,
}

fn has_flag(command: &str, flags: &[&str]) -> bool {
    command.split_whitespace().any(|w| flags.contains(&w))
}

impl App {
    pub fn new(opts: Options, tx: Sender<Bg>) -> Self {
        let root = opts.root;
        let saved = if opts.restore { workspace::load(&root) } else { None };
        let hook_server = hooks::Server::start(tx.clone()).ok();
        let mut app = Self {
            exe: opts.exe,
            syntax: Syntax::load(),
            tree: Tree::new(root.clone()),
            docs: Vec::new(),
            groups: vec![Group::default()],
            active_group: 0,
            agents: Vec::new(),
            next_agent_id: 0,
            slots: [0, 0],
            split: false,
            split_pct: 50,
            active_slot: 0,
            focus: Focus::Agent,
            mode: Mode::Normal,
            view: None,
            show_tree: true,
            zoom: false,
            tree_width: 30,
            agent_pct: 45,
            rects: Rects::default(),
            resolvers: HashMap::new(),
            refs: [Vec::new(), Vec::new()],
            hover: None,
            drag: Drag::None,
            agent_press: None,
            message: None,
            banner: None,
            back: Vec::new(),
            forward: Vec::new(),
            last_search: None,
            settings: Settings::default(),
            keymap: Keymap::new(),
            parked_search: None,
            closed: Vec::new(),
            mru: Vec::new(),
            rename_from: None,
            tree_mode: TreeMode::Files,
            outline_selected: 0,
            outline_scroll: 0,
            last_tree_click: None,
            popup: None,
            code_action_list: Vec::new(),
            organize_pending: false,
            pending_format: None,
            last_cursor: None,
            leader: false,
            shared_context: String::new(),
            file_index: Arc::default(),
            index_built: None,
            index_building: false,
            symbols: Arc::default(),
            symbols_built: None,
            symbols_building: false,
            repo: Repo::discover(&root),
            git: git::Status::default(),
            git_building: false,
            last_git: Instant::now() - Duration::from_secs(10),
            activity: Activity::new(root.clone()),
            last_tool: HashMap::new(),
            history: Vec::new(),
            reviewed: HashMap::new(),
            worktree_git: HashMap::new(),
            unreviewed: HashMap::new(),
            return_view: None,
            lsp: Lsp::new(root.clone(), tx.clone()),
            lsp_synced: HashMap::new(),
            pending_definition: None,
            hook_server,
            resuming: HashSet::new(),
            confirm: None,
            last_disk_check: Instant::now(),
            last_tree_refresh: Instant::now(),
            last_workspace_save: Instant::now(),
            last_title_check: Instant::now(),
            quit: false,
            host_out: Vec::new(),
            tx,
            vim: vim::Vim::new(),
            test_run: 0,
            root,
        };
        app.rebuild_index();
        match settings::load() {
            Ok(s) => {
                let (keymap, bad) = keys::keymap(&s.keys);
                app.keymap = keymap;
                app.lsp.configure(&s.lsp);
                app.settings = s;
                if !bad.is_empty() {
                    app.error(format!("settings.json keys: {}", bad.join(", ")));
                }
            }
            Err(e) => app.error(e),
        }

        let agent_specs = opts.agents.is_some();
        if let Some(specs) = opts.agents {
            for (name, cmd) in specs {
                let root = app.root.clone();
                app.push_agent(&name, &cmd, root);
            }
        }
        if let Some(ws) = saved {
            app.apply_workspace(ws, !agent_specs);
        }
        if app.agents.is_empty() {
            for name in ["claude", "codex"] {
                if on_path(name) {
                    let root = app.root.clone();
                    app.push_agent(name, name, root);
                }
            }
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
            let root = app.root.clone();
            app.push_agent("shell", &shell, root);
        }
        app
    }

    // ---- workspace ----

    fn apply_workspace(&mut self, ws: Workspace, with_agents: bool) {
        self.show_tree = ws.show_tree;
        self.tree_width = ws.tree_width.clamp(12, 80);
        self.agent_pct = ws.agent_pct.clamp(15, 85);
        self.split_pct = ws.split_pct.unwrap_or(50).clamp(15, 85);
        self.tree.expand_rel(&ws.expanded);
        // Saved index → index in `self.docs` (files that vanished are skipped).
        let mut opened = Vec::new();
        for d in &ws.docs {
            let path = self.root.join(&d.path);
            if path.is_file() {
                self.open_location(&path, Some(d.line), Some(d.col), None);
            }
            opened.push(self.docs.iter().position(|doc| doc.path == path));
        }
        let restore = |saved: usize| opened.get(saved).copied().flatten().unwrap_or(0);
        if ws.groups.len() == 2 && !self.docs.is_empty() {
            self.groups = ws.groups.iter().map(|&i| Group { active: restore(i) }).collect();
            self.active_group = ws.active_group.min(1);
        } else {
            self.groups = vec![Group { active: restore(ws.active_doc) }];
            self.active_group = 0;
        }
        self.back.clear();
        self.message = None;
        if with_agents {
            for a in ws.agents {
                let cwd = a.cwd.map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| self.root.clone());
                let idx = self.push_agent(&a.name, &a.command, cwd.clone());
                self.agents[idx].session_id = a.session_id;
                self.agents[idx].title = a.title;
                self.agents[idx].renamed = a.renamed;
                if let Some(branch) = a.worktree_branch {
                    self.agents[idx].worktree = Some((cwd, branch));
                }
            }
            let n = self.agents.len();
            if n > 0 {
                self.slots = [ws.slots[0].min(n - 1), ws.slots[1].min(n - 1)];
                // Launch with a single agent pane; other tabs start when opened.
                self.split = false;
            }
        }
    }

    fn workspace(&self) -> Workspace {
        // Only docs inside the project are saved, so map indices to the saved list.
        let saved: Vec<usize> = (0..self.docs.len()).filter(|&i| self.docs[i].path.starts_with(&self.root)).collect();
        let saved_index = |i: usize| saved.iter().position(|&s| s == i).unwrap_or(0);
        Workspace {
            docs: saved
                .iter()
                .map(|&i| &self.docs[i])
                .map(|d| DocState { path: refs::relative(&self.root, &d.path).into_owned(), line: d.cursor_line(), col: d.cursor_col() })
                .collect(),
            active_doc: saved_index(self.active_doc()),
            groups: if self.groups.len() > 1 { self.groups.iter().map(|g| saved_index(g.active)).collect() } else { Vec::new() },
            active_group: self.active_group,
            show_tree: self.show_tree,
            tree_width: self.tree_width,
            agent_pct: self.agent_pct,
            split: self.split,
            split_pct: Some(self.split_pct),
            agents: self
                .agents
                .iter()
                .map(|a| AgentState {
                    name: a.name.clone(),
                    command: a.command.clone(),
                    cwd: (a.cwd != self.root).then(|| a.cwd.to_string_lossy().into_owned()),
                    session_id: a.session_id.clone(),
                    worktree_branch: a.worktree.as_ref().map(|(_, b)| b.clone()),
                    title: a.title.clone(),
                    renamed: a.renamed,
                })
                .collect(),
            slots: self.slots,
            expanded: self.tree.expanded_rel(),
        }
    }

    pub fn save_workspace(&self) {
        workspace::save(&self.root, &self.workspace());
    }

    // ---- helpers ----

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now(), true));
    }

    fn confirmed(&mut self, key: &str) -> bool {
        let ok = self.confirm.as_ref().is_some_and(|(k, t)| k == key && t.elapsed() < Duration::from_secs(3));
        self.confirm = if ok { None } else { Some((key.to_string(), Instant::now())) };
        ok
    }

    fn active_agent(&self) -> usize {
        self.slots[if self.split { self.active_slot } else { 0 }]
    }

    fn agent(&mut self) -> Option<&mut Agent> {
        let i = self.active_agent();
        self.agents.get_mut(i)
    }

    fn doc(&self) -> Option<&Doc> {
        self.docs.get(self.active_doc())
    }

    fn doc_mut(&mut self) -> Option<&mut Doc> {
        let i = self.active_doc();
        self.docs.get_mut(i)
    }

    fn push_agent(&mut self, name: &str, command: &str, cwd: PathBuf) -> usize {
        let id = self.next_agent_id;
        self.next_agent_id += 1;
        self.agents.push(Agent::new(id, name, command, cwd));
        self.agents.len() - 1
    }

    fn unique_name(&self, base: &str) -> String {
        if !self.agents.iter().any(|a| a.name == base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base}-{n}")).find(|n| !self.agents.iter().any(|a| &a.name == n)).unwrap()
    }

    fn start_agent(&mut self, idx: usize) {
        let Some(a) = self.agents.get(idx) else { return };
        let mut args: Vec<String> = Vec::new();
        let mut env = vec![(hooks::ENV_AGENT.to_string(), a.id.to_string())];
        if let Some(server) = &self.hook_server {
            env.push((hooks::ENV_SOCKET.to_string(), server.addr.clone()));
            env.push((hooks::ENV_CONTEXT.to_string(), server.context.to_string_lossy().into_owned()));
        }
        let hooked = self.hook_server.is_some();
        let mut resume = false;
        let mut new_session = None;
        match a.kind {
            Some(AgentKind::Claude) => {
                if hooked {
                    args.extend(["--settings".to_string(), hooks::claude_settings(&self.exe)]);
                }
                if !has_flag(&a.command, &["--resume", "-r", "--continue", "-c", "--session-id"]) {
                    let home = crate::settings::home().unwrap_or_default();
                    let transcript = a.session_id.as_ref().map(|id| sessions::claude_project_dir(&home, &a.cwd).join(format!("{id}.jsonl")));
                    match (&a.session_id, transcript) {
                        (Some(id), Some(t)) if t.exists() => {
                            args.extend(["--resume".to_string(), id.clone()]);
                            resume = true;
                        }
                        _ => {
                            let id = workspace::uuid_v4();
                            args.extend(["--session-id".to_string(), id.clone()]);
                            new_session = Some(id);
                        }
                    }
                }
            }
            Some(AgentKind::Codex) if hooked => args.extend(["-c".to_string(), hooks::codex_notify(&self.exe)]),
            _ => {}
        }
        let tx = self.tx.clone();
        let a = &mut self.agents[idx];
        if let Some(id) = new_session {
            a.session_id = Some(id);
        }
        if a.kind == Some(AgentKind::Claude) && hooked {
            a.hook_working = Some(false);
        }
        a.start(tx, &args, &env);
        if resume {
            self.resuming.insert(a.id);
        }
    }

    pub fn ensure_agent_started(&mut self) {
        let idx = self.active_agent();
        if self.agents.get(idx).is_some_and(|a| !a.started()) {
            self.start_agent(idx);
        }
    }

    pub fn start_visible_agents(&mut self) {
        self.ensure_agent_started();
        if self.split {
            let other = self.slots[1];
            if self.agents.get(other).is_some_and(|a| !a.started()) {
                self.start_agent(other);
            }
        }
    }

    fn resolver(&mut self, root: &Path) -> &mut Resolver {
        if !self.resolvers.contains_key(root) {
            let mut r = Resolver::new(root.to_path_buf());
            if root == self.root {
                r.set_index(self.file_index.clone());
            }
            self.resolvers.insert(root.to_path_buf(), r);
        }
        self.resolvers.get_mut(root).unwrap()
    }

    // ---- background work ----

    fn rebuild_index(&mut self) {
        if self.index_building {
            return;
        }
        self.index_building = true;
        let tx = self.tx.clone();
        let root = self.root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Index(FileIndex::build(&root)));
        });
    }

    fn rebuild_symbols(&mut self) {
        if self.symbols_building || self.file_index.files.is_empty() {
            return;
        }
        self.symbols_building = true;
        let tx = self.tx.clone();
        let root = self.root.clone();
        let files = self.file_index.files.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Symbols(ProjectSymbols::build(&root, &files)));
        });
    }

    fn refresh_git(&mut self) {
        let Some(repo) = self.repo.clone() else { return };
        if self.git_building {
            return;
        }
        self.git_building = true;
        self.last_git = Instant::now();
        let tx = self.tx.clone();
        let worktrees: Vec<PathBuf> = self.agents.iter().filter_map(|a| a.worktree.as_ref().map(|w| w.0.clone())).collect();
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Git(repo.status().ok()));
            if !worktrees.is_empty() {
                let statuses = worktrees.into_iter().filter_map(|p| Repo { root: p.clone() }.status().ok().map(|s| (p, s))).collect();
                let _ = tx.send(Bg::WorktreeGit(statuses));
            }
        });
    }

    fn refresh_marks(&mut self) {
        let Some(repo) = &self.repo else { return };
        if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
            if doc.path.starts_with(&repo.root) {
                doc.marks = repo.line_marks(&doc.path);
            }
        }
    }

    pub fn on_bg(&mut self, ev: Bg) {
        match ev {
            Bg::PtyOutput(id, bytes) => {
                if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
                    let host = a.process(&bytes);
                    self.host_out.extend(host);
                }
            }
            Bg::PtyExit(id) => self.on_agent_exit(id),
            Bg::Hook(id, ev) => self.on_hook(id, ev),
            Bg::Index(index) => {
                self.index_building = false;
                self.index_built = Some(Instant::now());
                self.file_index = Arc::new(index);
                let idx = self.file_index.clone();
                let root = self.root.clone();
                self.resolver(&root).set_index(idx);
                if let Mode::Picker(p) = &self.mode {
                    if p.kind == Kind::Files {
                        self.update_quick_open();
                    }
                }
                if self.symbols_built.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                    self.rebuild_symbols();
                }
            }
            Bg::Symbols(s) => {
                self.symbols_building = false;
                self.symbols_built = Some(Instant::now());
                self.symbols = Arc::new(s);
            }
            Bg::Git(status) => {
                self.git_building = false;
                if let Some(st) = status {
                    self.tree.set_git(&st.files);
                    self.git = st;
                }
                self.refresh_marks();
                self.refresh_agent_views();
            }
            Bg::WorktreeGit(statuses) => {
                self.worktree_git = statuses.into_iter().collect();
                self.refresh_agent_views();
            }
            Bg::Sessions(list) => {
                if let Mode::Picker(p) = &mut self.mode {
                    if p.title == "Past Conversations" {
                        let items: Vec<Item> = list
                            .into_iter()
                            .map(|s| Item::new(format!("{} · {}", s.kind.name(), s.title), sessions::ago(s.modified), Target::Transcript(s.kind, s.id, s.path, s.title)))
                            .collect();
                        if items.is_empty() {
                            p.set_items(vec![Item::new("No past conversations for this project", "start one with Alt+N", Target::Action(Action::NewAgent(AgentKind::Claude)))]);
                        } else {
                            p.set_items(items);
                        }
                    } else if p.title == "Sessions" {
                        let mut items = session_tab_items(&self.agents);
                        items.extend(list.into_iter().map(|s| {
                            Item::new(format!("↻ {} · {}", s.kind.name(), s.title), sessions::ago(s.modified), Target::Resume(s.kind, s.id))
                        }));
                        p.set_items(items);
                    }
                }
            }
            Bg::TestDone(id, outcome, output) => {
                // Ignore a run the user has already replaced.
                if id == self.test_run {
                    let summary = outcome.summary();
                    let failed = !outcome.ok;
                    let root = self.root.clone();
                    let mut view = self.view.take();
                    if let Some(View::Tests(v)) = &mut view {
                        v.finish(outcome, &output, self.resolver(&root));
                    }
                    self.view = view;
                    self.banner = Some(Banner {
                        text: format!("{} tests: {summary}", if failed { "✗" } else { "✓" }),
                        error: failed,
                        at: Instant::now(),
                        changed: Vec::new(),
                    });
                }
            }
            Bg::Search(r) => {
                if let Some(View::Search(v)) = &mut self.view {
                    v.on_results(r);
                }
            }
            Bg::Lsp(ev) => match self.lsp.handle(ev) {
                Response::Ready(name) => self.info(format!("{name} ready")),
                Response::Locations(kind, locs) => self.on_locations(kind, locs),
                Response::Hover(text) => self.on_hover(text),
                Response::Signature(label, hl) => self.on_signature(label, hl),
                Response::Edit(edit) => self.apply_workspace_edit(edit),
                Response::CodeActions(actions) => self.on_code_actions(actions),
                Response::Error(e) => {
                    self.pending_format = None;
                    self.organize_pending = false;
                    self.error(e);
                }
                Response::Diagnostics | Response::None => {}
            },
        }
    }

    /// Run the project's tests and show the result.
    ///
    /// Runs in the focused agent's worktree when it has one, so an agent
    /// working in isolation is tested against its own changes rather than
    /// against the main checkout.
    fn run_tests(&mut self) {
        let configured = self.settings.test_command.clone();
        let Some(runner) = crate::testing::Runner::detect(&self.root, configured.as_deref()) else {
            return self.error("no test command found — set \"test_command\" in settings.json");
        };
        let dir = self
            .agents
            .get(self.active_agent())
            .and_then(|a| a.worktree.as_ref().map(|(p, _)| p.clone()))
            .unwrap_or_else(|| self.root.clone());
        self.test_run += 1;
        self.view = Some(View::Tests(views::TestView::new(format!("Tests · {}", runner.label))));
        crate::testing::run(self.test_run, runner, dir, self.tx.clone());
    }

    fn on_agent_exit(&mut self, id: usize) {
        let Some(idx) = self.agents.iter().position(|a| a.id == id) else { return };
        self.agents[idx].on_exit();
        // A stale session id makes `--resume` exit immediately: start fresh instead.
        if self.resuming.remove(&id) && self.agents[idx].died_quickly() {
            let a = &mut self.agents[idx];
            a.session_id = None;
            a.reset();
            self.start_agent(idx);
            self.info("could not resume the previous conversation; started a new one");
        }
    }

    fn on_hook(&mut self, id: usize, ev: AgentEvent) {
        let Some(idx) = self.agents.iter().position(|a| a.id == id) else { return };
        self.resuming.remove(&id);
        let name = self.agents[idx].name.clone();
        match &ev {
            AgentEvent::SessionStart { session_id } => self.agents[idx].session_id = Some(session_id.clone()),
            AgentEvent::Prompt { text } => {
                if self.agents[idx].hook_working.is_some() {
                    self.agents[idx].hook_working = Some(true);
                }
                let a = &mut self.agents[idx];
                if a.title.is_none() && !a.renamed {
                    let words: Vec<&str> = text.split_whitespace().filter(|w| !w.starts_with('@')).take(6).collect();
                    if !words.is_empty() {
                        a.title = Some(words.join(" "));
                    }
                }
            }
            AgentEvent::ToolStart { tool, file, detail } => {
                let what = detail.clone().or_else(|| file.as_ref().map(|f| refs::relative(&self.root, f).into_owned())).unwrap_or_default();
                self.last_tool.insert(id, format!("{tool}({what})"));
                if let Some(b) = &self.banner {
                    if b.error {
                        self.banner = None;
                    }
                }
            }
            AgentEvent::ToolEnd { tool, .. } => {
                self.last_tool.remove(&id);
                if matches!(tool.as_str(), "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Bash") {
                    self.refresh_git();
                    self.last_disk_check = Instant::now() - Duration::from_secs(1);
                }
            }
            AgentEvent::Stop => {
                if self.agents[idx].hook_working.is_some() {
                    self.agents[idx].hook_working = Some(false);
                }
            }
            // Only the human can clear a permission prompt, so it becomes part
            // of the agent's state rather than just a banner that times out.
            AgentEvent::Notification { permission, .. } => {
                self.agents[idx].awaiting_permission = *permission;
            }
        }
        match self.activity.record(id, &name, &ev) {
            Notice::Permission(message) => {
                self.agents[idx].alert();
                let what = self.last_tool.get(&id).cloned().unwrap_or(message);
                let key = if self.active_agent() == idx && self.focus == Focus::Agent { "answer in its pane" } else { "Alt+g to switch" };
                self.banner = Some(Banner { text: format!("⚠ {name} needs permission: {what} — {key}"), error: true, at: Instant::now(), changed: Vec::new() });
            }
            Notice::Finished { changed } if !changed.is_empty() => {
                self.refresh_git();
                let n = changed.len();
                self.banner = Some(Banner {
                    // The diff says what changed; the tests say whether to trust it.
                    text: format!("✓ {name} finished — changed {n} file{} — Alt+r review · Alt+u test", if n == 1 { "" } else { "s" }),
                    error: false,
                    at: Instant::now(),
                    changed,
                });
            }
            Notice::Finished { .. } => {
                if self.banner.as_ref().is_some_and(|b| b.error) {
                    self.banner = None;
                }
            }
            Notice::None => {}
        }
        if let Some(View::Activity(v)) = &mut self.view {
            if v.title == "Agent Activity" {
                v.set_turns(self.activity.turns.clone());
            }
        }
        self.refresh_agent_views();
    }

    /// Periodic work. Returns true when a redraw is needed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if self.index_built.is_some_and(|t| t.elapsed() > Duration::from_secs(30)) {
            self.index_built = None;
            self.rebuild_index();
        }
        let visible: Vec<usize> = if self.split { self.slots.to_vec() } else { vec![self.slots[0]] };
        for (i, a) in self.agents.iter_mut().enumerate() {
            changed |= a.update_status(visible.contains(&i));
        }
        if self.last_disk_check.elapsed() > Duration::from_millis(500) {
            self.last_disk_check = Instant::now();
            for i in 0..self.docs.len() {
                if let Some(msg) = self.docs[i].check_disk(&self.syntax) {
                    self.info(msg);
                    if i == self.active_doc() {
                        self.refresh_marks();
                    }
                    changed = true;
                }
            }
        }
        // Keep language servers in sync, debounced.
        for doc in &self.docs {
            let synced = self.lsp_synced.get(&doc.path).copied().unwrap_or(0);
            if doc.edits != synced && doc.edited_at().is_none_or(|t| t.elapsed() > Duration::from_millis(300)) {
                self.lsp.did_change(&doc.path, &doc.text());
                self.lsp_synced.insert(doc.path.clone(), doc.edits);
            }
        }
        if self.last_tree_refresh.elapsed() > Duration::from_secs(2) {
            self.last_tree_refresh = Instant::now();
            self.tree.refresh();
            changed = true;
        }
        if self.last_git.elapsed() > Duration::from_secs(3) {
            self.refresh_git();
        }
        // Name Claude tabs after their conversation once Claude has titled it.
        if self.last_title_check.elapsed() > Duration::from_secs(20) {
            self.last_title_check = Instant::now();
            for a in self.agents.iter_mut().filter(|a| a.kind == Some(AgentKind::Claude) && !a.renamed && a.started()) {
                if let Some(id) = &a.session_id {
                    if let Some(t) = sessions::claude_session_title(&a.cwd, id) {
                        if a.title.as_ref() != Some(&t) {
                            a.title = Some(t);
                            changed = true;
                        }
                    }
                }
            }
        }
        if self.last_workspace_save.elapsed() > Duration::from_secs(15) {
            self.last_workspace_save = Instant::now();
            self.save_workspace();
        }
        if self.message.as_ref().is_some_and(|(_, t, _)| t.elapsed() > Duration::from_secs(4)) {
            self.message = None;
            changed = true;
        }
        if self.banner.as_ref().is_some_and(|b| b.at.elapsed() > Duration::from_secs(if b.error { 120 } else { 30 })) {
            self.banner = None;
            changed = true;
        }
        if let Some(View::Search(v)) = &mut self.view {
            changed |= v.tick();
        }
        self.auto_save();
        self.touch_mru();
        self.write_shared_context();
        let touched_recent = self.activity.touched.values().any(|(_, t)| t.elapsed() < Duration::from_secs(9));
        changed || touched_recent || self.doc().is_some_and(Doc::flash_active)
    }

    // ---- input ----

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
            Mode::AgentFind(bar) => {
                bar.paste(text);
                if let Some(a) = self.agents.iter_mut().find(|a| a.id == bar.agent_id) {
                    bar.update(a, None);
                }
            }
            Mode::Picker(p) => {
                p.paste(text);
                if p.kind == Kind::Files {
                    self.update_quick_open();
                }
            }
            Mode::Prompt { input, .. } => input.push_str(text.trim()),
            Mode::Find(bar) => {
                bar.paste(text);
                let (q, o) = (bar.query.clone(), bar.opts);
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    doc.set_search(&q, o);
                }
            }
            _ => match self.focus {
                Focus::Agent => {
                    if let Some(a) = self.agent() {
                        a.paste(text);
                    }
                }
                Focus::Editor if self.view.is_none() => {
                    if let Some(d) = self.docs.get_mut(self.groups[self.active_group].active) {
                        d.insert_text(text);
                    }
                }
                Focus::Editor => {
                    if let Some(View::Search(v)) = &mut self.view {
                        v.paste(text);
                    }
                }
                _ => {}
            },
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let key = keys::normalize(key);
        // Leader key: Ctrl+] then a key acts like Alt+key. Works on macOS
        // terminals where Option types characters instead of sending Alt.
        let is_leader = key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char(']' | '5'));
        if self.leader {
            self.leader = false;
            if is_leader {
                if self.focus == Focus::Agent {
                    if let Some(a) = self.agent() {
                        a.write(&[0x1d]);
                    }
                }
                return;
            }
            if key.code == KeyCode::Esc {
                return;
            }
            let mut modifiers = key.modifiers | KeyModifiers::ALT;
            modifiers.remove(KeyModifiers::CONTROL);
            if matches!(key.code, KeyCode::Char(_)) {
                modifiers = modifiers - KeyModifiers::SHIFT;
            }
            return self.on_key(KeyEvent::new(key.code, modifiers));
        }
        if is_leader && matches!(self.mode, Mode::Normal) {
            self.leader = true;
            return;
        }
        if !matches!(self.mode, Mode::Normal) {
            return self.on_mode_key(key);
        }
        // The search view's option toggles (Alt+c/w/r) and replace-all (Alt+a) win over globals.
        if self.focus == Focus::Editor
            && matches!(self.view, Some(View::Search(_)))
            && key.modifiers == KeyModifiers::ALT
            && matches!(key.code, KeyCode::Char('c' | 'w' | 'r' | 'a'))
        {
            return self.view_key(key);
        }
        match global_binding(&self.keymap, key) {
            Some(Some(action)) => return self.run(action),
            Some(None) => return self.focus_key(key),
            None => {}
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if alt && !ctrl {
            match key.code {
                KeyCode::Char('<' | ',') => return self.agent_pct = (self.agent_pct + 5).min(85),
                KeyCode::Char('>') => return self.agent_pct = self.agent_pct.saturating_sub(5).max(15),
                KeyCode::Char('?') if self.focus == Focus::Agent => return self.run(Action::AgentFind),
                _ => {}
            }
        }
        if self.focus != Focus::Agent && ctrl {
            match key.code {
                KeyCode::Char('q') => return self.run(Action::Quit),
                KeyCode::Char('p') => return self.run(Action::QuickOpen),
                _ => {}
            }
        }
        self.focus_key(key)
    }

    fn focus_key(&mut self, key: KeyEvent) {
        match self.focus {
            Focus::Tree => self.tree_key(key),
            Focus::Editor => {
                if self.view.is_some() {
                    self.view_key(key)
                } else {
                    self.editor_key(key)
                }
            }
            Focus::Agent => {
                self.ensure_agent_started();
                let restart = self.agent().is_some_and(|a| (a.exited || a.error.is_some()) && key.code == KeyCode::Enter);
                if restart {
                    let idx = self.active_agent();
                    self.agents[idx].reset();
                    self.start_agent(idx);
                } else if let Some(a) = self.agent() {
                    a.send_key(key);
                }
                if self.banner.as_ref().is_some_and(|b| b.error) {
                    self.banner = None;
                }
            }
        }
    }

    fn view_key(&mut self, key: KeyEvent) {
        let result = match &mut self.view {
            Some(View::Review(v)) => v.handle_key(key),
            Some(View::Tests(v)) => v.handle_key(key),
            Some(View::Activity(v)) => v.handle_key(key),
            Some(View::Search(v)) => v.handle_key(key),
            Some(View::AgentChanges(v)) => v.handle_key(key),
            Some(View::Compare(v)) => v.handle_key(key),
            Some(View::Transcript(v)) => v.handle_key(key),
            None => return,
        };
        self.on_view_result(result);
    }

    fn on_view_result(&mut self, result: ViewResult) {
        match result {
            ViewResult::None => {}
            ViewResult::RerunTests => self.run_tests(),
            ViewResult::Close => {
                let back = self.return_view.take().filter(|_| matches!(self.view, Some(View::Review(_))));
                self.view = back;
                self.refresh_agent_views();
            }
            ViewResult::Open(path, line) => {
                self.return_view = None;
                if !matches!(self.view, Some(View::Search(_))) {
                    self.view = None;
                }
                self.open_location(&path, Some(line), None, None);
                self.focus_doc_from_view();
            }
            ViewResult::OpenAt(path, line, col) => {
                self.open_location(&path, Some(line), Some(col), None);
                self.focus_doc_from_view();
            }
            ViewResult::Message(m, err) => {
                if err {
                    self.error(m)
                } else {
                    self.info(m)
                }
            }
            ViewResult::Changed(m) => {
                self.info(m);
                self.refresh_git();
                self.last_disk_check = Instant::now() - Duration::from_secs(1);
            }
            ViewResult::Resume(kind, id) => {
                self.view = None;
                self.accept(Target::Resume(kind, id), "", Kind::Static);
            }
            ViewResult::ReviewTurn(i) => {
                let changed = match &self.view {
                    Some(View::Activity(v)) => v.turn(i).map(Turn::changed_files),
                    _ => None,
                };
                if let Some(files) = changed {
                    self.open_review(Some(files));
                }
            }
            ViewResult::Review { root, title, files } => {
                let from = self.view.take();
                match ReviewView::new(title, Repo { root }, Some(files)) {
                    Ok(v) => {
                        if v.is_empty() {
                            self.info("nothing left to review in the diff (already accepted, rejected or committed)");
                        }
                        self.view = Some(View::Review(v));
                        self.return_view = from;
                        self.focus = Focus::Editor;
                    }
                    Err(e) => {
                        self.view = from;
                        self.error(e);
                    }
                }
            }
            ViewResult::ToggleReviewed { agent, path, version, reviewed } => {
                self.info(format!("{path}: {}", if reviewed { "marked unreviewed" } else { "marked reviewed" }));
                self.reviewed.insert((agent, path), changes::toggle(version, reviewed));
                self.refresh_agent_views();
            }
        }
    }

    fn tree_key(&mut self, key: KeyEvent) {
        if self.tree_mode == TreeMode::Outline {
            return self.outline_key(key.code);
        }
        let page = self.rects.tree.height.max(2) as isize - 1;
        match key.code {
            KeyCode::Char('a') => self.prompt_new(false),
            KeyCode::Char('A') => self.prompt_new(true),
            KeyCode::Char('r') | KeyCode::F(2) => self.prompt_rename(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_entry(),
            KeyCode::Char('y') => self.copy_path(false),
            KeyCode::Char('Y') => self.copy_path(true),
            KeyCode::Char('o') => self.reveal_in_os(),
            KeyCode::Up | KeyCode::Char('k') => self.tree.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.tree.move_by(1),
            KeyCode::PageUp => self.tree.move_by(-page),
            KeyCode::PageDown => self.tree.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.tree.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.tree.move_by(isize::MAX / 2),
            KeyCode::Left | KeyCode::Char('h') => self.tree.collapse(),
            KeyCode::Char('R') => {
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
                    self.pin_active_preview();
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Char(' ') => {
                if let Activate::Open(p) = self.tree.activate() {
                    self.open_preview(&p);
                }
            }
            KeyCode::Tab => self.tree_mode = TreeMode::Outline,
            KeyCode::Esc => self.focus = Focus::Editor,
            _ => {}
        }
    }

    fn editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('g') if ctrl => return self.run(Action::GoToLine),
            KeyCode::Char('K') if ctrl && !alt => return self.editor_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            KeyCode::Char('f') if ctrl => return self.run(Action::Find),
            KeyCode::Char('h') if ctrl => return self.run(Action::FindReplace),
            KeyCode::F(3) if shift => return self.find_step(false),
            KeyCode::F(3) => return self.run(Action::FindNext),
            // Ctrl+\ arrives as 0x1C, which crossterm reports as Ctrl+4.
            KeyCode::Char('\\' | '4') if ctrl && !alt => return self.run(Action::SplitEditor),
            KeyCode::Esc if self.doc().is_some_and(|d| d.search.is_some() && d.cursor_count() == 1 && !d.has_selection()) => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    doc.clear_search();
                }
                return;
            }
            KeyCode::F(2) => return self.run(Action::RenameSymbol),
            KeyCode::Char('h') if alt => return self.run(Action::Hover),
            KeyCode::Char('.') if alt => return self.run(Action::CodeActions),
            KeyCode::F(12) if shift => return self.run(Action::FindReferences),
            KeyCode::F(12) => return self.run(Action::GoToDefinition),
            KeyCode::Char('w') if ctrl => return self.run(Action::CloseFile),
            KeyCode::PageUp if ctrl => return self.run(Action::PrevFile),
            KeyCode::PageDown if ctrl => return self.run(Action::NextFile),
            KeyCode::Right if alt && shift => return self.run(Action::ExpandSelection),
            KeyCode::Left if alt && shift => return self.run(Action::ShrinkSelection),
            KeyCode::Left if alt => return self.run(Action::GoBack),
            KeyCode::Right if alt => return self.go_forward(),
            KeyCode::Char('s') if ctrl => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    match doc.save() {
                        Ok(()) => {
                            let (path, name) = (doc.path.clone(), doc.file_name());
                            self.lsp.did_save(&path);
                            self.info(format!("saved {name}"));
                            self.refresh_git();
                            self.refresh_marks();
                        }
                        Err(e) => self.error(format!("save failed: {e}")),
                    }
                }
                return;
            }
            _ => {}
        }
        let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else {
            if key.code == KeyCode::Tab || key.code == KeyCode::Esc {
                self.focus = Focus::Tree;
            }
            return;
        };
        if key.code == KeyCode::Char('m') && alt && crate::markdown::is_markdown(&doc.path) {
            doc.md_preview = !doc.md_preview;
            let msg = if doc.md_preview { "markdown preview (Alt+m to edit)" } else { "editing markdown source (Alt+m to preview)" };
            return self.info(msg);
        }
        if doc.md_preview {
            let page = self.rects.groups[self.active_group].height.saturating_sub(2) as usize;
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => doc.md_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => doc.md_scroll = doc.md_scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => doc.md_scroll += page,
                KeyCode::PageUp => doc.md_scroll = doc.md_scroll.saturating_sub(page),
                KeyCode::Home | KeyCode::Char('g') => doc.md_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => doc.md_scroll = usize::MAX / 2,
                KeyCode::Char('c') if ctrl => {}
                _ => self.info("markdown preview — press Alt+m to edit"),
            }
            return;
        }
        // Modal editing gets first refusal on every key; anything it does not
        // claim falls through to the ordinary bindings, so Ctrl+S, the arrows
        // and every Alt shortcut keep working in any mode.
        if self.settings.vim {
            let mut vim = std::mem::take(&mut self.vim);
            let verdict = vim.handle(doc, key);
            self.vim = vim;
            match verdict {
                vim::VimResult::Handled => return,
                vim::VimResult::Yanked(text) => {
                    if !text.is_empty() {
                        self.copy_to_host(&text);
                    }
                    return;
                }
                vim::VimResult::Ex(cmd) => return self.vim_ex(&cmd),
                vim::VimResult::Search(query, forward) => {
                    if let Some(doc) = self.doc_mut() {
                        doc.set_search(&query, SearchOpts::default());
                        doc.find_step(forward);
                    }
                    return;
                }
                vim::VimResult::PassThrough => {}
            }
        }
        let result = doc.handle_key(key);
        if doc.dirty {
            doc.preview = false;
        }
        match key.code {
            KeyCode::Char('(' | ',') if !ctrl && !alt => self.signature_help(),
            KeyCode::Char(')') | KeyCode::Esc | KeyCode::Enter => self.popup = None,
            _ if self.popup.as_ref().is_some_and(|p| p.kind == PopupKind::Hover) => self.popup = None,
            _ => {}
        }
        match result {
            KeyResult::Copy(text) => {
                self.copy_to_host(&text);
                self.info("copied to clipboard");
            }
            KeyResult::Message(m) if m.starts_with("save failed") => self.error(m),
            KeyResult::Message(m) => self.info(m),
            KeyResult::Handled | KeyResult::Ignored => {}
        }
    }

    /// Carry out a `:` command. Only the ones that map onto something NOIDA
    /// already does; anything else says so rather than failing silently.
    fn vim_ex(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if let Ok(line) = cmd.parse::<usize>() {
            if let Some(doc) = self.doc_mut() {
                doc.goto(line.saturating_sub(1), None, None);
            }
            return;
        }
        match cmd.trim_end_matches('!') {
            "w" => self.run(Action::Save),
            "wa" => self.run(Action::SaveAll),
            "q" => self.run(if cmd.ends_with('!') { Action::CloseFile } else { Action::CloseFile }),
            "wq" | "x" => {
                self.run(Action::Save);
                self.run(Action::CloseFile);
            }
            "qa" => self.run(Action::Quit),
            "e" => self.run(Action::ReopenClosedFile),
            other => self.error(format!("not a NOIDA command: :{other}")),
        }
    }

    fn copy_to_host(&mut self, text: &str) {
        self.host_out.extend_from_slice(b"\x1b]52;c;");
        self.host_out.extend_from_slice(base64(text.as_bytes()).as_bytes());
        self.host_out.push(0x07);
    }

    fn on_mode_key(&mut self, key: KeyEvent) {
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Normal => {}
            Mode::Hints { mut typed } => {
                let all: Vec<FileRef> = self.refs[0].iter().chain(self.refs[1].iter()).cloned().collect();
                let total = all.len();
                match key.code {
                    KeyCode::Enter => {
                        let newest = self.refs[if self.split { self.active_slot } else { 0 }].last().cloned().or(all.last().cloned());
                        if let Some(r) = newest {
                            self.open_ref(&r);
                        }
                    }
                    KeyCode::Char(c) if HINT_KEYS.contains(c) => {
                        typed.push(c);
                        if let Some(i) = (0..total).find(|&i| hint_label(i, total) == typed) {
                            self.open_ref(&all[i]);
                        } else if (0..total).any(|i| hint_label(i, total).starts_with(&typed)) {
                            self.mode = Mode::Hints { typed };
                        }
                    }
                    _ => {}
                }
            }
            Mode::Picker(mut p) => match p.handle_key(key) {
                Outcome::Cancel => {}
                Outcome::Accept(target) => {
                    let query = p.query.clone();
                    let kind = p.kind;
                    self.accept(target, &query, kind);
                }
                Outcome::QueryChanged => {
                    let files = p.kind == Kind::Files;
                    self.mode = Mode::Picker(p);
                    if files {
                        self.update_quick_open();
                    }
                }
                Outcome::None => self.mode = Mode::Picker(p),
            },
            Mode::Find(bar) => self.on_find_key(bar, key),
            Mode::AgentFind(bar) => self.on_agent_find_key(bar, key),
            Mode::Prompt { kind, mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit_prompt(kind, input),
                KeyCode::Backspace => {
                    input.pop();
                    self.mode = Mode::Prompt { kind, input };
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.mode = Mode::Prompt { kind, input: String::new() };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.push(c);
                    self.mode = Mode::Prompt { kind, input };
                }
                _ => self.mode = Mode::Prompt { kind, input },
            },
        }
    }

    fn submit_prompt(&mut self, kind: PromptKind, input: String) {
        match kind {
            PromptKind::GotoLine => {
                let mut parts = input.split(':').map(|s| s.trim().parse::<usize>().ok());
                if let (Some(Some(line)), Some(doc)) = (parts.next(), self.docs.get_mut(self.groups[self.active_group].active)) {
                    doc.goto(line, parts.next().flatten(), None);
                }
            }
            PromptKind::NewFile => self.create_entry(&input, false),
            PromptKind::NewFolder => self.create_entry(&input, true),
            PromptKind::Rename => self.rename_entry(&input),
            PromptKind::RenameSymbol => self.rename_symbol(&input),
            PromptKind::RenameAgent => {
                let idx = self.active_agent();
                if let Some(a) = self.agents.get_mut(idx) {
                    let name = input.trim();
                    if name.is_empty() {
                        a.renamed = false;
                        a.title = None;
                    } else {
                        a.renamed = true;
                        a.title = Some(name.to_string());
                    }
                }
            }
            PromptKind::NewBranch => self.git_op(|r| r.create_branch(input.trim()).map(|_| format!("switched to new branch {}", input.trim()))),
            PromptKind::Commit => {
                if input.trim().is_empty() {
                    return self.error("empty commit message");
                }
                self.git_op(|r| r.commit(input.trim()));
            }
            PromptKind::WorktreeName(kind) => self.new_worktree_agent(kind, input.trim()),
            PromptKind::SendToPair(a, b) => self.send_to_pair(a, b, &input),
        }
    }

    fn git_op(&mut self, f: impl FnOnce(&Repo) -> anyhow::Result<String>) {
        let Some(repo) = self.repo.clone() else { return self.error("not a git repository") };
        match f(&repo) {
            Ok(msg) => self.info(msg),
            Err(e) => self.error(format!("{e:#}")),
        }
        self.refresh_git();
        self.last_disk_check = Instant::now() - Duration::from_secs(1);
    }

    /// Opening a result from the search view keeps the view around (Alt+/ returns to it).
    fn focus_doc_from_view(&mut self) {
        if let Some(View::Search(_)) = &self.view {
            let view = self.view.take();
            self.parked_search = view;
        }
        self.focus = Focus::Editor;
    }

    fn open_find(&mut self, replace: bool) {
        let Some(doc) = self.doc() else { return self.info("open a file first") };
        let selected = doc.selected_text().filter(|t| !t.contains('\n') && doc.cursor_count() == 1);
        let (query, opts) = match (selected, &self.last_search) {
            (Some(t), Some((_, o))) => (t, *o),
            (Some(t), None) => (t, SearchOpts::default()),
            (None, Some((q, o))) => (q.clone(), *o),
            (None, None) => (String::new(), SearchOpts::default()),
        };
        // In-selection search makes sense when a multi-line selection exists.
        let multi_line = doc.has_selection() && doc.selected_lines().0 != doc.selected_lines().1;
        let opts = SearchOpts { in_selection: multi_line, ..opts };
        let mut bar = FindBar::new(query.clone(), opts, replace);
        if replace && !query.is_empty() {
            bar.focus_replace();
        }
        if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
            doc.set_search(&query, opts);
        }
        self.view = None;
        self.focus = Focus::Editor;
        self.mode = Mode::Find(bar);
    }

    fn find_step(&mut self, forward: bool) {
        let last = self.last_search.clone();
        let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return };
        if doc.search.is_none() {
            match last {
                Some((q, o)) => doc.set_search(&q, o),
                None => return self.open_find(false),
            }
        }
        if !doc.find_step(forward) {
            self.error("no matches");
        }
    }

    fn on_find_key(&mut self, mut bar: FindBar, key: KeyEvent) {
        let result = bar.handle_key(key);
        let (query, opts, replacement) = (bar.query.clone(), bar.opts, bar.replace.clone());
        let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return };
        match result {
            FindResult::Close => {
                if !query.is_empty() {
                    self.last_search = Some((query, opts));
                }
                return;
            }
            FindResult::Changed => doc.set_search(&query, opts),
            FindResult::Next => {
                doc.find_step(true);
            }
            FindResult::Prev => {
                doc.find_step(false);
            }
            FindResult::ReplaceOne => {
                doc.replace_current(&replacement);
            }
            FindResult::ReplaceAll => {
                let n = doc.replace_all(&replacement);
                self.info(format!("replaced {n} occurrences"));
            }
            FindResult::SelectAll => {
                let n = doc.select_all_matches();
                self.info(format!("{n} cursors"));
                self.last_search = Some((query, opts));
                return;
            }
            FindResult::None => {}
        }
        self.last_search = Some((bar.query.clone(), bar.opts));
        self.mode = Mode::Find(bar);
    }

    fn open_agent_find(&mut self) {
        let idx = self.active_agent();
        let Some(id) = self.agents.get(idx).filter(|a| a.started()).map(|a| a.id) else { return self.info("no agent output to search") };
        if self.zoom && self.focus != Focus::Agent {
            self.zoom = false;
        }
        self.set_focus(Focus::Agent);
        self.mode = Mode::AgentFind(AgentFindBar::new(id));
    }

    fn on_agent_find_key(&mut self, mut bar: AgentFindBar, key: KeyEvent) {
        let result = bar.handle_key(key);
        let Some(agent) = self.agents.iter_mut().find(|a| a.id == bar.agent_id) else { return };
        match result {
            FindResult::Close => {
                agent.find_match = None;
                agent.parser.screen_mut().set_scrollback(0);
                return;
            }
            FindResult::Changed => bar.update(agent, None),
            FindResult::Next => bar.update(agent, Some(true)),
            FindResult::Prev => bar.update(agent, Some(false)),
            _ => {}
        }
        self.mode = Mode::AgentFind(bar);
    }

    /// Clicking ends an agent find but keeps the viewport, so the match can be selected.
    fn end_agent_find(&mut self) {
        if let Mode::AgentFind(bar) = &self.mode {
            let id = bar.agent_id;
            if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
                a.find_match = None;
            }
            self.mode = Mode::Normal;
        }
    }

    fn drag_agent_selection(&mut self, x: u16, y: u16) {
        let Some(press) = self.agent_press else { return };
        let area = self.rects.agents[press.slot];
        if area.width == 0 || area.height == 0 {
            return;
        }
        let row = y.clamp(area.y, area.y + area.height - 1) - area.y;
        let col = x.clamp(area.x, area.x + area.width - 1) - area.x;
        if !press.moved && (row, col) == (press.row, press.col) {
            return;
        }
        self.agent_press = Some(AgentPress { moved: true, ..press });
        if let Some(a) = self.agents.get_mut(self.slots[press.slot]) {
            let head = (a.line_at(row), col);
            a.selection = Some((press.anchor, head));
        }
    }

    fn finish_agent_selection(&mut self) {
        let Some(press) = self.agent_press.take() else { return };
        if !press.moved {
            // A plain click on a highlighted reference still opens it.
            if let Some(r) = self.refs[press.slot].iter().find(|r| r.contains(press.row, press.col)).cloned() {
                self.open_ref(&r);
            }
            return;
        }
        let text = self.agents.get_mut(self.slots[press.slot]).and_then(Agent::selection_text).unwrap_or_default();
        if text.is_empty() {
            return;
        }
        self.copy_to_host(&text);
        self.info(format!("copied {} characters", text.chars().count()));
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.popup = None;
        }
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let r = &self.rects;
        let (tree, editor, tree_block, main) = (r.tree, r.editor, r.tree_block, r.main);
        let agent_areas = r.agents;
        let agent_blocks = r.agent_blocks;
        let agent_area = r.agent_area;
        let slot_at = (0..2).find(|&s| agent_areas[s].width > 0 && inside(agent_areas[s]));
        let group_blocks = r.group_blocks;
        let group_at = (0..self.groups.len()).find(|&g| group_blocks[g].width > 0 && inside(group_blocks[g]));

        self.hover = slot_at.map(|s| (s, y - agent_areas[s].y, x - agent_areas[s].x));

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.end_agent_find();
                for a in &mut self.agents {
                    a.selection = None;
                }
                if !matches!(self.mode, Mode::Normal | Mode::Find(_)) {
                    self.mode = Mode::Normal;
                }
                let title_slot = (0..2).find(|&s| agent_blocks[s].width > 0 && y == agent_blocks[s].y && inside(agent_blocks[s]));
                if agent_area.width > 0 && x == agent_area.x && y > agent_area.y {
                    self.drag = Drag::AgentDivider;
                } else if tree_block.width > 0 && x + 1 == tree_block.x + tree_block.width && y > tree_block.y {
                    self.drag = Drag::TreeDivider;
                } else if self.split && y == agent_blocks[1].y && inside(agent_blocks[1]) && x < agent_blocks[1].x + 2 {
                    self.drag = Drag::SplitDivider;
                } else if let Some(slot) = title_slot.filter(|&s| self.rects.agent_new[s].is_some_and(|nx| x >= nx && x < nx + 3)) {
                    self.active_slot = slot;
                    let kind = self.agents.get(self.slots[slot]).and_then(|a| a.kind).unwrap_or(AgentKind::Claude);
                    self.run(Action::NewAgent(kind));
                } else if let Some(i) = title_slot.and_then(|s| self.rects.agent_closes[s].iter().find(|(cx, _)| *cx == x).map(|c| c.1)) {
                    self.close_agent_at(i);
                } else if let Some(slot) = title_slot {
                    let hit = self.rects.agent_tabs[slot].iter().find(|(a, b, _)| x >= *a && x < *b).map(|t| t.2);
                    if let Some(i) = hit {
                        self.assign_slot(slot, i);
                    }
                    self.active_slot = slot;
                    self.set_focus(Focus::Agent);
                } else if y == self.rects.editor_block.y && inside(self.rects.editor_block) {
                    let g = group_at.unwrap_or(self.active_group);
                    self.active_group = g;
                    let hit = self.rects.doc_tabs[g].iter().find(|(a, b, _)| x >= *a && x < *b).map(|t| t.2);
                    if let Some(i) = hit {
                        self.view = None;
                        self.set_active_doc(i);
                        let p = self.docs[i].path.clone();
                        self.tree.reveal(&p);
                        self.refresh_marks();
                    }
                    self.focus = Focus::Editor;
                } else if y == tree_block.y && inside(tree_block) {
                    self.tree_mode = if x < tree_block.x + 9 { TreeMode::Files } else { TreeMode::Outline };
                    self.focus = Focus::Tree;
                } else if inside(tree) {
                    self.focus = Focus::Tree;
                    if self.tree_mode == TreeMode::Outline {
                        self.outline_click(tree, y);
                    } else if let Some(i) = self.tree.row_at(tree, y) {
                        let double = self.tree_double_click(i);
                        self.tree.selected = i;
                        if let Activate::Open(p) = self.tree.activate() {
                            self.open_preview(&p);
                            if double {
                                self.pin_active_preview();
                                self.focus = Focus::Editor;
                            }
                        }
                    }
                } else if inside(editor) {
                    self.focus = Focus::Editor;
                    match &mut self.view {
                        Some(View::Review(v)) => v.click(y),
                        Some(View::Tests(v)) => {
                            let r = v.click(y);
                            self.on_view_result(r);
                        }
                        Some(View::Activity(v)) => {
                            let r = v.click(y);
                            self.on_view_result(r);
                        }
                        Some(View::Search(v)) => {
                            let r = v.click(y);
                            self.on_view_result(r);
                        }
                        Some(View::AgentChanges(v)) => {
                            let r = v.click(y);
                            self.on_view_result(r);
                        }
                        Some(View::Compare(v)) => {
                            let r = v.click(x, y);
                            self.on_view_result(r);
                        }
                        Some(View::Transcript(_)) => {}
                        None => {
                            // Clicking another group focuses it. When both groups show the same
                            // doc its stored layout belongs to the previously focused group, so
                            // that click only moves focus (the next render re-maps the doc).
                            let Some(g) = group_at else { return };
                            let switched = g != self.active_group;
                            let shared = self.groups.iter().all(|grp| grp.active == self.groups[g].active);
                            if switched {
                                self.active_group = g;
                                self.refresh_marks();
                            }
                            if (switched && shared) || !inside(self.rects.groups[g]) {
                                return;
                            }
                            if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                                let alt = m.modifiers.contains(KeyModifiers::ALT);
                                let shift = m.modifiers.contains(KeyModifiers::SHIFT);
                                let click = doc.click(x, y, shift && !alt, alt && !shift);
                                if matches!(click, Click::Text) {
                                    self.drag = Drag::Editor;
                                    if m.modifiers.contains(KeyModifiers::CONTROL) {
                                        self.run(Action::GoToDefinition);
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(slot) = slot_at {
                    let (row, col) = (y - agent_areas[slot].y, x - agent_areas[slot].x);
                    self.active_slot = slot;
                    let hit = self.refs[slot].iter().find(|r| r.contains(row, col)).cloned();
                    let idx = self.slots[slot];
                    let select = self.agents.get(idx).is_some_and(|a| a.started() && !a.wants_mouse());
                    if select {
                        // Decide on release: a click opens a reference, a drag selects text.
                        let anchor = (self.agents[idx].line_at(row), col);
                        self.agent_press = Some(AgentPress { slot, row, col, anchor, moved: false });
                        self.drag = Drag::AgentSelect;
                        if hit.is_none() {
                            self.set_focus(Focus::Agent);
                        }
                    } else if let Some(r) = hit {
                        self.open_ref(&r);
                    } else {
                        self.set_focus(Focus::Agent);
                        let idx = self.active_agent();
                        if let Some(a) = self.agents.get_mut(idx) {
                            a.forward_mouse(m, agent_areas[slot]);
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Middle) if (0..2).any(|s| self.rects.agent_blocks[s].width > 0 && y == self.rects.agent_blocks[s].y) => {
                let slot = (0..2).find(|&s| y == self.rects.agent_blocks[s].y).unwrap_or(0);
                if let Some(i) = self.rects.agent_tabs[slot].iter().find(|(a, b, _)| x >= *a && x < *b).map(|t| t.2) {
                    self.close_agent_at(i);
                }
            }
            MouseEventKind::Down(MouseButton::Middle) if y == self.rects.editor_block.y => {
                let g = group_at.unwrap_or(self.active_group);
                if let Some(i) = self.rects.doc_tabs[g].iter().find(|(a, b, _)| x >= *a && x < *b).map(|t| t.2) {
                    self.active_group = g;
                    self.set_active_doc(i);
                    self.run(Action::CloseFile);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Drag::Editor => {
                    if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                        let column = m.modifiers.contains(KeyModifiers::ALT);
                        doc.drag(x, y, column);
                    }
                }
                Drag::AgentDivider if main.width > 0 => {
                    let right = main.x + main.width;
                    self.agent_pct = ((right.saturating_sub(x)) as u32 * 100 / main.width as u32).clamp(15, 85) as u16;
                }
                Drag::TreeDivider => self.tree_width = (x.saturating_sub(main.x) + 1).clamp(12, 80),
                Drag::AgentSelect => self.drag_agent_selection(x, y),
                Drag::SplitDivider if agent_area.height > 0 => {
                    self.split_pct = ((y.saturating_sub(agent_area.y)) as u32 * 100 / agent_area.height as u32).clamp(15, 85) as u16;
                }
                _ => {
                    if let Some(slot) = slot_at {
                        let idx = self.slots[slot];
                        if let Some(a) = self.agents.get_mut(idx) {
                            a.forward_mouse(m, agent_areas[slot]);
                        }
                    }
                }
            },
            MouseEventKind::Up(_) => {
                if std::mem::replace(&mut self.drag, Drag::None) == Drag::AgentSelect {
                    return self.finish_agent_selection();
                }
                if let Some(slot) = slot_at {
                    let idx = self.slots[slot];
                    if let Some(a) = self.agents.get_mut(idx) {
                        a.forward_mouse(m, agent_areas[slot]);
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta: isize = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                if inside(tree) {
                    self.tree.scroll(delta);
                } else if inside(editor) {
                    match &mut self.view {
                        Some(View::Review(v)) => v.scroll_by(delta),
                        Some(View::Activity(v)) => v.scroll_by(delta),
                        Some(View::Search(v)) => v.scroll_by(delta),
                        Some(View::AgentChanges(v)) => v.scroll_by(delta),
                        Some(View::Compare(v)) => v.scroll_by(delta),
                        Some(View::Transcript(v)) => v.scroll_by(delta),
                        Some(View::Tests(v)) => v.scroll_by(delta),
                        None => {
                            let g = group_at.unwrap_or(self.active_group);
                            if let Some(doc) = self.docs.get_mut(self.groups[g].active) {
                                if doc.md_preview {
                                    doc.md_scroll = (doc.md_scroll as isize + delta).max(0) as usize;
                                } else {
                                    doc.scroll(delta);
                                }
                            }
                        }
                    }
                } else if let Some(slot) = slot_at {
                    let idx = self.slots[slot];
                    if let Some(a) = self.agents.get_mut(idx) {
                        if !a.forward_mouse(m, agent_areas[slot]) {
                            a.scroll(-delta);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---- commands ----

    fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        if f == Focus::Agent {
            self.ensure_agent_started();
            let idx = self.active_agent();
            if let Some(a) = self.agents.get_mut(idx) {
                a.seen();
            }
        }
    }

    fn assign_slot(&mut self, slot: usize, agent: usize) {
        if self.split && self.slots[1 - slot] == agent {
            self.slots.swap(0, 1);
        } else {
            self.slots[slot] = agent;
        }
        self.agents[agent].seen();
    }

    pub fn run(&mut self, action: Action) {
        match action {
            Action::CommandPalette => self.open_palette(),
            Action::Keys => self.open_keys(),
            Action::QuickOpen => {
                if self.index_built.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                    self.rebuild_index();
                }
                self.mode = Mode::Picker(Picker::new("Open File", Kind::Files, Vec::new()));
                self.update_quick_open();
            }
            Action::GoToLine => self.mode = Mode::Prompt { kind: PromptKind::GotoLine, input: String::new() },
            Action::Find => self.open_find(false),
            Action::FindReplace => self.open_find(true),
            Action::SearchWorkspace if self.parked_search.is_some() && self.view.is_none() && !self.doc().is_some_and(Doc::has_selection) => {
                self.view = self.parked_search.take();
                self.focus = Focus::Editor;
            }
            Action::SearchWorkspace | Action::ReplaceWorkspace => {
                self.parked_search = None;
                let query = self.doc().and_then(|d| d.selected_text().filter(|t| !t.contains('\n')).or_else(|| d.word_at_cursor().filter(|_| !d.has_selection()))).unwrap_or_default();
                let query = if matches!(self.view, Some(View::Search(_))) { String::new() } else { query };
                self.view = Some(View::Search(SearchView::new(self.root.clone(), self.tx.clone(), query, action == Action::ReplaceWorkspace)));
                self.focus = Focus::Editor;
            }
            Action::FindNext => self.find_step(true),
            Action::FindPrev => self.find_step(false),
            Action::ToggleWordWrap => {
                self.settings.word_wrap = !self.settings.word_wrap;
                let wrap = self.settings.word_wrap;
                for d in &mut self.docs {
                    d.wrap = wrap;
                }
                self.info(if wrap { "word wrap on" } else { "word wrap off" });
            }
            Action::Fold | Action::Unfold => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    let line = doc.cursor_line0();
                    let folded = doc.is_folded(line);
                    if (action == Action::Fold) != folded || action == Action::Fold {
                        doc.toggle_fold(line);
                    }
                }
            }
            Action::FoldAll => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    doc.fold_all();
                }
            }
            Action::UnfoldAll => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    doc.unfold_all();
                }
            }
            Action::ToggleComment => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    if !doc.toggle_comment() {
                        self.info("no comment syntax known for this file type");
                    }
                }
            }
            Action::SelectAllOccurrences => {
                let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return };
                let (query, word) = match doc.selected_text() {
                    Some(t) if !t.contains('\n') => (t, false),
                    _ => match doc.word_at_cursor() {
                        Some(w) => (w, true),
                        None => return,
                    },
                };
                doc.set_search(&query, SearchOpts { case: true, word, ..Default::default() });
                let n = doc.select_all_matches();
                doc.clear_search();
                self.info(format!("{n} cursors"));
            }
            Action::AddCursorAbove | Action::AddCursorBelow => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    doc.add_cursor_vertical(action == Action::AddCursorBelow);
                }
            }
            Action::OpenSettings => {
                match settings::ensure_file() {
                    Some(p) => self.open_path(&p, None),
                    None => self.error("cannot locate the config directory"),
                }
                self.focus = Focus::Editor;
            }
            Action::CloseOtherFiles => {
                let keep = self.active_doc();
                self.close_where("others", |i, _| i != keep);
            }
            Action::CloseAllFiles => self.close_where("all", |_, _| true),
            Action::CloseFilesToRight => {
                let keep = self.active_doc();
                self.close_where("right", |i, _| i > keep);
            }
            Action::PinFile => self.toggle_pin(),
            Action::ReopenClosedFile => self.reopen_closed(),
            Action::RecentFiles => self.recent_files(),
            Action::RecentLocations => self.recent_locations(),
            Action::NewFile => self.prompt_new(false),
            Action::NewFolder => self.prompt_new(true),
            Action::RenamePath => self.prompt_rename(),
            Action::DeletePath => self.delete_entry(),
            Action::CopyRelativePath => self.copy_path(false),
            Action::CopyAbsolutePath => self.copy_path(true),
            Action::RevealInOs => self.reveal_in_os(),
            Action::ToggleOutline => {
                self.show_tree = true;
                self.tree_mode = if self.tree_mode == TreeMode::Files { TreeMode::Outline } else { TreeMode::Files };
                self.set_focus(Focus::Tree);
            }
            Action::SendSymbol => self.send_symbol(),
            Action::ToggleShareContext => {
                self.settings.share_editor_context = !self.settings.share_editor_context;
                self.info(if self.settings.share_editor_context {
                    "agents now see which file and lines you're looking at when you prompt"
                } else {
                    "stopped sharing editor context with agents"
                });
            }
            Action::ToggleMarkdownPreview => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    if crate::markdown::is_markdown(&doc.path) {
                        doc.md_preview = !doc.md_preview;
                    } else {
                        self.info("not a markdown file");
                    }
                }
            }
            Action::Hover => self.hover(),
            Action::RenameSymbol => self.prompt_rename_symbol(),
            Action::CodeActions => self.code_actions(None),
            Action::FormatDocument => self.format(false),
            Action::FormatSelection => self.format(true),
            Action::OrganizeImports => self.code_actions(Some("source.organizeImports")),
            Action::AskMenu => self.ask_menu(),
            Action::ToggleTheme => {
                self.settings.theme = if crate::theme::is_light() { "dark".into() } else { "light".into() };
                settings::apply(&self.settings);
                self.info(format!("theme: {}", self.settings.theme));
            }
            Action::Save => self.editor_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Action::SaveAll => {
                let mut n = 0;
                for d in self.docs.iter_mut().filter(|d| d.dirty) {
                    if d.save().is_ok() {
                        n += 1;
                    }
                }
                self.info(format!("saved {n} files"));
                self.refresh_git();
            }
            Action::CloseFile => self.close_doc(),
            Action::SplitEditor => self.split_editor(),
            Action::FocusOtherEditorGroup => self.focus_other_group(),
            Action::CloseEditorGroup => self.close_group(),
            Action::MoveTabToOtherGroup => self.move_tab_to_other_group(),
            Action::NextFile => self.cycle_doc(1),
            Action::PrevFile => self.cycle_doc(-1),
            Action::ToggleTree => {
                self.show_tree = !self.show_tree;
                if !self.show_tree && self.focus == Focus::Tree {
                    self.focus = Focus::Editor;
                }
            }
            Action::Zoom => self.zoom = !self.zoom,
            Action::GoBack => self.go_back(),
            Action::JumpToRef => self.start_hints(),
            Action::FocusTree => {
                self.show_tree = true;
                self.set_focus(Focus::Tree);
            }
            Action::FocusEditor => self.set_focus(Focus::Editor),
            Action::FocusAgent => self.set_focus(Focus::Agent),
            Action::SendSelection => self.send_context(None, true),
            Action::SendFile => self.send_context(None, false),
            Action::Ask(ask) => self.send_context(Some(ask.prompt()), true),
            Action::NextAgent => {
                if self.agents.len() > 1 {
                    let slot = if self.split { self.active_slot } else { 0 };
                    let mut next = (self.slots[slot] + 1) % self.agents.len();
                    if self.split && next == self.slots[1 - slot] && self.agents.len() > 2 {
                        next = (next + 1) % self.agents.len();
                    }
                    self.assign_slot(slot, next);
                }
                self.set_focus(Focus::Agent);
            }
            Action::NewAgent(kind) => {
                let command = match kind {
                    AgentKind::Shell => std::env::var("SHELL").unwrap_or_else(|_| "bash".into()),
                    k => k.name().to_string(),
                };
                let name = self.unique_name(kind.name());
                let root = self.root.clone();
                let idx = self.push_agent(&name, &command, root);
                self.show_agent(idx);
            }
            Action::CloseAgent => self.close_agent(),
            Action::RestartAgent => {
                let idx = self.active_agent();
                if let Some(a) = self.agents.get_mut(idx) {
                    a.kill();
                    a.reset();
                    self.start_agent(idx);
                    self.info("agent restarted");
                }
            }
            Action::ToggleSplit => {
                if self.agents.len() < 2 {
                    self.run(Action::NewAgent(AgentKind::Shell));
                }
                self.split = !self.split;
                if self.split && self.slots[0] == self.slots[1] {
                    self.slots[1] = (self.slots[0] + 1) % self.agents.len();
                }
                self.active_slot = 0;
                self.start_visible_agents();
            }
            Action::OtherAgentPane => {
                if self.split {
                    self.active_slot = 1 - self.active_slot;
                }
                self.set_focus(Focus::Agent);
            }
            Action::AgentFind => self.open_agent_find(),
            Action::AgentCopyScreen => {
                let idx = self.active_agent();
                let Some(text) = self.agents.get(idx).filter(|a| a.started()).map(Agent::screen_text) else { return self.info("no agent output to copy") };
                self.copy_to_host(&text);
                self.info(format!("copied {} characters", text.chars().count()));
            }
            Action::RunTests => self.run_tests(),
            Action::ReviewChanges => {
                let only = self.banner.as_ref().filter(|b| !b.changed.is_empty()).map(|b| b.changed.clone());
                self.open_review(only);
            }
            Action::AcceptAllChanges => self.git_op(|r| r.stage_all().map(|_| "staged all changes".to_string())),
            Action::SwitchBranch => {
                let Some(repo) = &self.repo else { return self.error("not a git repository") };
                match repo.branches() {
                    Ok(branches) => {
                        let items = branches
                            .into_iter()
                            .map(|(b, current)| Item::new(b.clone(), if current { "current" } else { "" }, Target::Branch(b)))
                            .collect();
                        self.mode = Mode::Picker(Picker::new("Switch Branch", Kind::Static, items));
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            Action::NewBranch => self.mode = Mode::Prompt { kind: PromptKind::NewBranch, input: String::new() },
            Action::Commit => self.mode = Mode::Prompt { kind: PromptKind::Commit, input: String::new() },
            Action::GitLog => {
                let Some(repo) = &self.repo else { return self.error("not a git repository") };
                match repo.log(git::LOG_LIMIT) {
                    Ok(log) if log.is_empty() => self.info("no commits yet"),
                    Ok(log) => self.mode = Mode::Picker(Picker::new("Git Log", Kind::Static, commit_items(log)).ordered()),
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            Action::FileHistory => {
                let Some(repo) = &self.repo else { return self.error("not a git repository") };
                let Some(path) = self.doc().map(|d| d.path.clone()) else { return self.info("open a file first") };
                let Ok(rel) = path.strip_prefix(&repo.root) else { return self.error("file is outside the repository") };
                let rel = rel.to_string_lossy().into_owned();
                match repo.file_history(&rel, git::LOG_LIMIT) {
                    Ok(log) if log.is_empty() => self.info(format!("no commits touch {rel}")),
                    Ok(log) => self.mode = Mode::Picker(Picker::new(format!("History: {rel}"), Kind::Static, commit_items(log)).ordered()),
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            Action::AgentActivity => {
                self.view = Some(View::Activity(ActivityView::new("Agent Activity".into(), self.root.clone(), self.activity.turns.clone())));
                self.focus = Focus::Editor;
            }
            Action::AgentHistory => {
                self.history = self.activity.history();
                let items = self
                    .history
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let when = sessions::ago(std::time::UNIX_EPOCH + Duration::from_secs(t.started));
                        Item::new(format!("{} · {}", t.agent, t.prompt.split_whitespace().collect::<Vec<_>>().join(" ")), format!("{} · {when}", t.summary()), Target::History(i))
                    })
                    .collect();
                self.mode = Mode::Picker(Picker::new("Agent History", Kind::Static, items).ordered());
            }
            Action::Handoff { from, to } => self.handoff(from, to),
            Action::FixFindings { reviewer, fixer } => self.fix_findings(reviewer, fixer),
            Action::AgentChanges => {
                self.return_view = None;
                self.view = Some(View::AgentChanges(AgentChangesView::new(self.change_groups())));
                self.focus = Focus::Editor;
            }
            Action::CompareAgents => self.pick_pair("Compare Two Agents", |a, b| Action::ComparePair { a, b }),
            Action::ComparePair { a, b } => {
                if a >= self.agents.len() || b >= self.agents.len() {
                    return;
                }
                let mut sides = [self.compare_side(a), self.compare_side(b)];
                agent_views::mark_shared(&mut sides);
                self.return_view = None;
                self.view = Some(View::Compare(CompareView::new(sides)));
                self.focus = Focus::Editor;
            }
            Action::RunInBoth => self.pick_pair("Send Same Prompt to Two Agents", |a, b| Action::SendToPair { a, b }),
            Action::SendToPair { a, b } => self.mode = Mode::Prompt { kind: PromptKind::SendToPair(a, b), input: String::new() },
            Action::RenameAgent => {
                let idx = self.active_agent();
                let Some(a) = self.agents.get(idx) else { return };
                let input = a.title.clone().unwrap_or_else(|| a.name.clone());
                self.mode = Mode::Prompt { kind: PromptKind::RenameAgent, input };
            }
            Action::BrowseHistory => {
                self.mode = Mode::Picker(Picker::new("Past Conversations", Kind::Static, Vec::new()).ordered());
                self.info("loading past conversations…");
                let tx = self.tx.clone();
                let root = self.root.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Bg::Sessions(sessions::list(&root)));
                });
            }
            Action::Sessions => {
                self.mode = Mode::Picker(Picker::new("Sessions", Kind::Static, session_tab_items(&self.agents)).ordered());
                let tx = self.tx.clone();
                let root = self.root.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Bg::Sessions(sessions::list(&root)));
                });
            }
            Action::GoToSymbol => {
                let Some(doc) = self.doc() else { return self.info("open a file first") };
                let path = doc.path.clone();
                let items: Vec<Item> = symbols::outline(&path, &doc.text())
                    .into_iter()
                    .map(|s| Item::new(s.name, s.kind, Target::File { path: path.clone(), line: Some(s.line + 1), col: Some(s.col + 1) }).hint(format!("L{}", s.line + 1)))
                    .collect();
                if items.is_empty() {
                    return self.info("no symbols found (supported: Rust, TS/JS, Python, Go, Java)");
                }
                self.mode = Mode::Picker(Picker::new("Go to Symbol in File", Kind::Static, items).ordered());
            }
            Action::GoToProjectSymbol => {
                if self.symbols.symbols.is_empty() {
                    self.rebuild_symbols();
                    return self.info("indexing symbols…");
                }
                let items = self
                    .symbols
                    .symbols
                    .iter()
                    .take(50_000)
                    .map(|(p, s)| {
                        Item::new(s.name.clone(), format!("{} · {}:{}", s.kind, refs::relative(&self.root, p), s.line + 1), Target::File { path: p.clone(), line: Some(s.line + 1), col: Some(s.col + 1) })
                    })
                    .collect();
                self.mode = Mode::Picker(Picker::new("Go to Symbol in Project", Kind::Static, items));
            }
            Action::GoToDefinition => {
                let Some(doc) = self.doc() else { return };
                let Some(word) = doc.word_at_cursor() else { return self.info("no symbol at cursor") };
                let (path, line, col) = (doc.path.clone(), doc.cursor_line0(), doc.utf16_col());
                if self.lsp.request(Request::Definition, &path, line, col) {
                    self.pending_definition = Some(word);
                } else {
                    self.symbol_definition(&word);
                }
            }
            Action::FindReferences => {
                let Some(doc) = self.doc() else { return };
                let Some(word) = doc.word_at_cursor() else { return self.info("no symbol at cursor") };
                let (path, line, col) = (doc.path.clone(), doc.cursor_line0(), doc.utf16_col());
                if !self.lsp.request(Request::References, &path, line, col) {
                    let hits = symbols::find_references(&self.root, &self.file_index.files, &word, 2000);
                    let items = hits
                        .into_iter()
                        .map(|(p, l, c, text)| Item::new(format!("{}:{}", refs::relative(&self.root, &p), l + 1), text, Target::File { path: p, line: Some(l + 1), col: Some(c + 1) }))
                        .collect();
                    self.mode = Mode::Picker(Picker::new(format!("References: {word}"), Kind::Static, items).ordered());
                }
            }
            Action::ExpandSelection => {
                let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) else { return };
                let text = doc.text();
                match symbols::expand(&doc.path, &text, doc.selection_bytes()) {
                    Some(range) => doc.expand_to(range),
                    None => self.info("structural selection needs a supported language"),
                }
            }
            Action::ShrinkSelection => {
                if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                    doc.shrink_selection();
                }
            }
            Action::Problems => {
                let mut all: Vec<(&PathBuf, &lsp::Diagnostic)> = self.lsp.diagnostics.iter().flat_map(|(p, ds)| ds.iter().map(move |d| (p, d))).collect();
                all.sort_by_key(|(p, d)| (d.severity, (*p).clone(), d.line));
                let items = all
                    .into_iter()
                    .map(|(p, d)| {
                        let icon = if d.severity <= 1 { "✗" } else if d.severity == 2 { "⚠" } else { "ℹ" };
                        Item::new(format!("{icon} {}", d.message), format!("{}:{} {}", refs::relative(&self.root, p), d.line + 1, d.source), Target::File { path: p.clone(), line: Some(d.line + 1), col: Some(d.col + 1) })
                    })
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    return self.info("no problems reported (language servers report as files open)");
                }
                self.mode = Mode::Picker(Picker::new("Problems", Kind::Static, items).ordered());
            }
            Action::FixProblem => self.fix_problem(),
            Action::NewWorktreeAgent(kind) => {
                if self.repo.is_none() {
                    return self.error("worktrees need a git repository");
                }
                self.mode = Mode::Prompt { kind: PromptKind::WorktreeName(kind), input: String::new() };
            }
            Action::ApplyWorktree => {
                let idx = self.active_agent();
                let Some((path, _)) = self.agents.get(idx).and_then(|a| a.worktree.clone()) else {
                    return self.error("the active agent tab is not in a worktree");
                };
                self.git_op(|r| r.apply_worktree(&Repo { root: path }).map(|n| format!("applied {n} changed files from the worktree")));
            }
            Action::RemoveWorktree => {
                let idx = self.active_agent();
                let Some((path, branch)) = self.agents.get(idx).and_then(|a| a.worktree.clone()) else {
                    return self.error("the active agent tab is not in a worktree");
                };
                if !self.confirmed("remove-worktree") {
                    return self.error(format!("remove worktree {branch} and discard its changes? run again to confirm"));
                }
                self.agents[idx].kill();
                self.git_op(|r| r.remove_worktree(&path, &branch).map(|_| format!("removed worktree {branch}")));
                self.remove_agent(idx);
            }
            Action::Quit => self.request_quit(),
        }
    }

    fn open_palette(&mut self) {
        let mut items: Vec<Item> = Action::palette()
            .into_iter()
            .filter_map(|a| a.describe().map(|(label, key)| Item::new(label, "", Target::Action(a)).hint(self.shortcut_hint(a, key))))
            .collect();
        for (fi, from) in self.agents.iter().enumerate() {
            let Some(turn) = self.activity.last_turn(&from.name) else { continue };
            for (ti, to) in self.agents.iter().enumerate() {
                if ti != fi && to.kind.is_some() {
                    items.push(Item::new(format!("Handoff: Ask {} to Review {}'s Changes", to.name, from.name), turn.summary(), Target::Action(Action::Handoff { from: fi, to: ti })));
                    items.push(Item::new(format!("Handoff: Ask {} to Fix {}'s Review Findings", to.name, from.name), turn.summary(), Target::Action(Action::FixFindings { reviewer: fi, fixer: ti })));
                }
            }
        }
        self.mode = Mode::Picker(Picker::new("Command Palette", Kind::Static, items));
    }

    /// Every action with its settings.json id and current shortcut.
    fn open_keys(&mut self) {
        let items: Vec<Item> = std::iter::once(Action::CommandPalette)
            .chain(Action::palette())
            .filter_map(|a| Some((a, a.id()?)))
            .filter_map(|(a, id)| a.describe().map(|(label, key)| Item::new(label, id, Target::Action(a)).hint(self.shortcut_hint(a, key))))
            .collect();
        self.mode = Mode::Picker(Picker::new("Keyboard Shortcuts · ids for \"keys\" in settings.json", Kind::Static, items));
    }

    /// Custom bindings, then the built-in hint unless its key was disabled in settings.
    fn shortcut_hint(&self, action: Action, default: &str) -> String {
        let mut custom: Vec<String> = self.keymap.iter().filter(|(_, a)| **a == Some(action)).map(|(k, _)| keys::display(k)).collect();
        custom.sort();
        let disabled = keys::parse(default).is_some_and(|k| self.keymap.contains_key(&k));
        if !default.is_empty() && !disabled {
            custom.push(default.to_string());
        }
        if let Some(h) = keys::ctrl_shift_hint(action).filter(|h| keys::enhanced() && !self.keymap.contains_key(&keys::parse(h).unwrap_or_default())) {
            custom.push(h.to_string());
        }
        custom.join(", ")
    }

    fn accept(&mut self, target: Target, query: &str, kind: Kind) {
        match target {
            Target::File { path, line, col } => {
                let line = line.or_else(|| if kind == Kind::Files { refs::split_line_suffix(query).1 } else { None });
                self.view = None;
                self.open_location(&path, line, col, None);
                self.focus = Focus::Editor;
            }
            Target::Action(a) => self.run(a),
            Target::Branch(b) => self.git_op(|r| r.switch(&b).map(|_| format!("switched to {b}"))),
            Target::Commit { hash, paths } => {
                let Some(repo) = self.repo.clone() else { return self.error("not a git repository") };
                let short: String = hash.chars().take(7).collect();
                let title = match paths.first() {
                    Some(p) => format!("Commit {short} · {p}"),
                    None => format!("Commit {short}"),
                };
                match ReviewView::commit(title, repo, hash, paths) {
                    Ok(v) => {
                        self.view = Some(View::Review(v));
                        self.focus = Focus::Editor;
                    }
                    Err(e) => self.error(e),
                }
            }
            Target::History(i) => {
                if let Some(t) = self.history.get(i).cloned() {
                    self.view = Some(View::Activity(ActivityView::new(format!("History: {}", t.agent), self.root.clone(), vec![t])));
                    self.focus = Focus::Editor;
                }
            }
            Target::AgentTab(i) => self.show_agent(i),
            Target::CodeAction(i) => self.apply_code_action(i),
            Target::Transcript(kind, id, path, title) => {
                let md = sessions::transcript_markdown(kind, &path);
                self.view = Some(View::Transcript(views::TranscriptView::new(format!("{} · {title}", kind.name()), kind, id, md)));
                self.focus = Focus::Editor;
            }
            Target::Resume(kind, id) => {
                if let Some(i) = self.agents.iter().position(|a| a.session_id.as_deref() == Some(&id)) {
                    return self.show_agent(i);
                }
                let command = match kind {
                    AgentKind::Codex => format!("codex resume {id}"),
                    _ => format!("claude --resume {id}"),
                };
                let name = self.unique_name(kind.name());
                let root = self.root.clone();
                let idx = self.push_agent(&name, &command, root);
                if kind == AgentKind::Claude {
                    self.agents[idx].session_id = Some(id);
                }
                self.show_agent(idx);
                self.resuming.insert(self.agents[idx].id);
            }
        }
    }

    fn show_agent(&mut self, idx: usize) {
        let slot = if self.split { self.active_slot } else { 0 };
        self.assign_slot(slot, idx);
        if self.zoom && self.focus != Focus::Agent {
            self.zoom = false;
        }
        self.set_focus(Focus::Agent);
    }

    fn remove_agent(&mut self, idx: usize) {
        if idx >= self.agents.len() {
            return;
        }
        self.agents.remove(idx);
        for s in &mut self.slots {
            if *s > idx {
                *s -= 1;
            } else if *s == idx {
                *s = 0;
            }
        }
        if self.agents.len() < 2 || self.slots[0] == self.slots[1] {
            self.split = false;
            self.active_slot = 0;
        }
    }

    fn close_agent(&mut self) {
        let idx = self.active_agent();
        self.close_agent_at(idx);
    }

    fn close_agent_at(&mut self, idx: usize) {
        let Some(a) = self.agents.get(idx) else { return };
        let (running, name, worktree) = (a.running(), a.name.clone(), a.worktree.clone());
        if running && !self.confirmed(&format!("close-agent-{}", a.id)) {
            return self.error(format!("{name} is still running — click ✕ again (or Alt+W) to stop it"));
        }
        if let Some((_, branch)) = worktree {
            self.info(format!("closed tab; worktree {branch} kept (use Worktree: Remove to delete it)"));
        }
        self.agents[idx].kill();
        self.remove_agent(idx);
    }

    fn open_review(&mut self, only: Option<Vec<String>>) {
        self.return_view = None;
        let idx = self.active_agent();
        let (repo, title) = match self.agents.get(idx).and_then(|a| a.worktree.clone()) {
            Some((path, branch)) => (Some(Repo { root: path }), format!("Changes · {branch}")),
            None => (self.repo.clone(), "Changes".to_string()),
        };
        let Some(repo) = repo else { return self.error("not a git repository") };
        match ReviewView::new(title, repo, only) {
            Ok(v) => {
                if v.is_empty() {
                    self.info("no unstaged changes to review");
                }
                self.view = Some(View::Review(v));
                self.focus = Focus::Editor;
                self.banner = None;
            }
            Err(e) => self.error(e),
        }
    }

    fn handoff(&mut self, from: usize, to: usize) {
        let (Some(f), Some(_)) = (self.agents.get(from), self.agents.get(to)) else { return };
        let turn = self.activity.last_turn(&f.name).cloned().unwrap_or_default();
        let files = turn.changed_files();
        let mut prompt = format!("Review the changes {} just made", f.name);
        if !turn.prompt.is_empty() {
            prompt.push_str(&format!(" for the task \"{}\"", turn.prompt.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
        prompt.push('.');
        if files.is_empty() {
            prompt.push_str(" Use `git diff` to see them.");
        } else {
            let list: Vec<String> = files.iter().map(|p| format!("@{p}")).collect();
            prompt.push_str(&format!(" Changed files: {}. Use `git diff -- {}` to see exactly what changed.", list.join(" "), files.join(" ")));
        }
        prompt.push_str(" Focus on correctness, edge cases, performance and security. Cite file:line for each finding. ");
        self.show_agent(to);
        if let Some(a) = self.agent() {
            a.paste(&prompt);
        }
    }

    fn fix_findings(&mut self, reviewer: usize, fixer: usize) {
        let (Some(r), Some(_)) = (self.agents.get(reviewer), self.agents.get(fixer)) else { return };
        let name = r.name.clone();
        let prompt = changes::fix_findings_prompt(&name, self.activity.last_turn(&name));
        self.show_agent(fixer);
        self.set_focus(Focus::Agent);
        if let Some(a) = self.agent() {
            a.paste(&prompt);
        }
        self.info(format!("paste {name}'s findings at the end of the prompt, then press Enter"));
    }

    /// Picker over every pair of agent tabs (not shells).
    fn pick_pair(&mut self, title: &str, action: fn(usize, usize) -> Action) {
        let ids: Vec<usize> = (0..self.agents.len()).filter(|&i| matches!(self.agents[i].kind, Some(k) if k != AgentKind::Shell)).collect();
        if ids.len() < 2 {
            return self.info("open at least two Claude/Codex tabs first");
        }
        let where_ = |i: usize| self.agents[i].worktree.as_ref().map_or("main checkout".to_string(), |w| w.1.clone());
        let mut items = Vec::new();
        for (n, &a) in ids.iter().enumerate() {
            for &b in &ids[n + 1..] {
                let detail = format!("{} · {}", where_(a), where_(b));
                items.push(Item::new(format!("{} ⇄ {}", self.agents[a].name, self.agents[b].name), detail, Target::Action(action(a, b))));
            }
        }
        self.mode = Mode::Picker(Picker::new(title, Kind::Static, items).ordered());
    }

    fn send_to_pair(&mut self, a: usize, b: usize, input: &str) {
        let text = input.trim();
        if text.is_empty() || a >= self.agents.len() || b >= self.agents.len() {
            return;
        }
        let idle: Vec<String> = [a, b].iter().filter(|&&i| !self.agents[i].started() || self.agents[i].exited).map(|&i| self.agents[i].name.clone()).collect();
        if !idle.is_empty() {
            return self.error(format!("{} not running; open the tab first", idle.join(" and ")));
        }
        // Paste only: the user checks both prompts and presses Enter in each pane.
        for i in [a, b] {
            self.agents[i].paste(text);
        }
        self.split = true;
        self.assign_slot(0, a);
        self.assign_slot(1, b);
        self.info(format!("prompt pasted into {} and {}; press Enter in each pane to send (Alt+w switches pane)", self.agents[a].name, self.agents[b].name));
    }

    fn abs_path(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() { path.to_path_buf() } else { self.root.join(path) }
    }

    /// Files each agent changed, with git state and reviewed flags.
    fn change_groups(&self) -> Vec<ChangeGroup> {
        let mut groups = Vec::new();
        for a in self.agents.iter().filter(|a| matches!(a.kind, Some(k) if k != AgentKind::Shell)) {
            let (repo_root, status) = match &a.worktree {
                Some((path, _)) => (Some(path.clone()), self.worktree_git.get(path)),
                None => (self.repo.as_ref().map(|r| r.root.clone()), self.repo.as_ref().map(|_| &self.git)),
            };
            let in_repo = status.is_some();
            let mut files: Vec<ChangedFile> = Vec::new();
            let mut add = |key: String, abs: PathBuf, created: bool, version: changes::Version| {
                let repo_rel = repo_root.as_ref().and_then(|r| abs.strip_prefix(r).ok()).map_or_else(|| key.clone(), |p| p.to_string_lossy().into_owned());
                let state = status.and_then(|s| s.files.get(&abs).copied());
                let mark = self.reviewed.get(&(a.name.clone(), key.clone())).copied();
                let reviewed = changes::is_reviewed(version, state, in_repo, mark);
                files.push(ChangedFile { abs, repo_rel, key, state, created, version, reviewed });
            };
            for f in changes::agent_files(&self.activity.turns, &a.name) {
                let abs = self.abs_path(&f.path);
                add(f.path, abs, f.created, f.version);
            }
            // Worktree agents own their checkout, so every change there is theirs.
            if let (Some(_), Some(st)) = (&a.worktree, status) {
                let mut extra: Vec<(&PathBuf, &git::FileState)> = st.files.iter().collect();
                extra.sort_by(|x, y| x.0.cmp(y.0));
                let known: HashSet<PathBuf> = changes::agent_files(&self.activity.turns, &a.name).iter().map(|f| self.abs_path(&f.path)).collect();
                for (abs, state) in extra {
                    if !known.contains(abs) {
                        let key = refs::relative(&self.root, abs).into_owned();
                        add(key, abs.clone(), *state == git::FileState::Untracked, changes::Version::default());
                    }
                }
                files.sort_by(|x, y| x.repo_rel.cmp(&y.repo_rel));
            }
            groups.push(ChangeGroup { agent: a.name.clone(), repo_root, worktree: a.worktree.as_ref().map(|w| w.1.clone()), files });
        }
        groups
    }

    /// Recompute the changed-files view and the per-agent unreviewed counts.
    fn refresh_agent_views(&mut self) {
        let groups = self.change_groups();
        self.unreviewed = groups.iter().map(|g| (g.agent.clone(), g.unreviewed())).collect();
        for v in [&mut self.view, &mut self.return_view] {
            if let Some(View::AgentChanges(view)) = v {
                view.set_groups(groups.clone());
            }
        }
    }

    fn compare_side(&self, idx: usize) -> CompareSide {
        let a = &self.agents[idx];
        let turns: Vec<&Turn> = self.activity.turns.iter().filter(|t| t.agent == a.name).collect();
        let prompt = turns.last().map(|t| t.prompt.clone()).unwrap_or_default();
        let commands = turns.iter().flat_map(|t| t.commands.iter().cloned()).collect();
        let to_files = |stats: Vec<(String, Option<(usize, usize)>)>| stats.into_iter().map(|(path, counts)| CompareFile { path, counts, shared: false }).collect();
        let mut side = CompareSide { agent: a.name.clone(), repo_root: None, where_: String::new(), files: Vec::new(), commands, prompt, note: None };
        if let Some((path, branch)) = &a.worktree {
            let wt = Repo { root: path.clone() };
            side.repo_root = Some(path.clone());
            let main_head = self.repo.as_ref().and_then(|r| r.head().ok());
            let base = main_head.and_then(|h| wt.merge_base("HEAD", &h).ok()).unwrap_or_else(|| "HEAD".into());
            side.where_ = format!("worktree {branch} vs {}", &base[..base.len().min(7)]);
            match wt.numstat(&base, None) {
                Ok(stats) => side.files = to_files(stats),
                Err(e) => side.note = Some(format!("git: {e:#}")),
            }
            return side;
        }
        side.where_ = "main checkout".into();
        let recorded = changes::agent_files(&self.activity.turns, &a.name);
        let Some(repo) = &self.repo else {
            side.files = recorded.into_iter().map(|f| CompareFile { path: f.path, counts: None, shared: false }).collect();
            side.note = Some("not a git repository; line counts unavailable".into());
            return side;
        };
        side.repo_root = Some(repo.root.clone());
        let paths: Vec<String> = recorded.iter().filter_map(|f| self.abs_path(&f.path).strip_prefix(&repo.root).ok().map(|p| p.to_string_lossy().into_owned())).collect();
        match repo.numstat("HEAD", Some(&paths)) {
            Ok(stats) => {
                let gone = paths.len().saturating_sub(stats.len());
                side.files = to_files(stats);
                let mut note = "files this agent edited; counts are uncommitted changes vs HEAD, including other agents' edits".to_string();
                if gone > 0 {
                    note.push_str(&format!("; {gone} more have no uncommitted changes"));
                }
                side.note = Some(note);
            }
            Err(e) => side.note = Some(format!("git: {e:#}")),
        }
        side
    }

    fn new_worktree_agent(&mut self, kind: AgentKind, name: &str) {
        let Some(repo) = self.repo.clone() else { return };
        let name = if name.is_empty() { "task" } else { name };
        match repo.add_worktree(name) {
            Ok((path, branch)) => {
                let slug = branch.trim_start_matches("noida/").to_string();
                let tab = self.unique_name(&format!("{}@{slug}", kind.name()));
                let command = match kind {
                    AgentKind::Shell => std::env::var("SHELL").unwrap_or_else(|_| "bash".into()),
                    k => k.name().to_string(),
                };
                let idx = self.push_agent(&tab, &command, path.clone());
                self.agents[idx].worktree = Some((path, branch.clone()));
                self.show_agent(idx);
                self.info(format!("{tab} works in its own worktree on branch {branch}; Alt+r reviews its changes"));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn send_context(&mut self, prompt: Option<&str>, lines: bool) {
        let Some(doc) = self.doc() else { return self.info("open a file first") };
        let rel = refs::relative(&self.root, &doc.path).to_string();
        let ctx = if lines {
            let (s, e) = doc.selected_lines();
            if s == e { format!("@{rel}#L{s}") } else { format!("@{rel}#L{s}-{e}") }
        } else {
            format!("@{rel}")
        };
        let text = match prompt {
            Some(p) => format!("{ctx} {p} "),
            None => format!("{ctx} "),
        };
        self.set_focus(Focus::Agent);
        if let Some(a) = self.agent() {
            a.paste(&text);
        }
    }

    fn fix_problem(&mut self) {
        let Some(doc) = self.doc() else { return self.info("open a file first") };
        let line = doc.cursor_line0();
        let Some(diags) = self.lsp.diagnostics.get(&doc.path) else { return self.info("no problems in this file") };
        let Some(d) = diags.iter().min_by_key(|d| (d.line.abs_diff(line), d.severity)).cloned() else { return };
        let rel = refs::relative(&self.root, &doc.path).to_string();
        let sev = match d.severity {
            1 => "error",
            2 => "warning",
            _ => "problem",
        };
        let source = if d.source.is_empty() { String::new() } else { format!(" ({})", d.source) };
        let text = format!("Fix this {sev} at @{rel}#L{}: {}{source} ", d.line + 1, d.message);
        self.set_focus(Focus::Agent);
        if let Some(a) = self.agent() {
            a.paste(&text);
        }
    }

    fn symbol_definition(&mut self, word: &str) {
        let defs: Vec<(PathBuf, crate::symbols::Symbol)> = self.symbols.definitions(word).into_iter().cloned().collect();
        match defs.len() {
            0 => self.info(format!("no definition found for {word}")),
            1 => {
                let (p, s) = &defs[0];
                self.open_location(&p.clone(), Some(s.line + 1), Some(s.col + 1), None);
            }
            _ => {
                let items = defs
                    .into_iter()
                    .map(|(p, s)| Item::new(format!("{}:{}", refs::relative(&self.root, &p), s.line + 1), s.kind, Target::File { path: p, line: Some(s.line + 1), col: Some(s.col + 1) }))
                    .collect();
                self.mode = Mode::Picker(Picker::new(format!("Definitions: {word}"), Kind::Static, items).ordered());
            }
        }
    }

    fn on_locations(&mut self, kind: Request, locs: Vec<lsp::Location>) {
        let word = self.pending_definition.take();
        if locs.is_empty() {
            if let (Request::Definition, Some(w)) = (kind, word) {
                return self.symbol_definition(&w);
            }
            return self.info("no results");
        }
        // LSP columns are UTF-16; files not yet open are converted after opening.
        if locs.len() == 1 {
            let (p, line, col16) = locs[0].clone();
            self.open_location(&p, Some(line + 1), None, None);
            if let Some(doc) = self.docs.get_mut(self.groups[self.active_group].active) {
                let col = doc.char_col_from_utf16(line, col16);
                doc.goto(line + 1, Some(col + 1), None);
            }
            return;
        }
        let title = if kind == Request::Definition { "Definitions" } else { "References" };
        let items = locs
            .into_iter()
            .map(|(p, l, c)| {
                let text = std::fs::read_to_string(&p).ok().and_then(|t| t.lines().nth(l).map(|s| s.trim().to_string())).unwrap_or_default();
                Item::new(format!("{}:{}", refs::relative(&self.root, &p), l + 1), text, Target::File { path: p, line: Some(l + 1), col: Some(c + 1) })
            })
            .collect();
        self.mode = Mode::Picker(Picker::new(title, Kind::Static, items).ordered());
    }

    fn request_quit(&mut self) {
        let dirty: Vec<String> = self.docs.iter().filter(|d| d.dirty).map(Doc::file_name).collect();
        if dirty.is_empty() || self.confirmed("quit") {
            self.save_workspace();
            self.quit = true;
        } else {
            self.error(format!("unsaved: {} — press again to quit anyway", dirty.join(", ")));
        }
    }

    fn cycle_doc(&mut self, delta: isize) {
        if self.docs.is_empty() {
            return;
        }
        self.view = None;
        let n = self.docs.len() as isize;
        self.set_active_doc(((self.active_doc() as isize + delta).rem_euclid(n)) as usize);
        let path = self.docs[self.active_doc()].path.clone();
        self.tree.reveal(&path);
        self.refresh_marks();
    }

    fn close_doc(&mut self) {
        if self.view.is_some() {
            self.view = None;
            return;
        }
        let Some(doc) = self.doc() else { return };
        let (dirty, name) = (doc.dirty, doc.file_name());
        if dirty && !self.confirmed("close-doc") {
            return self.error(format!("{name} has unsaved changes — close again to discard"));
        }
        let path = self.docs[self.active_doc()].path.clone();
        self.lsp.did_close(&path);
        self.lsp_synced.remove(&path);
        let i = self.active_doc();
        self.docs.remove(i);
        groups::on_doc_removed(&mut self.groups, &mut self.active_group, i, self.docs.len());
        self.refresh_marks();
    }

    // ---- navigation ----

    pub fn open_path(&mut self, path: &Path, line: Option<usize>) {
        self.open_location(path, line, None, None);
    }

    fn open_ref(&mut self, r: &FileRef) {
        if let Some(name) = &r.symbol {
            if r.ambiguous.is_some() {
                return self.symbol_definition(&name.clone());
            }
        } else if let Some(partial) = &r.ambiguous {
            let query = match r.line {
                Some(l) => format!("{partial}:{l}"),
                None => partial.clone(),
            };
            self.mode = Mode::Picker(Picker::new("Open File", Kind::Files, Vec::new()).with_query(query));
            self.update_quick_open();
            return self.info(format!("{partial} matches several files — pick one"));
        }
        self.view = None;
        self.open_location(&r.path.clone(), r.line, r.col, r.end_line);
    }

    fn here(&self) -> Option<Loc> {
        self.doc().map(|d| (d.path.clone(), d.cursor_line(), d.cursor_col()))
    }

    fn open_location(&mut self, path: &Path, line: Option<usize>, col: Option<usize>, end_line: Option<usize>) {
        if path.is_dir() {
            self.show_tree = true;
            self.tree.reveal(&path.join("_"));
            return self.info(format!("revealed {}", refs::relative(&self.root, path)));
        }
        if let Some(here) = self.here() {
            if here.0 != path || line.is_some_and(|l| l != here.1) {
                self.back.push(here);
                self.forward.clear();
                if self.back.len() > 100 {
                    self.back.remove(0);
                }
            }
        }
        let idx = match self.docs.iter().position(|d| d.path == path) {
            Some(i) => i,
            None => match Doc::open(path, &self.syntax) {
                Ok(mut doc) => {
                    doc.wrap = self.settings.word_wrap;
                    self.lsp.did_open(&doc.path, &doc.text());
                    self.lsp_synced.insert(doc.path.clone(), doc.edits);
                    self.docs.push(doc);
                    self.docs.len() - 1
                }
                Err(e) => return self.error(format!("cannot open {}: {e}", refs::relative(&self.root, path))),
            },
        };
        self.set_active_doc(idx);
        if let Some(l) = line {
            self.docs[idx].goto(l, col, end_line);
        }
        if self.zoom && self.focus == Focus::Agent {
            self.zoom = false;
        }
        self.tree.reveal(path);
        self.refresh_marks();
        let rel = refs::relative(&self.root, path).to_string();
        self.info(match line {
            Some(l) => format!("{rel}:{l}"),
            None => rel,
        });
    }

    fn jump(&mut self, loc: Loc) {
        let idx = match self.docs.iter().position(|d| d.path == loc.0) {
            Some(i) => i,
            None => {
                let depth = (self.back.len(), self.forward.len());
                self.open_location(&loc.0, Some(loc.1), Some(loc.2), None);
                self.back.truncate(depth.0);
                return;
            }
        };
        self.view = None;
        self.set_active_doc(idx);
        self.docs[idx].goto(loc.1, Some(loc.2), None);
        self.tree.reveal(&loc.0);
        self.refresh_marks();
    }

    fn go_back(&mut self) {
        let Some(loc) = self.back.pop() else { return self.info("no previous location") };
        if let Some(here) = self.here() {
            self.forward.push(here);
        }
        self.jump(loc);
    }

    fn go_forward(&mut self) {
        let Some(loc) = self.forward.pop() else { return self.info("no next location") };
        if let Some(here) = self.here() {
            self.back.push(here);
        }
        self.jump(loc);
    }

    fn start_hints(&mut self) {
        if self.zoom && self.focus != Focus::Agent {
            self.zoom = false;
        }
        if self.refs[0].is_empty() && self.refs[1].is_empty() {
            return self.info("no file references visible in the agent pane");
        }
        self.mode = Mode::Hints { typed: String::new() };
    }

    fn update_quick_open(&mut self) {
        let Mode::Picker(p) = &mut self.mode else { return };
        if p.kind != Kind::Files {
            return;
        }
        let (q, _) = refs::split_line_suffix(&p.query);
        let q = q.to_lowercase();
        let files = &self.file_index.files;
        let item = |rel: &String| {
            let (dir, name) = rel.rsplit_once('/').map_or(("", rel.as_str()), |(d, n)| (d, n));
            Item::new(name, dir, Target::File { path: self.root.join(rel), line: None, col: None })
        };
        let items: Vec<Item> = if q.is_empty() {
            let open: Vec<String> = self.docs.iter().rev().filter(|d| d.path.starts_with(&self.root)).map(|d| refs::relative(&self.root, &d.path).into_owned()).collect();
            open.iter().chain(files.iter().filter(|f| !open.contains(f)).take(300)).map(item).collect()
        } else {
            let mut scored: Vec<(i64, &String)> = files.iter().filter_map(|f| crate::picker::fuzzy_score(&q, f).map(|s| (s, f))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())));
            scored.into_iter().take(300).map(|(_, f)| item(f)).collect()
        };
        p.set_items(items);
    }
}

/// Picker items for `git log` / file history. For file history each commit is
/// limited to the file's path there plus its older name, so renames show up.
fn commit_items(log: Vec<git::Commit>) -> Vec<Item> {
    let older: Vec<Option<String>> = log.iter().skip(1).map(|c| c.path.clone()).chain([None]).collect();
    log.into_iter()
        .zip(older)
        .map(|(c, older)| {
            let label = format!("{} {}", c.short, c.subject);
            let mut paths: Vec<String> = c.path.into_iter().collect();
            if let Some(o) = older.filter(|o| !paths.contains(o)) {
                if !paths.is_empty() {
                    paths.push(o);
                }
            }
            Item::new(label, c.author, Target::Commit { hash: c.hash, paths }).hint(c.date)
        })
        .collect()
}

fn session_tab_items(agents: &[Agent]) -> Vec<Item> {
    let mut items: Vec<Item> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let state = if a.running() { "running" } else if a.started() { "stopped" } else { "not started" };
            let wt = a.worktree.as_ref().map(|(_, b)| format!(" · worktree {b}")).unwrap_or_default();
            Item::new(format!("{} {}", a.status_glyph(), a.name), format!("tab · {state}{wt}"), Target::AgentTab(i))
        })
        .collect();
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Shell] {
        items.push(Item::new(format!("+ New {} tab", kind.name()), "", Target::Action(Action::NewAgent(kind))));
    }
    items
}

/// Shortcut that applies in every pane: a custom binding from settings, else a
/// built-in Ctrl+Shift or Alt shortcut. `Some(None)` means disabled in settings.
fn global_binding(keymap: &Keymap, key: KeyEvent) -> Option<Option<Action>> {
    if let Some(bound) = keys::spec(key).and_then(|s| keymap.get(&s)) {
        return Some(*bound);
    }
    if let Some(a) = keys::ctrl_shift_action(key) {
        return Some(Some(a));
    }
    match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) && !key.modifiers.contains(KeyModifiers::CONTROL) => global_action(c).map(Some),
        _ => None,
    }
}

/// Alt+key shortcuts that work from every pane. Chosen to avoid Claude Code's
/// own Meta bindings (p, t, b, f, m) and terminal escape ambiguities (O, [, \).
fn global_action(c: char) -> Option<Action> {
    Some(match c {
        '1' => Action::FocusTree,
        '2' => Action::FocusEditor,
        '3' => Action::FocusAgent,
        '0' => Action::ToggleTree,
        'x' => Action::CommandPalette,
        'o' => Action::QuickOpen,
        'j' => Action::JumpToRef,
        's' => Action::SendSelection,
        'S' => Action::SendFile,
        'e' => Action::AskMenu,
        'T' => Action::ReopenClosedFile,
        'W' => Action::CloseAgent,
        'N' => Action::NewAgent(AgentKind::Claude),
        'H' => Action::BrowseHistory,
        'F' => Action::FormatDocument,
        'E' => Action::RecentFiles,
        'C' => Action::AgentChanges,
        'n' => Action::NextAgent,
        'g' => Action::Sessions,
        'v' => Action::ToggleSplit,
        'w' => Action::OtherAgentPane,
        'r' => Action::ReviewChanges,
        'a' => Action::AgentActivity,
        'l' => Action::GoToSymbol,
        'k' => Action::GoToProjectSymbol,
        'i' => Action::Problems,
        '/' => Action::SearchWorkspace,
        'z' => Action::Zoom,
        '-' => Action::GoBack,
        'u' => Action::RunTests,
        'q' => Action::Quit,
        _ => return None,
    })
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
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
    fn custom_and_ctrl_shift_bindings() {
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        let empty = Keymap::new();
        assert_eq!(global_binding(&empty, KeyEvent::new(KeyCode::Char('P'), cs)), Some(Some(Action::CommandPalette)));
        assert_eq!(global_binding(&empty, KeyEvent::new(KeyCode::Char('p'), cs)), Some(Some(Action::CommandPalette)));
        assert_eq!(global_binding(&empty, KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)), None);
        // Kitty reports Alt+Shift+s as lowercase with SHIFT; it must still be Alt+S.
        assert_eq!(global_binding(&empty, keys::normalize(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT | KeyModifiers::SHIFT))), Some(Some(Action::SendFile)));
        let settings: HashMap<String, String> = [("alt+y", "quick_open"), ("alt+n", "none"), ("ctrl+shift+p", "search_workspace")].map(|(k, v)| (k.into(), v.into())).into();
        let (map, bad) = keys::keymap(&settings);
        assert!(bad.is_empty());
        assert_eq!(global_binding(&map, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT)), Some(Some(Action::QuickOpen)));
        assert_eq!(global_binding(&map, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)), Some(None));
        assert_eq!(global_binding(&map, KeyEvent::new(KeyCode::Char('P'), cs)), Some(Some(Action::SearchWorkspace)));
    }

    #[test]
    fn global_keys_avoid_claude_bindings() {
        for c in ['p', 't', 'b', 'f', 'm', 'O', '[', '\\'] {
            assert!(global_action(c).is_none(), "Alt+{c} must reach the agent");
        }
    }
}
