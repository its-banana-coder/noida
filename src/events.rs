//! Everything that happens off the UI thread arrives as a `Bg` event on one
//! channel: PTY output, agent hooks, git status, indexes and language servers.

use crate::git;
use crate::hooks::AgentEvent;
use crate::lsp::LspEvent;
use crate::refs::FileIndex;
use crate::sessions::PastSession;
use crate::symbols::ProjectSymbols;

pub enum Bg {
    PtyOutput(usize, Vec<u8>),
    PtyExit(usize),
    Hook(usize, AgentEvent),
    Index(FileIndex),
    Git(Option<git::Status>),
    Sessions(Vec<PastSession>),
    Symbols(ProjectSymbols),
    Lsp(LspEvent),
}
