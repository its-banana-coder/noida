//! Structured agent events via Claude Code hooks and Codex `notify`.
//!
//! NOIDA listens on a local socket. Agents it launches get extra settings that
//! run `noida hook` (Claude) or `noida hook-codex` (Codex); those tiny client
//! processes forward the event JSON to the socket and exit without output, so
//! they never influence the agent's own decisions or permission prompts.
//!
//! The socket is a Unix socket where there is one, and a loopback TCP socket on
//! Windows. `NOIDA_SOCKET` carries whichever address applies, so the client side
//! only ever has to echo it back.

use std::io::{Read, Write};
#[cfg(windows)]
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::{Value, json};

use crate::events::Bg;

/// One connection from a hook client.
#[cfg(unix)]
type Stream = UnixStream;
#[cfg(windows)]
type Stream = TcpStream;

pub const ENV_SOCKET: &str = "NOIDA_SOCKET";
pub const ENV_AGENT: &str = "NOIDA_AGENT_ID";
pub const ENV_CONTEXT: &str = "NOIDA_CONTEXT";

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
    /// What to put in `NOIDA_SOCKET`: a socket path, or `host:port#token`.
    pub addr: String,
    /// File holding the user's current editor context, read by `noida hook`.
    pub context: PathBuf,
    /// Socket file to delete on exit; Windows has none.
    cleanup: Option<PathBuf>,
}

impl Server {
    pub fn start(tx: Sender<Bg>) -> std::io::Result<Server> {
        // Unix socket paths are limited to ~104 bytes; macOS's $TMPDIR is long, so prefer /tmp there.
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| {
            let tmp = std::env::temp_dir();
            if tmp.as_os_str().len() > 40 && Path::new("/tmp").is_dir() { PathBuf::from("/tmp") } else { tmp }
        });
        let context = dir.join(format!("noida-{}.ctx", std::process::id()));
        let _ = std::fs::write(&context, "");

        #[cfg(unix)]
        let (addr, cleanup, listener) = {
            let path = dir.join(format!("noida-{}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path)?;
            (path.to_string_lossy().into_owned(), Some(path), listener)
        };
        #[cfg(windows)]
        let (addr, cleanup, listener) = {
            // Any local process could reach a loopback port, so the address
            // carries a token that only the agents we launch are given.
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let token = token();
            (format!("{}#{token}", listener.local_addr()?), None, listener)
        };
        let want = expected_token(&addr);

        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some((agent, events)) = read_message(stream, want.as_deref()) {
                    for ev in events {
                        if tx.send(Bg::Hook(agent, ev)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Ok(Server { addr, context, cleanup })
    }
}

/// The token in a `host:port#token` address, if it has one.
fn expected_token(addr: &str) -> Option<String> {
    addr.split_once('#').map(|(_, t)| t.to_string())
}

#[cfg(windows)]
fn token() -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::process::id().hash(&mut h);
    std::time::SystemTime::now().hash(&mut h);
    std::time::Instant::now().hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(path) = &self.cleanup {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&self.context);
    }
}

fn read_message(mut stream: Stream, want: Option<&str>) -> Option<(usize, Vec<AgentEvent>)> {
    // macOS rejects SO_RCVTIMEO (EINVAL) once the peer has closed, which hook
    // clients do right after writing; the data is still readable, so ignore it.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    // Read until EOF; keep what arrived even if the read times out.
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
        }
    }
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let raw = match want {
        // A loopback socket is reachable by any local process, so drop anything
        // that cannot quote the token we handed the agent.
        Some(want) => {
            let (got, rest) = raw.split_once('\n')?;
            if got.trim() != want {
                return None;
            }
            rest.to_string()
        }
        None => raw,
    };
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
    let (Ok(sock), Ok(agent)) = (std::env::var(ENV_SOCKET), std::env::var(ENV_AGENT)) else { return };
    let Some(mut stream) = connect(&sock) else { return };
    let prefix = match sock.split_once('#') {
        Some((_, token)) => format!("{token}\n"),
        None => String::new(),
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(format!("{prefix}{agent}\n{source}\n{payload}").as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn connect(addr: &str) -> Option<Stream> {
    #[cfg(unix)]
    return UnixStream::connect(addr).ok();
    #[cfg(windows)]
    return TcpStream::connect(addr.split('#').next()?).ok();
}

/// For UserPromptSubmit, text printed to stdout is added to Claude's context.
/// Returns the editor context to print, if sharing is on and a file is open.
pub fn prompt_context(payload: &str) -> Option<String> {
    let v: Value = serde_json::from_str(payload).ok()?;
    if v["hook_event_name"] != "UserPromptSubmit" {
        return None;
    }
    let text = std::fs::read_to_string(std::env::var_os(ENV_CONTEXT)?).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
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
    // Hook commands run through a shell: POSIX quoting on Unix, cmd.exe on Windows.
    #[cfg(unix)]
    let command = format!("'{}' hook", exe.to_string_lossy().replace('\'', r"'\''"));
    #[cfg(windows)]
    let command = format!("\"{}\" hook", exe.to_string_lossy());
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
    fn prompt_context_only_for_prompts() {
        let file = std::env::temp_dir().join(format!("noida-ctx-test-{}", std::process::id()));
        std::fs::write(&file, "looking at src/a.rs\n").unwrap();
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var(ENV_CONTEXT, &file) };
        assert_eq!(prompt_context(r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#).as_deref(), Some("looking at src/a.rs"));
        assert_eq!(prompt_context(r#"{"hook_event_name":"Stop"}"#), None);
        std::fs::write(&file, "").unwrap();
        assert_eq!(prompt_context(r#"{"hook_event_name":"UserPromptSubmit"}"#), None);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn socket_roundtrip() {
        let (tx, rx) = std::sync::mpsc::channel();
        let server = Server::start(tx).unwrap();
        let mut s = connect(&server.addr).unwrap();
        let prefix = match server.addr.split_once('#') {
            Some((_, token)) => format!("{token}\n"),
            None => String::new(),
        };
        s.write_all(format!("{prefix}7\nclaude\n{{\"hook_event_name\":\"Stop\"}}").as_bytes()).unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        drop(s);
        match rx.recv_timeout(Duration::from_secs(10)).expect("hook event") {
            Bg::Hook(7, AgentEvent::Stop) => {}
            _ => panic!("unexpected event"),
        }
        let settings: Value = serde_json::from_str(&claude_settings(Path::new("/opt/it's/noida"))).unwrap();
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
        if cfg!(unix) {
            assert_eq!(command, "'/opt/it'\\''s/noida' hook");
        } else {
            assert_eq!(command, "\"/opt/it's/noida\" hook");
        }
    }
}
