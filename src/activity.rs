//! What agents did: per-turn timelines, touched files, and persisted history.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::hooks::AgentEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Prompt,
    Read,
    Search,
    Edit,
    Create,
    Command,
    Tool,
    Permission,
    Finished,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub kind: EntryKind,
    pub text: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Turn {
    pub agent: String,
    pub prompt: String,
    pub started: u64,
    pub ended: Option<u64>,
    pub entries: Vec<Entry>,
    pub read: BTreeSet<String>,
    pub edited: BTreeSet<String>,
    pub created: BTreeSet<String>,
    pub commands: Vec<String>,
}

impl Turn {
    pub fn changed_files(&self) -> Vec<String> {
        self.edited.union(&self.created).cloned().collect()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        let changed = self.changed_files().len();
        if changed > 0 {
            parts.push(format!("{changed} changed"));
        }
        if !self.read.is_empty() {
            parts.push(format!("{} read", self.read.len()));
        }
        if !self.commands.is_empty() {
            parts.push(format!("{} cmds", self.commands.len()));
        }
        if parts.is_empty() {
            "no tool use".into()
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Touch {
    Reading,
    Edited,
}

pub struct Activity {
    root: PathBuf,
    history_file: Option<PathBuf>,
    /// Finished and in-progress turns this session, oldest first.
    pub turns: Vec<Turn>,
    /// Open turn index per agent id.
    open: HashMap<usize, usize>,
    pub touched: HashMap<PathBuf, (Touch, Instant)>,
    existed: HashMap<PathBuf, bool>,
}

/// What the UI should surface after an event.
#[derive(Debug, PartialEq)]
pub enum Notice {
    None,
    Permission(String),
    Finished { changed: Vec<String> },
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Activity {
    pub fn new(root: PathBuf) -> Self {
        let history_file = std::env::var_os("HOME").map(|h| {
            let escaped: String = root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            PathBuf::from(h).join(".local/share/noida/history").join(format!("{escaped}.jsonl"))
        });
        Self { root, history_file, turns: Vec::new(), open: HashMap::new(), touched: HashMap::new(), existed: HashMap::new() }
    }

    #[cfg(test)]
    fn without_history(mut self) -> Self {
        self.history_file = None;
        self
    }

    fn rel(&self, p: &Path) -> String {
        crate::refs::relative(&self.root, p).into_owned()
    }

    fn turn(&mut self, agent: usize, name: &str) -> &mut Turn {
        let idx = match self.open.get(&agent) {
            Some(&i) => i,
            None => {
                self.turns.push(Turn { agent: name.to_string(), started: now(), ..Default::default() });
                let i = self.turns.len() - 1;
                self.open.insert(agent, i);
                i
            }
        };
        &mut self.turns[idx]
    }

    pub fn record(&mut self, agent: usize, name: &str, ev: &AgentEvent) -> Notice {
        let at = now();
        match ev {
            AgentEvent::SessionStart { .. } => Notice::None,
            AgentEvent::Prompt { text } => {
                if let Some(i) = self.open.remove(&agent) {
                    self.finish(i);
                }
                let t = self.turn(agent, name);
                t.prompt = text.clone();
                t.entries.push(Entry { at, kind: EntryKind::Prompt, text: text.clone(), path: None });
                Notice::None
            }
            AgentEvent::ToolStart { tool, file, .. } => {
                if let Some(f) = file {
                    let abs = if f.is_absolute() { f.clone() } else { self.root.join(f) };
                    self.existed.insert(abs.clone(), abs.exists());
                    if matches!(tool.as_str(), "Read" | "Grep" | "Glob" | "LS") {
                        self.touched.insert(abs, (Touch::Reading, Instant::now()));
                    }
                }
                Notice::None
            }
            AgentEvent::ToolEnd { tool, file, detail } => {
                let abs = file.as_ref().map(|f| if f.is_absolute() { f.clone() } else { self.root.join(f) });
                let rel = abs.as_ref().map(|p| self.rel(p));
                let existed = abs.as_ref().and_then(|p| self.existed.remove(p)).unwrap_or(true);
                let (kind, text) = match tool.as_str() {
                    "Read" | "NotebookRead" => (EntryKind::Read, format!("Read {}", rel.clone().unwrap_or_default())),
                    "Grep" | "Glob" | "LS" => (EntryKind::Search, format!("{tool} {}", detail.clone().or(rel.clone()).unwrap_or_default())),
                    "Edit" | "MultiEdit" | "NotebookEdit" => (EntryKind::Edit, format!("Edited {}", rel.clone().unwrap_or_default())),
                    "Write" if !existed => (EntryKind::Create, format!("Created {}", rel.clone().unwrap_or_default())),
                    "Write" => (EntryKind::Edit, format!("Wrote {}", rel.clone().unwrap_or_default())),
                    "Bash" => (EntryKind::Command, format!("$ {}", detail.clone().unwrap_or_default())),
                    _ => (EntryKind::Tool, format!("{tool} {}", detail.clone().or(rel.clone()).unwrap_or_default())),
                };
                if let (Some(abs), EntryKind::Edit | EntryKind::Create) = (&abs, kind) {
                    self.touched.insert(abs.clone(), (Touch::Edited, Instant::now()));
                }
                let t = self.turn(agent, name);
                match kind {
                    EntryKind::Read => {
                        t.read.insert(rel.clone().unwrap_or_default());
                    }
                    EntryKind::Edit => {
                        t.edited.insert(rel.clone().unwrap_or_default());
                    }
                    EntryKind::Create => {
                        t.created.insert(rel.clone().unwrap_or_default());
                    }
                    EntryKind::Command => t.commands.push(detail.clone().unwrap_or_default()),
                    _ => {}
                }
                if t.entries.len() < 500 {
                    t.entries.push(Entry { at, kind, text, path: rel });
                }
                Notice::None
            }
            AgentEvent::Notification { message, permission } => {
                if *permission {
                    let t = self.turn(agent, name);
                    t.entries.push(Entry { at, kind: EntryKind::Permission, text: message.clone(), path: None });
                    Notice::Permission(message.clone())
                } else {
                    Notice::None
                }
            }
            AgentEvent::Stop => match self.open.remove(&agent) {
                Some(i) => {
                    self.finish(i);
                    Notice::Finished { changed: self.turns[i].changed_files() }
                }
                None => Notice::Finished { changed: Vec::new() },
            },
        }
    }

    fn finish(&mut self, i: usize) {
        let t = &mut self.turns[i];
        t.ended = Some(now());
        t.entries.push(Entry { at: now(), kind: EntryKind::Finished, text: format!("Finished — {}", t.summary()), path: None });
        if let Some(file) = &self.history_file {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let (Ok(mut f), Ok(line)) = (std::fs::OpenOptions::new().create(true).append(true).open(file), serde_json::to_string(t)) {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    /// Persisted turns from earlier NOIDA sessions, newest first.
    pub fn history(&self) -> Vec<Turn> {
        let Some(Ok(f)) = self.history_file.as_ref().map(std::fs::File::open) else { return Vec::new() };
        let mut turns: Vec<Turn> = std::io::BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect();
        turns.reverse();
        turns.truncate(500);
        turns
    }

    /// Most recent turn (open or finished) for an agent name.
    pub fn last_turn(&self, agent: &str) -> Option<&Turn> {
        self.turns.iter().rev().find(|t| t.agent == agent)
    }
}

pub fn clock(at: u64) -> String {
    // Local time via libc would add a dependency; the TZ offset comes from `date`.
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let offset = *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim();
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let h: i64 = s.get(1..3)?.parse().ok()?;
                let m: i64 = s.get(3..5)?.parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    });
    let local = at as i64 + offset;
    format!("{:02}:{:02}", local.rem_euclid(86400) / 3600, local.rem_euclid(3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_turn() {
        let root = std::env::temp_dir();
        let mut a = Activity::new(root.clone()).without_history();
        let file = root.join(format!("noida-activity-new-{}.rs", std::process::id()));
        a.record(1, "claude", &AgentEvent::Prompt { text: "add feature".into() });
        a.record(1, "claude", &AgentEvent::ToolEnd { tool: "Read".into(), file: Some(root.join("x.rs")), detail: None });
        a.record(1, "claude", &AgentEvent::ToolStart { tool: "Write".into(), file: Some(file.clone()), detail: None });
        a.record(1, "claude", &AgentEvent::ToolEnd { tool: "Write".into(), file: Some(file.clone()), detail: None });
        a.record(1, "claude", &AgentEvent::ToolEnd { tool: "Bash".into(), file: None, detail: Some("cargo test".into()) });
        assert!(matches!(a.record(1, "claude", &AgentEvent::Notification { message: "needs permission".into(), permission: true }), Notice::Permission(_)));
        let notice = a.record(1, "claude", &AgentEvent::Stop);
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(notice, Notice::Finished { changed: vec![name] });
        let t = &a.turns[0];
        assert_eq!(t.summary(), "1 changed · 1 read · 1 cmds");
        assert_eq!(t.entries.first().unwrap().kind, EntryKind::Prompt);
        assert_eq!(t.entries.last().unwrap().kind, EntryKind::Finished);
        assert_eq!(a.touched[&file].0, Touch::Edited);
    }
}
