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
    FindReplace,
    SearchWorkspace,
    ReplaceWorkspace,
    FindNext,
    FindPrev,
    ToggleWordWrap,
    Fold,
    Unfold,
    FoldAll,
    UnfoldAll,
    ToggleComment,
    SelectAllOccurrences,
    AddCursorAbove,
    AddCursorBelow,
    OpenSettings,
    ToggleTheme,
    Save,
    SaveAll,
    CloseFile,
    SplitEditor,
    FocusOtherEditorGroup,
    CloseEditorGroup,
    MoveTabToOtherGroup,
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
    AgentFind,
    AgentCopyScreen,
    ReviewChanges,
    AcceptAllChanges,
    SwitchBranch,
    NewBranch,
    Commit,
    GitLog,
    FileHistory,
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
    CloseOtherFiles,
    CloseAllFiles,
    CloseFilesToRight,
    PinFile,
    ReopenClosedFile,
    RecentFiles,
    RecentLocations,
    NewFile,
    NewFolder,
    RenamePath,
    DeletePath,
    CopyRelativePath,
    CopyAbsolutePath,
    RevealInOs,
    ToggleOutline,
    SendSymbol,
    ToggleShareContext,
    ToggleMarkdownPreview,
    AskMenu,
    Hover,
    RenameSymbol,
    CodeActions,
    FormatDocument,
    FormatSelection,
    OrganizeImports,
    Keys,
    Quit,
}

/// Stable ids for `settings.json` keybindings. One table so `id` and `from_id` can't drift.
const IDS: &[(Action, &str)] = &[
    (Action::CommandPalette, "command_palette"),
    (Action::SplitEditor, "split_editor"),
    (Action::AgentFind, "agent_find"),
    (Action::AgentCopyScreen, "agent_copy_screen"),
    (Action::GitLog, "git_log"),
    (Action::FileHistory, "file_history"),
    (Action::FocusOtherEditorGroup, "focus_other_editor_group"),
    (Action::CloseEditorGroup, "close_editor_group"),
    (Action::MoveTabToOtherGroup, "move_tab_to_other_group"),
    (Action::QuickOpen, "quick_open"),
    (Action::GoToLine, "go_to_line"),
    (Action::Find, "find"),
    (Action::FindReplace, "find_replace"),
    (Action::SearchWorkspace, "search_workspace"),
    (Action::ReplaceWorkspace, "replace_workspace"),
    (Action::FindNext, "find_next"),
    (Action::FindPrev, "find_prev"),
    (Action::ToggleWordWrap, "toggle_word_wrap"),
    (Action::Fold, "fold"),
    (Action::Unfold, "unfold"),
    (Action::FoldAll, "fold_all"),
    (Action::UnfoldAll, "unfold_all"),
    (Action::ToggleComment, "toggle_comment"),
    (Action::SelectAllOccurrences, "select_all_occurrences"),
    (Action::AddCursorAbove, "add_cursor_above"),
    (Action::AddCursorBelow, "add_cursor_below"),
    (Action::OpenSettings, "open_settings"),
    (Action::ToggleTheme, "toggle_theme"),
    (Action::Save, "save"),
    (Action::SaveAll, "save_all"),
    (Action::CloseFile, "close_file"),
    (Action::NextFile, "next_file"),
    (Action::PrevFile, "prev_file"),
    (Action::ToggleTree, "toggle_tree"),
    (Action::Zoom, "zoom"),
    (Action::GoBack, "go_back"),
    (Action::JumpToRef, "jump_to_ref"),
    (Action::FocusTree, "focus_tree"),
    (Action::FocusEditor, "focus_editor"),
    (Action::FocusAgent, "focus_agent"),
    (Action::SendSelection, "send_selection"),
    (Action::SendFile, "send_file"),
    (Action::Ask(Ask::Explain), "ask_explain"),
    (Action::Ask(Ask::Refactor), "ask_refactor"),
    (Action::Ask(Ask::FindBugs), "ask_find_bugs"),
    (Action::Ask(Ask::WriteTests), "ask_write_tests"),
    (Action::Ask(Ask::Optimize), "ask_optimize"),
    (Action::Ask(Ask::Review), "ask_review"),
    (Action::NextAgent, "next_agent"),
    (Action::NewAgent(AgentKind::Claude), "new_agent_claude"),
    (Action::NewAgent(AgentKind::Codex), "new_agent_codex"),
    (Action::NewAgent(AgentKind::Shell), "new_agent_shell"),
    (Action::CloseAgent, "close_agent"),
    (Action::RestartAgent, "restart_agent"),
    (Action::ToggleSplit, "toggle_split"),
    (Action::OtherAgentPane, "other_agent_pane"),
    (Action::ReviewChanges, "review_changes"),
    (Action::AcceptAllChanges, "accept_all_changes"),
    (Action::SwitchBranch, "switch_branch"),
    (Action::NewBranch, "new_branch"),
    (Action::Commit, "commit"),
    (Action::AgentActivity, "agent_activity"),
    (Action::AgentHistory, "agent_history"),
    (Action::GoToSymbol, "go_to_symbol"),
    (Action::GoToProjectSymbol, "go_to_project_symbol"),
    (Action::GoToDefinition, "go_to_definition"),
    (Action::FindReferences, "find_references"),
    (Action::ExpandSelection, "expand_selection"),
    (Action::ShrinkSelection, "shrink_selection"),
    (Action::Problems, "problems"),
    (Action::FixProblem, "fix_problem"),
    (Action::NewWorktreeAgent(AgentKind::Claude), "new_worktree_agent_claude"),
    (Action::NewWorktreeAgent(AgentKind::Codex), "new_worktree_agent_codex"),
    (Action::NewWorktreeAgent(AgentKind::Shell), "new_worktree_agent_shell"),
    (Action::ApplyWorktree, "apply_worktree"),
    (Action::RemoveWorktree, "remove_worktree"),
    (Action::Sessions, "sessions"),
    (Action::CloseOtherFiles, "close_other_files"),
    (Action::CloseAllFiles, "close_all_files"),
    (Action::CloseFilesToRight, "close_files_to_right"),
    (Action::PinFile, "pin_file"),
    (Action::ReopenClosedFile, "reopen_closed_file"),
    (Action::RecentFiles, "recent_files"),
    (Action::RecentLocations, "recent_locations"),
    (Action::NewFile, "new_file"),
    (Action::NewFolder, "new_folder"),
    (Action::RenamePath, "rename_path"),
    (Action::DeletePath, "delete_path"),
    (Action::CopyRelativePath, "copy_relative_path"),
    (Action::CopyAbsolutePath, "copy_absolute_path"),
    (Action::RevealInOs, "reveal_in_os"),
    (Action::ToggleOutline, "toggle_outline"),
    (Action::SendSymbol, "send_symbol"),
    (Action::ToggleShareContext, "toggle_share_context"),
    (Action::ToggleMarkdownPreview, "toggle_markdown_preview"),
    (Action::AskMenu, "ask_menu"),
    (Action::Hover, "hover"),
    (Action::RenameSymbol, "rename_symbol"),
    (Action::CodeActions, "code_actions"),
    (Action::FormatDocument, "format_document"),
    (Action::FormatSelection, "format_selection"),
    (Action::OrganizeImports, "organize_imports"),
    (Action::Keys, "keyboard_shortcuts"),
    (Action::Quit, "quit"),
];

impl Action {
    pub fn id(self) -> Option<&'static str> {
        IDS.iter().find(|(a, _)| *a == self).map(|(_, id)| *id)
    }

    pub fn from_id(id: &str) -> Option<Action> {
        IDS.iter().find(|(_, i)| *i == id).map(|(a, _)| *a)
    }

    #[cfg(test)]
    /// Every action that has an id, in table order.
    pub fn all_with_ids() -> impl Iterator<Item = (Action, &'static str)> {
        IDS.iter().copied()
    }

    /// (label, shortcut) for the palette. `None` hides the action from it.
    pub fn describe(self) -> Option<(String, &'static str)> {
        let s = |l: &str, k: &'static str| Some((l.to_string(), k));
        match self {
            Action::CommandPalette => s("Command Palette", "Alt+x"),
            Action::QuickOpen => s("Open File…", "Alt+o"),
            Action::GoToLine => s("Go to Line…", "Ctrl+G"),
            Action::Find => s("Find in File…", "Ctrl+F"),
            Action::FindReplace => s("Replace in File…", "Ctrl+H"),
            Action::SearchWorkspace => s("Search in Workspace…", "Alt+/"),
            Action::ReplaceWorkspace => s("Replace in Workspace…", ""),
            Action::FindNext => s("Find Next", "F3"),
            Action::FindPrev => s("Find Previous", "Shift+F3"),
            Action::ToggleWordWrap => s("View: Toggle Word Wrap", ""),
            Action::Fold => s("Fold", "click ▾"),
            Action::Unfold => s("Unfold", "click ▸"),
            Action::FoldAll => s("Fold All", ""),
            Action::UnfoldAll => s("Unfold All", ""),
            Action::ToggleComment => s("Toggle Line Comment", "Ctrl+/"),
            Action::SelectAllOccurrences => s("Select All Occurrences", ""),
            Action::AddCursorAbove => s("Add Cursor Above", "Ctrl+Alt+↑"),
            Action::AddCursorBelow => s("Add Cursor Below", "Ctrl+Alt+↓"),
            Action::OpenSettings => s("Preferences: Open Settings (JSON)", ""),
            Action::ToggleTheme => s("Preferences: Toggle Dark/Light Theme", ""),
            Action::Save => s("Save File", "Ctrl+S"),
            Action::SaveAll => s("Save All Files", ""),
            Action::CloseFile => s("Close File", "Ctrl+W"),
            Action::SplitEditor => s("View: Split Editor", "Ctrl+\\"),
            Action::FocusOtherEditorGroup => s("View: Focus Other Editor Group", ""),
            Action::CloseEditorGroup => s("View: Close Editor Group", ""),
            Action::MoveTabToOtherGroup => s("View: Move Tab to Other Editor Group", ""),
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
            Action::Ask(a) => Some((format!("Ask Agent: {}", a.label()), "")),
            Action::NextAgent => s("Agent: Next Tab", "Alt+n"),
            Action::NewAgent(k) => Some((format!("Agent: New {} Tab", title(k.name())), "")),
            Action::CloseAgent => s("Agent: Close Tab", "Alt+W"),
            Action::RestartAgent => s("Agent: Restart", ""),
            Action::ToggleSplit => s("View: Split Agent Panes", "Alt+v"),
            Action::OtherAgentPane => s("Focus: Other Agent Pane", "Alt+w"),
            Action::AgentFind => s("Agent: Find in Output…", "Alt+?"),
            Action::AgentCopyScreen => s("Agent: Copy Visible Output", ""),
            Action::ReviewChanges => s("Git: Review Changes", "Alt+r"),
            Action::AcceptAllChanges => s("Git: Accept (Stage) All Changes", ""),
            Action::SwitchBranch => s("Git: Switch Branch…", ""),
            Action::NewBranch => s("Git: New Branch…", ""),
            Action::Commit => s("Git: Commit Staged…", ""),
            Action::GitLog => s("Git: Log…", ""),
            Action::FileHistory => s("Git: File History…", ""),
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
            Action::FixProblem => s("Ask Agent: Fix Problem at Cursor", ""),
            Action::NewWorktreeAgent(k) => Some((format!("Worktree: New {} in Isolated Worktree…", title(k.name())), "")),
            Action::ApplyWorktree => s("Worktree: Apply Agent's Changes to Project", ""),
            Action::RemoveWorktree => s("Worktree: Remove Agent's Worktree", ""),
            Action::Sessions => s("Agent: Sessions (switch or resume past)…", "Alt+g"),
            Action::CloseOtherFiles => s("Close Other Tabs", ""),
            Action::CloseAllFiles => s("Close All Tabs", ""),
            Action::CloseFilesToRight => s("Close Tabs to the Right", ""),
            Action::PinFile => s("Pin / Unpin Tab", ""),
            Action::ReopenClosedFile => s("Reopen Closed Tab", "Alt+T"),
            Action::RecentFiles => s("Go to Recent File…", "Alt+E"),
            Action::RecentLocations => s("Go to Recent Location…", ""),
            Action::NewFile => s("File: New File…", "tree: a"),
            Action::NewFolder => s("File: New Folder…", "tree: A"),
            Action::RenamePath => s("File: Rename / Move…", "tree: r, F2"),
            Action::DeletePath => s("File: Delete", "tree: d"),
            Action::CopyRelativePath => s("File: Copy Relative Path", "tree: y"),
            Action::CopyAbsolutePath => s("File: Copy Absolute Path", "tree: Y"),
            Action::RevealInOs => s("File: Reveal in File Manager", "tree: o"),
            Action::ToggleOutline => s("View: Toggle Outline", "tree: Tab"),
            Action::SendSymbol => s("Agent: Send Current Symbol", ""),
            Action::AskMenu => s("Ask Agent…", "Alt+e"),
            Action::ToggleShareContext => s("Agent: Toggle Sharing Editor Context", ""),
            Action::ToggleMarkdownPreview => s("Markdown: Toggle Preview", "Alt+m"),
            Action::Hover => s("Show Hover Information", "Alt+h"),
            Action::RenameSymbol => s("Rename Symbol…", "F2"),
            Action::CodeActions => s("Quick Fix / Code Actions…", "Alt+."),
            Action::FormatDocument => s("Format Document", "Alt+F"),
            Action::FormatSelection => s("Format Selection", ""),
            Action::OrganizeImports => s("Organize Imports", ""),
            Action::Keys => s("Help: Keyboard Shortcuts", ""),
            Action::Quit => s("Quit", "Alt+q"),
        }
    }

    pub fn palette() -> Vec<Action> {
        use Action::*;
        let mut v = vec![
            QuickOpen, SearchWorkspace, ReplaceWorkspace, GoToSymbol, Hover, RenameSymbol, CodeActions, FormatDocument, FormatSelection, OrganizeImports, GoToProjectSymbol, GoToDefinition, FindReferences, Problems, FixProblem,
            ReviewChanges, AcceptAllChanges, SwitchBranch, NewBranch, Commit, GitLog, FileHistory,
            Sessions, AgentActivity, AgentHistory, ToggleShareContext, AskMenu, SendSelection, SendFile, SendSymbol,
        ];
        v.extend(self::Ask::ALL.map(Action::Ask));
        v.extend([
            NewAgent(AgentKind::Claude), NewAgent(AgentKind::Codex), NewAgent(AgentKind::Shell),
            NewWorktreeAgent(AgentKind::Claude), NewWorktreeAgent(AgentKind::Codex), NewWorktreeAgent(AgentKind::Shell), ApplyWorktree, RemoveWorktree,
            NextAgent, CloseAgent, RestartAgent, ToggleSplit, OtherAgentPane, AgentFind, AgentCopyScreen,
            GoToLine, Find, FindReplace, FindNext, FindPrev, ToggleComment, SelectAllOccurrences, AddCursorAbove, AddCursorBelow,
            ToggleMarkdownPreview, Fold, Unfold, FoldAll, UnfoldAll, ToggleWordWrap, OpenSettings, ToggleTheme, ExpandSelection, ShrinkSelection, Save, SaveAll, CloseFile, NextFile, PrevFile,
            RecentFiles, RecentLocations, ReopenClosedFile, CloseOtherFiles, CloseAllFiles, CloseFilesToRight, PinFile,
            NewFile, NewFolder, RenamePath, DeletePath, CopyRelativePath, CopyAbsolutePath, RevealInOs, ToggleOutline,
            JumpToRef, GoBack, ToggleTree, Zoom, SplitEditor, FocusOtherEditorGroup, CloseEditorGroup, MoveTabToOtherGroup, FocusTree, FocusEditor, FocusAgent, Keys, Quit,
        ]);
        v
    }
}

fn title(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_cover_palette() {
        let mut seen = std::collections::HashSet::new();
        for (a, id) in Action::all_with_ids() {
            assert!(seen.insert(id), "duplicate id {id}");
            assert_eq!(Action::from_id(id), Some(a));
        }
        for a in Action::palette() {
            assert!(a.id().is_some(), "{a:?} has no id");
        }
        // "Help: Keyboard Shortcuts" lists the palette, so every id must be reachable there.
        let listed: Vec<Action> = std::iter::once(Action::CommandPalette).chain(Action::palette()).collect();
        for (a, id) in Action::all_with_ids() {
            assert!(listed.contains(&a), "{id} missing from the palette");
        }
        assert_eq!(Action::from_id("command_palette"), Some(Action::CommandPalette));
        assert_eq!(Action::NewAgent(AgentKind::Claude).id(), Some("new_agent_claude"));
        assert_eq!(Action::Handoff { from: 0, to: 1 }.id(), None);
    }
}
