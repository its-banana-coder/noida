//! Per-agent changed files and "reviewed" tracking, derived from activity
//! turns and git status. Kept free of UI state so the rules are testable.

use std::collections::BTreeMap;

use crate::activity::{EntryKind, Turn};
use crate::git::FileState;

/// One state of an agent's edits to a file. Any later edit by the agent
/// (a new turn touching the file, another edit entry, a later timestamp)
/// produces a different version, which invalidates a reviewed mark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Version {
    pub turns: usize,
    pub edits: usize,
    pub last: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentFile {
    /// Path as recorded by activity (relative to the project root when inside it).
    pub path: String,
    pub created: bool,
    pub version: Version,
}

/// A manual reviewed/unreviewed decision, valid only for the version it was made on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub version: Version,
    pub reviewed: bool,
}

/// Every file an agent edited or created this session (union across its turns), sorted by path.
pub fn agent_files(turns: &[Turn], agent: &str) -> Vec<AgentFile> {
    let mut files: BTreeMap<String, AgentFile> = BTreeMap::new();
    for t in turns.iter().filter(|t| t.agent == agent) {
        for p in t.edited.union(&t.created).filter(|p| !p.is_empty()) {
            let f = files.entry(p.clone()).or_insert_with(|| AgentFile { path: p.clone(), created: false, version: Version::default() });
            f.version.turns += 1;
            f.version.last = f.version.last.max(t.started);
            f.created |= t.created.contains(p);
        }
        for e in t.entries.iter().filter(|e| matches!(e.kind, EntryKind::Edit | EntryKind::Create)) {
            if let Some(f) = e.path.as_ref().and_then(|p| files.get_mut(p)) {
                f.version.edits += 1;
                f.version.last = f.version.last.max(e.at);
            }
        }
    }
    files.into_values().collect()
}

/// A file is reviewed when a manual mark for its current version says so, or,
/// without one, when git shows nothing left to review: all changes staged
/// (accepted) or none left in the working tree (rejected or committed).
/// `state` is the file's git status (`None` = clean); `in_repo` is false
/// outside a git repository, where only manual marks count.
pub fn is_reviewed(version: Version, state: Option<FileState>, in_repo: bool, mark: Option<Mark>) -> bool {
    if let Some(m) = mark.filter(|m| m.version == version) {
        return m.reviewed;
    }
    in_repo && matches!(state, None | Some(FileState::Staged))
}

/// The mark that flips a file's current reviewed state.
pub fn toggle(version: Version, reviewed: bool) -> Mark {
    Mark { version, reviewed: !reviewed }
}

/// Prompt asking `fixer` to address the findings `reviewer` reported in `turn`.
/// NOIDA doesn't capture the agent's reply text, so the prompt ends where the
/// user pastes the findings, and says so rather than inventing them.
pub fn fix_findings_prompt(reviewer: &str, turn: Option<&Turn>) -> String {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut p = match turn {
        Some(t) if !t.prompt.trim().is_empty() => format!("{reviewer} reviewed code when asked: \"{}\".", one_line(&t.prompt)),
        Some(_) => format!("{reviewer} just reviewed code."),
        None => format!("{reviewer} reviewed recent changes (no turn details were recorded)."),
    };
    if let Some(t) = turn {
        let list = |set: Vec<&String>| {
            let mut v: Vec<String> = set.iter().take(10).map(|f| format!("@{f}")).collect();
            if set.len() > 10 {
                v.push(format!("and {} more", set.len() - 10));
            }
            v.join(" ")
        };
        if !t.read.is_empty() {
            p.push_str(&format!(" It looked at {}.", list(t.read.iter().collect())));
        }
        let changed: Vec<&String> = t.edited.union(&t.created).collect();
        if !changed.is_empty() {
            p.push_str(&format!(" It also changed {}.", list(changed)));
        }
    }
    p.push_str(&format!(
        " Address the issues {reviewer} reported: fix each finding that holds up, verify the fix, and briefly list any you skipped and why. {reviewer}'s findings: "
    ));
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::hooks::AgentEvent;
    use std::path::PathBuf;

    fn edit(a: &mut Activity, id: usize, name: &str, root: &PathBuf, file: &str) {
        a.record(id, name, &AgentEvent::ToolEnd { tool: "Edit".into(), file: Some(root.join(file)), detail: None });
    }

    fn activity() -> (Activity, PathBuf) {
        let root = std::env::temp_dir().join("noida-changes-test");
        (Activity::new(root.clone()).without_history(), root)
    }

    #[test]
    fn unions_files_across_turns_per_agent() {
        let (mut a, root) = activity();
        a.record(1, "claude", &AgentEvent::Prompt { text: "one".into() });
        edit(&mut a, 1, "claude", &root, "src/a.rs");
        a.record(1, "claude", &AgentEvent::ToolEnd { tool: "Read".into(), file: Some(root.join("src/r.rs")), detail: None });
        a.record(1, "claude", &AgentEvent::Stop);
        a.record(2, "codex", &AgentEvent::Prompt { text: "other".into() });
        edit(&mut a, 2, "codex", &root, "src/c.rs");
        a.record(1, "claude", &AgentEvent::Prompt { text: "two".into() });
        edit(&mut a, 1, "claude", &root, "src/b.rs");
        edit(&mut a, 1, "claude", &root, "src/a.rs");
        a.record(1, "claude", &AgentEvent::Stop);

        let files = agent_files(&a.turns, "claude");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/a.rs", "src/b.rs"]);
        assert_eq!((files[0].version.turns, files[0].version.edits), (2, 2));
        assert_eq!((files[1].version.turns, files[1].version.edits), (1, 1));
        assert_eq!(agent_files(&a.turns, "codex").len(), 1);
        assert!(agent_files(&a.turns, "shell").is_empty());
    }

    #[test]
    fn reviewed_from_git_state() {
        let v = Version { turns: 1, edits: 1, last: 5 };
        assert!(!is_reviewed(v, Some(FileState::Modified), true, None));
        assert!(!is_reviewed(v, Some(FileState::Untracked), true, None));
        assert!(!is_reviewed(v, Some(FileState::Conflict), true, None));
        // Accepted: everything staged.
        assert!(is_reviewed(v, Some(FileState::Staged), true, None));
        // Rejected (or committed): nothing left in the diff.
        assert!(is_reviewed(v, None, true, None));
        // Without git only manual marks count.
        assert!(!is_reviewed(v, None, false, None));
    }

    #[test]
    fn manual_mark_until_agent_edits_again() {
        let (mut a, root) = activity();
        a.record(1, "claude", &AgentEvent::Prompt { text: "go".into() });
        edit(&mut a, 1, "claude", &root, "x.rs");
        let v1 = agent_files(&a.turns, "claude")[0].version;
        assert!(!is_reviewed(v1, Some(FileState::Modified), true, None));

        let mark = toggle(v1, false);
        assert!(is_reviewed(v1, Some(FileState::Modified), true, Some(mark)));
        // Toggling again un-reviews, even when git alone would say reviewed.
        let unmark = toggle(v1, true);
        assert!(!is_reviewed(v1, Some(FileState::Staged), true, Some(unmark)));

        // Another edit in the same turn invalidates the mark.
        edit(&mut a, 1, "claude", &root, "x.rs");
        let v2 = agent_files(&a.turns, "claude")[0].version;
        assert_ne!(v1, v2);
        assert!(!is_reviewed(v2, Some(FileState::Modified), true, Some(mark)));

        // So does touching it in a later turn.
        let mark2 = toggle(v2, false);
        a.record(1, "claude", &AgentEvent::Stop);
        a.record(1, "claude", &AgentEvent::Prompt { text: "again".into() });
        edit(&mut a, 1, "claude", &root, "x.rs");
        let v3 = agent_files(&a.turns, "claude")[0].version;
        assert!(!is_reviewed(v3, Some(FileState::Modified), true, Some(mark2)));
        // Edits by another agent don't touch claude's version.
        a.record(2, "codex", &AgentEvent::Prompt { text: "x".into() });
        edit(&mut a, 2, "codex", &root, "x.rs");
        assert_eq!(agent_files(&a.turns, "claude")[0].version, v3);
    }

    #[test]
    fn fix_findings_prompt_is_honest() {
        let (mut a, root) = activity();
        a.record(1, "codex", &AgentEvent::Prompt { text: "review\n the auth changes".into() });
        a.record(1, "codex", &AgentEvent::ToolEnd { tool: "Read".into(), file: Some(root.join("auth.rs")), detail: None });
        let p = fix_findings_prompt("codex", a.last_turn("codex"));
        assert!(p.starts_with("codex reviewed code when asked: \"review the auth changes\"."));
        assert!(p.contains("@auth.rs"));
        assert!(p.ends_with("codex's findings: "));
        let none = fix_findings_prompt("codex", None);
        assert!(none.contains("no turn details were recorded"));
    }
}
