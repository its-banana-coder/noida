//! Discovers past Claude Code and Codex conversations for a project so they
//! can be resumed in a new tab.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::actions::AgentKind;

#[derive(Clone, Debug)]
pub struct PastSession {
    pub kind: AgentKind,
    pub id: String,
    pub title: String,
    pub modified: SystemTime,
}

impl PastSession {
    /// Command line that resumes this session.
    #[cfg(test)]
    pub fn resume_command(&self) -> String {
        match self.kind {
            AgentKind::Claude => format!("claude --resume {}", self.id),
            AgentKind::Codex => format!("codex resume {}", self.id),
            AgentKind::Shell => String::new(),
        }
    }
}

/// Claude stores transcripts under a directory named after the cwd with every
/// non-alphanumeric character replaced by `-`.
pub fn claude_project_dir(home: &Path, cwd: &Path) -> PathBuf {
    let escaped: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(escaped)
}

pub fn list(cwd: &Path) -> Vec<PastSession> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut out = claude_sessions(&home, cwd);
    out.extend(codex_sessions(&home, cwd));
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

fn claude_sessions(home: &Path, cwd: &Path) -> Vec<PastSession> {
    let Ok(dir) = std::fs::read_dir(claude_project_dir(home, cwd)) else { return Vec::new() };
    let mut files: Vec<(SystemTime, PathBuf)> = dir
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .take(100)
        .filter_map(|(modified, path)| {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let title = claude_title(&path)?;
            Some(PastSession { kind: AgentKind::Claude, id, title, modified })
        })
        .collect()
}

/// Prefer the AI/custom title; fall back to the first real user prompt.
/// Returns None for transcripts with no user prompt (nothing to resume).
fn claude_title(path: &Path) -> Option<String> {
    let mut buf = Vec::new();
    File::open(path).ok()?.take(512 * 1024).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let (mut custom, mut ai, mut first_prompt) = (None, None, None);
    for line in text.lines() {
        if !(line.contains("\"user\"") || line.contains("itle\"")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") => custom = v["customTitle"].as_str().map(String::from),
            Some("ai-title") => ai = v["aiTitle"].as_str().map(String::from),
            Some("user") if first_prompt.is_none() => {
                first_prompt = v["message"]["content"].as_str().filter(|p| is_real_prompt(p)).map(String::from);
            }
            _ => {}
        }
    }
    let prompt = first_prompt?;
    Some(one_line(custom.or(ai).as_deref().unwrap_or(&prompt)))
}

fn codex_sessions(home: &Path, cwd: &Path) -> Vec<PastSession> {
    let mut files = Vec::new();
    collect_jsonl(&home.join(".codex/sessions"), 0, &mut files);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .take(300)
        .filter_map(|(modified, path)| {
            let (id, session_cwd, title) = codex_meta(&path)?;
            (session_cwd == cwd).then(|| PastSession { kind: AgentKind::Codex, id, title, modified })
        })
        .take(100)
        .collect()
}

fn collect_jsonl(dir: &Path, depth: usize, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() && depth < 4 {
            collect_jsonl(&p, depth + 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                out.push((m, p));
            }
        }
    }
}

fn codex_meta(path: &Path) -> Option<(String, PathBuf, String)> {
    let reader = BufReader::new(File::open(path).ok()?);
    let mut lines = reader.lines();
    let meta: Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
    let payload = &meta["payload"];
    let id = payload["id"].as_str().or(payload["session_id"].as_str())?.to_string();
    let cwd = PathBuf::from(payload["cwd"].as_str()?);
    for line in lines.take(60).map_while(Result::ok) {
        if !line.contains("\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let p = &v["payload"];
        if p["type"] == "message" && p["role"] == "user" {
            let text = p["content"].as_array().and_then(|c| c.iter().find_map(|x| x["text"].as_str()));
            if let Some(t) = text.filter(|t| is_real_prompt(t)) {
                return Some((id, cwd, one_line(t)));
            }
        }
    }
    Some((id, cwd, "(no prompt)".into()))
}

fn is_real_prompt(p: &str) -> bool {
    let t = p.trim_start();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")
}

fn one_line(s: &str) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 90 {
        line.chars().take(89).collect::<String>() + "…"
    } else {
        line
    }
}

pub fn ago(t: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(t).map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_dir_escaping() {
        let d = claude_project_dir(Path::new("/h"), Path::new("/home/me/my_app.v2"));
        assert_eq!(d, Path::new("/h/.claude/projects/-home-me-my-app-v2"));
    }

    #[test]
    fn parses_transcripts() {
        let dir = std::env::temp_dir().join(format!("noida-sessions-{}", std::process::id()));
        let cwd = Path::new("/work/proj");
        let cdir = claude_project_dir(&dir, cwd);
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(
            cdir.join("abc.jsonl"),
            concat!(
                "{\"type\":\"mode\",\"sessionId\":\"abc\"}\n",
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"<command-name>/clear</command-name>\"}}\n",
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix   the login\\nbug\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Fix login bug\"}\n",
            ),
        )
        .unwrap();
        std::fs::write(cdir.join("empty.jsonl"), "{\"type\":\"mode\"}\n").unwrap();
        let xdir = dir.join(".codex/sessions/2026/01/02");
        std::fs::create_dir_all(&xdir).unwrap();
        std::fs::write(
            xdir.join("rollout-1.jsonl"),
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"x1\",\"cwd\":\"/work/proj\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"add caching\"}]}}\n",
            ),
        )
        .unwrap();
        std::fs::write(xdir.join("rollout-2.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"x2\",\"cwd\":\"/elsewhere\"}}\n").unwrap();

        let claude = claude_sessions(&dir, cwd);
        assert_eq!(claude.len(), 1);
        assert_eq!((claude[0].id.as_str(), claude[0].title.as_str()), ("abc", "Fix login bug"));
        let codex = codex_sessions(&dir, cwd);
        assert_eq!(codex.len(), 1);
        assert_eq!((codex[0].id.as_str(), codex[0].title.as_str()), ("x1", "add caching"));
        assert_eq!(codex[0].resume_command(), "codex resume x1");
        std::fs::remove_dir_all(&dir).ok();
    }
}

