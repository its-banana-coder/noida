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
    /// Transcript file, for browsing the conversation.
    pub path: PathBuf,
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
            Some(PastSession { kind: AgentKind::Claude, id, title, modified, path })
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
            (session_cwd == cwd).then(|| PastSession { kind: AgentKind::Codex, id, title, modified, path })
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

/// Title of a Claude session (custom or AI-generated), for naming its tab.
pub fn claude_session_title(cwd: &Path, id: &str) -> Option<String> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    claude_title(&claude_project_dir(&home, cwd).join(format!("{id}.jsonl")))
}

/// Renders a transcript as Markdown: prompts, replies and compact tool steps.
pub fn transcript_markdown(kind: AgentKind, path: &Path) -> String {
    let mut buf = Vec::new();
    if File::open(path).and_then(|f| f.take(8 * 1024 * 1024).read_to_end(&mut buf)).is_err() {
        return format!("Could not read `{}`.", path.display());
    }
    let text = String::from_utf8_lossy(&buf);
    let mut out = String::new();
    let mut last_role = "";
    let mut push = |role: &'static str, body: &str, out: &mut String| {
        let body = body.trim();
        if body.is_empty() {
            return;
        }
        if role != last_role {
            let heading = match role {
                "you" => "### You",
                "agent" => if kind == AgentKind::Codex { "### Codex" } else { "### Claude" },
                _ => "",
            };
            if !heading.is_empty() {
                out.push_str(&format!("\n{heading}\n\n"));
            }
            last_role = role;
        }
        out.push_str(body);
        out.push_str("\n\n");
    };
    let tool_line = |name: &str, input: &Value| -> String {
        let detail = ["file_path", "command", "pattern", "path", "url", "description", "query"]
            .iter()
            .find_map(|k| input.get(*k).and_then(Value::as_str))
            .map(|d| one_line(d))
            .unwrap_or_default();
        format!("- ⚙ **{name}** `{detail}`")
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match kind {
            AgentKind::Codex => {
                let p = &v["payload"];
                match (p["type"].as_str(), p["role"].as_str()) {
                    (Some("message"), Some(role @ ("user" | "assistant"))) => {
                        let body: Vec<&str> = p["content"].as_array().map(|c| c.iter().filter_map(|x| x["text"].as_str()).filter(|t| is_real_prompt(t)).collect()).unwrap_or_default();
                        push(if role == "user" { "you" } else { "agent" }, &body.join("\n\n"), &mut out);
                    }
                    (Some("function_call"), _) => {
                        let args: Value = p["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
                        let name = p["name"].as_str().unwrap_or("tool");
                        let cmd = args["command"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "));
                        let line = match cmd {
                            Some(c) => format!("- ⚙ **{name}** `{}`", one_line(&c)),
                            None => tool_line(name, &args),
                        };
                        push("agent", &line, &mut out);
                    }
                    _ => {}
                }
            }
            _ => {
                if v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) {
                    continue;
                }
                let content = &v["message"]["content"];
                match v["type"].as_str() {
                    Some("user") => {
                        if let Some(t) = content.as_str().filter(|t| is_real_prompt(t)) {
                            push("you", t, &mut out);
                        } else if let Some(items) = content.as_array() {
                            let texts: Vec<&str> = items.iter().filter(|i| i["type"] == "text").filter_map(|i| i["text"].as_str()).filter(|t| is_real_prompt(t)).collect();
                            push("you", &texts.join("\n\n"), &mut out);
                        }
                    }
                    Some("assistant") => {
                        for item in content.as_array().into_iter().flatten() {
                            match item["type"].as_str() {
                                Some("text") => push("agent", item["text"].as_str().unwrap_or(""), &mut out),
                                Some("tool_use") => push("agent", &tool_line(item["name"].as_str().unwrap_or("tool"), &item["input"]), &mut out),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if out.trim().is_empty() {
        "This conversation has no messages to show.".into()
    } else {
        out
    }
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
        let md = transcript_markdown(AgentKind::Claude, &claude[0].path);
        assert!(md.contains("### You") && md.contains("the login"), "{md}");
        let cx = transcript_markdown(AgentKind::Codex, &codex[0].path);
        assert!(cx.contains("add caching") && !cx.contains("environment_context"), "{cx}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

