//! Structured agent events via Claude Code hooks and Codex `notify`.
//!
//! NOIDA listens on a Unix socket. Agents it launches get extra settings that
//! run `noida hook` (Claude) or `noida hook-codex` (Codex); those tiny client
//! processes forward the event JSON to the socket and exit without output, so
//! they never influence the agent's own decisions or permission prompts.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::{Value, json};

use crate::events::Bg;

pub const ENV_SOCKET: &str = "NOIDA_SOCKET";
pub const ENV_AGENT: &str = "NOIDA_AGENT_ID";

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    SessionStart { session_id: String },
    Prompt { text: String },
    ToolStart { tool: String, file: Option<PathBuf>, detail: Option<String> },
    ToolEnd { tool: String, file: Option<PathBuf>, detail: Option<String> },
    /// Claude is waiting: a permission prompt or idle input.
    Notification { message: String, permission: bool },
    Stop,
}

pub struct Server {
    pub path: PathBuf,
}

impl Server {
    pub fn start(tx: Sender<Bg>) -> std::io::Result<Server> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("noida-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some((agent, events)) = read_message(stream) {
                    for ev in events {
                        if tx.send(Bg::Hook(agent, ev)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Ok(Server { path })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_message(mut stream: UnixStream) -> Option<(usize, Vec<AgentEvent>)> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let mut parts = raw.splitn(3, '\n');
    let agent: usize = parts.next()?.trim().parse().ok()?;
    let source = parts.next()?.trim().to_string();
    let payload: Value = serde_json::from_str(parts.next()?).ok()?;
    let events = match source.as_str() {
        "claude" => parse_claude(&payload).into_iter().collect(),
        "codex" => parse_codex(&payload),
        _ => Vec::new(),
    };
    Some((agent, events))
}

/// Client side: forward one event to the running NOIDA. Never fails loudly.
pub fn forward(source: &str, payload: &str) {
    let (Some(sock), Some(agent)) = (std::env::var_os(ENV_SOCKET), std::env::var(ENV_AGENT).ok()) else { return };
    let Ok(mut stream) = UnixStream::connect(sock) else { return };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(format!("{agent}\n{source}\n{payload}").as_bytes());
}

pub fn parse_claude(v: &Value) -> Option<AgentEvent> {
    let s = |k: &str| v[k].as_str().map(String::from);
    let tool_input = &v["tool_input"];
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| tool_input[k].as_str())
        .map(PathBuf::from);
    let detail = ["command", "pattern", "url", "query", "description"]
        .iter()
        .find_map(|k| tool_input[k].as_str())
        .map(String::from);
    Some(match v["hook_event_name"].as_str()? {
        "SessionStart" => AgentEvent::SessionStart { session_id: s("session_id")? },
        "UserPromptSubmit" => AgentEvent::Prompt { text: s("prompt").unwrap_or_default() },
        "PreToolUse" => AgentEvent::ToolStart { tool: s("tool_name")?, file, detail },
        "PostToolUse" => AgentEvent::ToolEnd { tool: s("tool_name")?, file, detail },
        "Notification" => {
            let message = s("message").unwrap_or_default();
            let kind = v["notification_type"].as_str().unwrap_or("");
            let permission = kind.contains("permission") || message.to_lowercase().contains("permission");
            AgentEvent::Notification { message, permission }
        }
        "Stop" => AgentEvent::Stop,
        _ => return None,
    })
}

/// Codex only reports finished turns via `notify`.
pub fn parse_codex(v: &Value) -> Vec<AgentEvent> {
    match v["type"].as_str() {
        Some("agent-turn-complete") => {
            let prompt = v["input-messages"]
                .as_array()
                .and_then(|m| m.last())
                .and_then(Value::as_str)
                .map(|t| AgentEvent::Prompt { text: t.to_string() });
            prompt.into_iter().chain([AgentEvent::Stop]).collect()
        }
        _ => Vec::new(),
    }
}

/// `--settings` JSON adding NOIDA's hooks on top of the user's own settings.
pub fn claude_settings(exe: &Path) -> String {
    let command = format!("'{}' hook", exe.to_string_lossy().replace('\'', r"'\''"));
    let hook = json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    let plain = json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    json!({
        "hooks": {
            "SessionStart": plain,
            "UserPromptSubmit": plain,
            "PreToolUse": hook,
            "PostToolUse": hook,
            "Notification": plain,
            "Stop": plain,
        }
    })
    .to_string()
}

/// `-c notify=[...]` value for Codex.
pub fn codex_notify(exe: &Path) -> String {
    format!("notify={}", json!([exe.to_string_lossy(), "hook-codex"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_hooks() {
        let pre = json!({"hook_event_name": "PreToolUse", "tool_name": "Edit", "tool_input": {"file_path": "/p/a.rs", "old_string": "x"}});
        assert_eq!(
            parse_claude(&pre),
            Some(AgentEvent::ToolStart { tool: "Edit".into(), file: Some("/p/a.rs".into()), detail: None })
        );
        let bash = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "cargo test"}});
        assert_eq!(
            parse_claude(&bash),
            Some(AgentEvent::ToolEnd { tool: "Bash".into(), file: None, detail: Some("cargo test".into()) })
        );
        let note = json!({"hook_event_name": "Notification", "message": "Claude needs your permission to use Bash"});
        assert!(matches!(parse_claude(&note), Some(AgentEvent::Notification { permission: true, .. })));
        let codex = json!({"type": "agent-turn-complete", "input-messages": ["fix it"], "last-assistant-message": "done"});
        assert_eq!(parse_codex(&codex), vec![AgentEvent::Prompt { text: "fix it".into() }, AgentEvent::Stop]);
    }

    #[test]
    fn socket_roundtrip() {
        let (tx, rx) = std::sync::mpsc::channel();
        let server = Server::start(tx).unwrap();
        let mut s = UnixStream::connect(&server.path).unwrap();
        s.write_all(b"7\nclaude\n{\"hook_event_name\":\"Stop\"}").unwrap();
        drop(s);
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Bg::Hook(7, AgentEvent::Stop) => {}
            _ => panic!("unexpected event"),
        }
        let settings: Value = serde_json::from_str(&claude_settings(Path::new("/opt/it's/noida"))).unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "'/opt/it'\\''s/noida' hook");
    }
}
