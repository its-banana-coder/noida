//! Everything that happens off the UI thread arrives as a `Bg` event on one
//! channel: PTY output, agent hooks, git status, indexes and language servers.

use crate::git;
use crate::hooks::AgentEvent;
use crate::lsp::LspEvent;
use crate::app::SearchResults;
use crate::refs::FileIndex;
use crate::sessions::PastSession;
use crate::symbols::ProjectSymbols;
use crate::testing::Outcome;

pub enum Bg {
    PtyOutput(usize, Vec<u8>),
    PtyExit(usize),
    Hook(usize, AgentEvent),
    Index(FileIndex),
    Git(Option<git::Status>),
    /// Status of each worktree agent's checkout, keyed by worktree path.
    WorktreeGit(Vec<(std::path::PathBuf, git::Status)>),
    Sessions(Vec<PastSession>),
    Symbols(ProjectSymbols),
    Lsp(LspEvent),
    Search(SearchResults),
    /// A test run finished: (run id, what it found, full output).
    TestDone(u64, Outcome, String),
}
