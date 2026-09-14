//! Every user-invokable command. Keys, the command palette and context
//! actions all dispatch through `App::run`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Shell,
}

impl AgentKind {
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Shell => "shell",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Explain,
    Refactor,
    FindBugs,
    WriteTests,
    Optimize,
    Review,
}

impl Ask {
    pub const ALL: [Ask; 6] = [Ask::Explain, Ask::Refactor, Ask::FindBugs, Ask::WriteTests, Ask::Optimize, Ask::Review];

    pub fn label(self) -> &'static str {
        match self {
            Ask::Explain => "Explain",
            Ask::Refactor => "Refactor",
            Ask::FindBugs => "Find bugs",
            Ask::WriteTests => "Write tests",
            Ask::Optimize => "Optimize",
            Ask::Review => "Review",
        }
    }

    pub fn prompt(self) -> &'static str {
        match self {
            Ask::Explain => "Explain what this code does and why it is structured this way.",
            Ask::Refactor => "Refactor this code for clarity without changing behavior.",
            Ask::FindBugs => "Look for bugs, edge cases and incorrect assumptions in this code. Cite file:line for each finding.",
            Ask::WriteTests => "Write tests covering this code, including edge cases.",
            Ask::Optimize => "Find performance problems in this code and fix the significant ones.",
            Ask::Review => "Review this code for correctness, edge cases, performance and security. Cite file:line for each finding.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    CommandPalette,
    QuickOpen,
    GoToLine,
    Find,
    FindNext,
    Save,
    SaveAll,
    CloseFile,
    NextFile,
    PrevFile,
    ToggleTree,
    Zoom,
    GoBack,
    JumpToRef,
    FocusTree,
    FocusEditor,
    FocusAgent,
    SendSelection,
    SendFile,
    Ask(Ask),
    NextAgent,
    NewAgent(AgentKind),
    CloseAgent,
    RestartAgent,
    ToggleSplit,
    OtherAgentPane,
    ReviewChanges,
    AcceptAllChanges,
    SwitchBranch,
    NewBranch,
    Commit,
    AgentActivity,
    AgentHistory,
    Handoff { from: usize, to: usize },
    GoToSymbol,
    GoToProjectSymbol,
    GoToDefinition,
    FindReferences,
    ExpandSelection,
    ShrinkSelection,
    Problems,
    FixProblem,
    NewWorktreeAgent(AgentKind),
    ApplyWorktree,
    RemoveWorktree,
    Sessions,
    Keys,
    Quit,
}

impl Action {
    /// (label, shortcut) for the palette. `None` hides the action from it.
    pub fn describe(self) -> Option<(String, &'static str)> {
        let s = |l: &str, k: &'static str| Some((l.to_string(), k));
        match self {
            Action::CommandPalette => s("Command Palette", "Alt+x"),
            Action::QuickOpen => s("Open File…", "Alt+o"),
            Action::GoToLine => s("Go to Line…", "Ctrl+G"),
            Action::Find => s("Find in File…", "Ctrl+F"),
            Action::FindNext => s("Find Next", "F3"),
            Action::Save => s("Save File", "Ctrl+S"),
            Action::SaveAll => s("Save All Files", ""),
            Action::CloseFile => s("Close File", "Ctrl+W"),
            Action::NextFile => s("Next Open File", "Ctrl+PgDn"),
            Action::PrevFile => s("Previous Open File", "Ctrl+PgUp"),
            Action::ToggleTree => s("View: Toggle File Tree", "Alt+0"),
            Action::Zoom => s("View: Zoom Pane", "Alt+z"),
            Action::GoBack => s("Go Back", "Alt+-"),
            Action::JumpToRef => s("Jump to File Reference in Agent Output", "Alt+j"),
            Action::FocusTree => s("Focus: File Tree", "Alt+1"),
            Action::FocusEditor => s("Focus: Editor", "Alt+2"),
            Action::FocusAgent => s("Focus: Agent", "Alt+3"),
            Action::SendSelection => s("Agent: Send Selection", "Alt+s"),
            Action::SendFile => s("Agent: Send Current File", "Alt+S"),
            Action::Ask(a) => Some((format!("Ask Agent: {}", a.label()), if a == Ask::Explain { "Alt+e" } else { "" })),
            Action::NextAgent => s("Agent: Next Tab", "Alt+n"),
            Action::NewAgent(k) => Some((format!("Agent: New {} Tab", title(k.name())), "")),
            Action::CloseAgent => s("Agent: Close Tab", ""),
            Action::RestartAgent => s("Agent: Restart", ""),
            Action::ToggleSplit => s("View: Split Agent Panes", "Alt+v"),
            Action::OtherAgentPane => s("Focus: Other Agent Pane", "Alt+w"),
            Action::ReviewChanges => s("Git: Review Changes", "Alt+r"),
            Action::AcceptAllChanges => s("Git: Accept (Stage) All Changes", ""),
            Action::SwitchBranch => s("Git: Switch Branch…", ""),
            Action::NewBranch => s("Git: New Branch…", ""),
            Action::Commit => s("Git: Commit Staged…", ""),
            Action::AgentActivity => s("Agent: Activity & Timeline", "Alt+a"),
            Action::AgentHistory => s("Agent: History", ""),
            Action::Handoff { .. } => None,
            Action::GoToSymbol => s("Go to Symbol in File…", "Alt+l"),
            Action::GoToProjectSymbol => s("Go to Symbol in Project…", "Alt+k"),
            Action::GoToDefinition => s("Go to Definition", "F12"),
            Action::FindReferences => s("Find References", "Shift+F12"),
            Action::ExpandSelection => s("Expand Selection", "Alt+↑"),
            Action::ShrinkSelection => s("Shrink Selection", "Alt+↓"),
            Action::Problems => s("Problems…", "Alt+i"),
            Action::FixProblem => s("Ask Agent: Fix Problem at Cursor", "Alt+F"),
            Action::NewWorktreeAgent(k) => Some((format!("Worktree: New {} in Isolated Worktree…", title(k.name())), "")),
            Action::ApplyWorktree => s("Worktree: Apply Agent's Changes to Project", ""),
            Action::RemoveWorktree => s("Worktree: Remove Agent's Worktree", ""),
            Action::Sessions => s("Agent: Sessions (switch or resume past)…", "Alt+g"),
            Action::Keys => s("Help: Keyboard Shortcuts", ""),
            Action::Quit => s("Quit", "Alt+q"),
        }
    }

    pub fn palette() -> Vec<Action> {
        use Action::*;
        let mut v = vec![
            QuickOpen, GoToSymbol, GoToProjectSymbol, GoToDefinition, FindReferences, Problems, FixProblem,
            ReviewChanges, AcceptAllChanges, SwitchBranch, NewBranch, Commit,
            Sessions, AgentActivity, AgentHistory, SendSelection, SendFile,
        ];
        v.extend(self::Ask::ALL.map(Action::Ask));
        v.extend([
            NewAgent(AgentKind::Claude), NewAgent(AgentKind::Codex), NewAgent(AgentKind::Shell),
            NewWorktreeAgent(AgentKind::Claude), NewWorktreeAgent(AgentKind::Codex), NewWorktreeAgent(AgentKind::Shell), ApplyWorktree, RemoveWorktree,
            NextAgent, CloseAgent, RestartAgent, ToggleSplit, OtherAgentPane,
            GoToLine, Find, FindNext, ExpandSelection, ShrinkSelection, Save, SaveAll, CloseFile, NextFile, PrevFile,
            JumpToRef, GoBack, ToggleTree, Zoom, FocusTree, FocusEditor, FocusAgent, Keys, Quit,
        ]);
        v
    }
}

fn title(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
