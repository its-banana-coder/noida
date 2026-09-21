//! Minimal Language Server Protocol client: diagnostics, definition and
//! references, with full-document sync.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::events::Bg;
use crate::symbols::Lang;

pub enum LspEvent {
    Message(usize, Value),
    Exited(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    /// 0-based; columns are UTF-16 units as sent by the server.
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    /// Original JSON, passed back when requesting code actions.
    pub raw: Value,
    /// 1 = error, 2 = warning, 3 = info, 4 = hint.
    pub severity: u8,
    pub message: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    Definition,
    References,
    Hover,
    SignatureHelp,
    Rename,
    CodeAction,
    Formatting,
    ExecuteCommand,
    /// Pull diagnostics (`textDocument/diagnostic`), used by servers that don't push them.
    Diagnostic,
}

/// Edits per file: ((line, utf16), (line, utf16), new text).
pub type TextEdit = ((usize, usize), (usize, usize), String);
pub type WorkspaceEdit = Vec<(PathBuf, Vec<TextEdit>)>;

/// (path, 0-based line, 0-based UTF-16 column)
pub type Location = (PathBuf, usize, usize);

pub enum Response {
    Locations(Request, Vec<Location>),
    Hover(String),
    /// Signature label and the active parameter's char range within it.
    Signature(String, Option<(usize, usize)>),
    Edit(WorkspaceEdit),
    CodeActions(Vec<Value>),
    Diagnostics,
    Ready(String),
    Error(String),
    None,
}

fn parse_text_edits(v: &Value) -> Vec<TextEdit> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let r = &e["range"];
                    Some((
                        (r["start"]["line"].as_u64()? as usize, r["start"]["character"].as_u64()? as usize),
                        (r["end"]["line"].as_u64()? as usize, r["end"]["character"].as_u64()? as usize),
                        e["newText"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_workspace_edit(v: &Value) -> WorkspaceEdit {
    let mut out: WorkspaceEdit = Vec::new();
    if let Some(changes) = v["changes"].as_object() {
        for (u, edits) in changes {
            if let Some(p) = path_from_uri(u) {
                out.push((p, parse_text_edits(edits)));
            }
        }
    }
    if let Some(dc) = v["documentChanges"].as_array() {
        for change in dc {
            if let Some(p) = change["textDocument"]["uri"].as_str().and_then(path_from_uri) {
                out.push((p, parse_text_edits(&change["edits"])));
            }
        }
    }
    out
}

fn hover_text(v: &Value) -> String {
    let part = |x: &Value| -> String {
        match x {
            Value::String(s) => s.clone(),
            Value::Object(_) => x["value"].as_str().unwrap_or("").to_string(),
            _ => String::new(),
        }
    };
    let raw = match &v["contents"] {
        Value::Array(a) => a.iter().map(part).collect::<Vec<_>>().join("\n\n"),
        other => part(other),
    };
    raw.lines().filter(|l| !l.trim_start().starts_with("```")).collect::<Vec<_>>().join("\n").trim().to_string()
}

fn server_for(lang: Lang) -> Option<(&'static str, &'static [&'static str])> {
    let candidates: &[(&str, &[&str])] = match lang {
        Lang::Rust => &[("rust-analyzer", &[])],
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => &[("typescript-language-server", &["--stdio"])],
        Lang::Python => &[("pyright-langserver", &["--stdio"]), ("pylsp", &[])],
        Lang::Go => &[("gopls", &[])],
        Lang::Java => &[("jdtls", &[])],
    };
    candidates.iter().copied().find(|(bin, _)| on_path(bin))
}

/// TypeScript 7 (the native compiler) ships its own language server as
/// `tsc --lsp --stdio`; typescript-language-server can't drive it. Prefer a
/// project-local compiler, then the one on PATH.
fn native_typescript(root: &Path) -> Option<String> {
    let local = root.join("node_modules/.bin/tsc");
    let candidates = [local.to_string_lossy().into_owned(), "tsc".to_string()];
    candidates.into_iter().find(|tsc| {
        (tsc == "tsc" && on_path("tsc") || Path::new(tsc).is_file())
            && Command::new(tsc)
                .arg("--version")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|v| v.split_whitespace().nth(1)?.split('.').next()?.parse::<u32>().ok())
                .is_some_and(|major| major >= 7)
    })
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

fn language_id(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "rust",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "typescriptreact",
        Lang::JavaScript => "javascript",
        Lang::Python => "python",
        Lang::Go => "go",
        Lang::Java => "java",
    }
}

pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

struct Server {
    name: String,
    stdin: ChildStdin,
    child: Child,
    ready: bool,
    next_id: u64,
    pending: HashMap<u64, Request>,
    /// Opened documents and their version.
    open: HashMap<PathBuf, i32>,
    queued: Vec<Value>,
    /// Server offers pull diagnostics.
    pull: bool,
    /// Server has pushed diagnostics, so pulling isn't needed.
    pushes: bool,
    pull_paths: HashMap<u64, PathBuf>,
}

impl Server {
    fn send(&mut self, msg: &Value) {
        let body = msg.to_string();
        let _ = write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body);
        let _ = self.stdin.flush();
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if self.ready || method == "initialized" {
            self.send(&msg);
        } else {
            self.queued.push(msg);
        }
    }
}

impl Server {
    fn pull_diagnostics(&mut self, path: &Path) {
        if !self.ready || !self.pull || self.pushes {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, Request::Diagnostic);
        self.pull_paths.insert(id, path.to_path_buf());
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "textDocument/diagnostic", "params": {"textDocument": {"uri": uri(path)}}}));
    }
}

fn parse_diagnostics(items: &Value) -> Vec<Diagnostic> {
    items
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| Diagnostic {
                    line: d["range"]["start"]["line"].as_u64().unwrap_or(0) as usize,
                    col: d["range"]["start"]["character"].as_u64().unwrap_or(0) as usize,
                    end_line: d["range"]["end"]["line"].as_u64().unwrap_or(0) as usize,
                    end_col: d["range"]["end"]["character"].as_u64().unwrap_or(0) as usize,
                    raw: d.clone(),
                    severity: d["severity"].as_u64().unwrap_or(1) as u8,
                    message: d["message"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string(),
                    source: d["source"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub struct Lsp {
    root: PathBuf,
    tx: Sender<Bg>,
    servers: Vec<Server>,
    by_lang: HashMap<Lang, Option<usize>>,
    /// Servers configured by file extension in settings.json. NOIDA only ships
    /// tree-sitter grammars for a handful of languages, and `Lang` is tied to
    /// those, so without this a language we have no grammar for -- SQL, C#,
    /// Kotlin, Terraform -- could never get a language server at all.
    by_ext: HashMap<String, Option<usize>>,
    configured: HashMap<String, Vec<String>>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

impl Lsp {
    pub fn new(root: PathBuf, tx: Sender<Bg>) -> Self {
        Self {
            root,
            tx,
            servers: Vec::new(),
            by_lang: HashMap::new(),
            by_ext: HashMap::new(),
            configured: HashMap::new(),
            diagnostics: HashMap::new(),
        }
    }

    /// Language servers from settings, as extension -> command line, e.g.
    /// `{"sql": "sqls", "java": "jdtls -data /tmp/jdtls"}`. A configured
    /// extension wins over the built-in default for that language, so a
    /// server that needs arguments can be spelled out.
    pub fn configure(&mut self, servers: &HashMap<String, String>) {
        self.configured = servers
            .iter()
            .filter_map(|(ext, cmd)| {
                let parts: Vec<String> = cmd.split_whitespace().map(String::from).collect();
                (!parts.is_empty()).then(|| (ext.trim_start_matches('.').to_ascii_lowercase(), parts))
            })
            .collect();
    }

    fn ext_of(path: &Path) -> Option<String> {
        Some(path.extension()?.to_str()?.to_ascii_lowercase())
    }

    /// The running server for this file, if there is one.
    fn index_for(&self, path: &Path) -> Option<usize> {
        if let Some(ext) = Self::ext_of(path) {
            if let Some(idx) = self.by_ext.get(&ext) {
                return *idx;
            }
        }
        Lang::for_path(path).and_then(|l| self.by_lang.get(&l).copied().flatten())
    }

    /// Start (or reuse) the server for this file.
    fn server_for_path(&mut self, path: &Path) -> Option<usize> {
        if let Some(ext) = Self::ext_of(path) {
            if self.configured.contains_key(&ext) {
                if let Some(idx) = self.by_ext.get(&ext) {
                    return *idx;
                }
                let cmd = self.configured.get(&ext).cloned()?;
                let args: Vec<&str> = cmd[1..].iter().map(String::as_str).collect();
                let idx = self.start(&cmd[0].clone(), &args);
                self.by_ext.insert(ext, idx);
                return idx;
            }
        }
        self.server(Lang::for_path(path)?)
    }

    pub fn server_name(&self, path: &Path) -> Option<&str> {
        Some(&self.servers[self.index_for(path)?].name)
    }

    fn server(&mut self, lang: Lang) -> Option<usize> {
        if let Some(idx) = self.by_lang.get(&lang) {
            return *idx;
        }
        let idx = self.spawn(lang);
        // TypeScript and JavaScript share one server.
        if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) {
            for l in [Lang::TypeScript, Lang::Tsx, Lang::JavaScript] {
                self.by_lang.insert(l, idx);
            }
        }
        self.by_lang.insert(lang, idx);
        idx
    }

    fn spawn(&mut self, lang: Lang) -> Option<usize> {
        let (bin, args): (String, Vec<&str>) = match native_typescript(&self.root).filter(|_| matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript)) {
            Some(tsc) => (tsc, vec!["--lsp", "--stdio"]),
            None => {
                let (bin, args) = server_for(lang)?;
                (bin.to_string(), args.to_vec())
            }
        };
        self.start(&bin, &args)
    }

    /// Launch one language server and wire up its stdout reader.
    fn start(&mut self, bin: &str, args: &[&str]) -> Option<usize> {
        let bin = bin.to_string();
        let name = Path::new(&bin).file_name().map_or(bin.clone(), |n| n.to_string_lossy().into_owned());
        let mut child = Command::new(&bin)
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let idx = self.servers.len();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut len = 0usize;
                loop {
                    let mut header = String::new();
                    match reader.read_line(&mut header) {
                        Ok(0) | Err(_) => {
                            let _ = tx.send(Bg::Lsp(LspEvent::Exited(idx)));
                            return;
                        }
                        Ok(_) => {}
                    }
                    let header = header.trim();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(v) = header.strip_prefix("Content-Length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                if reader.read_exact(&mut body).is_err() {
                    let _ = tx.send(Bg::Lsp(LspEvent::Exited(idx)));
                    return;
                }
                if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                    if tx.send(Bg::Lsp(LspEvent::Message(idx, v))).is_err() {
                        return;
                    }
                }
            }
        });
        let mut server = Server {
            name: if name == "tsc" { "tsc --lsp (TypeScript 7)".into() } else { name },
            stdin,
            child,
            ready: false,
            next_id: 1,
            pending: HashMap::new(),
            open: HashMap::new(),
            queued: Vec::new(),
            pull: false,
            pushes: false,
            pull_paths: HashMap::new(),
        };
        let root_uri = uri(&self.root);
        server.send(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": "root"}],
                "capabilities": {
                    "textDocument": {
                        "synchronization": {"didSave": true},
                        "publishDiagnostics": {"relatedInformation": false},
                        "definition": {"linkSupport": true},
                        "references": {},
                        "diagnostic": {"dynamicRegistration": false},
                        "hover": {"contentFormat": ["markdown", "plaintext"]},
                        "signatureHelp": {"signatureInformation": {"parameterInformation": {"labelOffsetSupport": true}, "activeParameterSupport": true}},
                        "rename": {"prepareSupport": false},
                        "formatting": {},
                        "rangeFormatting": {},
                        "codeAction": {
                            "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"]}},
                            "isPreferredSupport": true
                        }
                    },
                    "workspace": {"workspaceFolders": true, "configuration": true, "applyEdit": true, "workspaceEdit": {"documentChanges": true}},
                    "window": {"workDoneProgress": false}
                }
            }
        }));
        self.servers.push(server);
        Some(idx)
    }

    pub fn did_open(&mut self, path: &Path, text: &str) {
        let Some(idx) = self.server_for_path(path) else { return };
        let lang_id = match Lang::for_path(path) {
            Some(lang) => language_id(lang).to_string(),
            // No grammar: the extension is the best language id we have, and
            // it is what most servers expect anyway ("sql", "kt", "tf").
            None => Self::ext_of(path).unwrap_or_default(),
        };
        let s = &mut self.servers[idx];
        if s.open.contains_key(path) {
            return;
        }
        s.open.insert(path.to_path_buf(), 1);
        s.notify("textDocument/didOpen", json!({"textDocument": {"uri": uri(path), "languageId": lang_id, "version": 1, "text": text}}));
        s.pull_diagnostics(path);
    }

    pub fn did_change(&mut self, path: &Path, text: &str) {
        let Some(idx) = self.index_for(path) else { return };
        let s = &mut self.servers[idx];
        let Some(version) = s.open.get_mut(path) else { return };
        *version += 1;
        let v = *version;
        s.notify("textDocument/didChange", json!({"textDocument": {"uri": uri(path), "version": v}, "contentChanges": [{"text": text}]}));
        s.pull_diagnostics(path);
    }

    pub fn did_save(&mut self, path: &Path) {
        let Some(idx) = self.index_for(path) else { return };
        let s = &mut self.servers[idx];
        if s.open.contains_key(path) {
            s.notify("textDocument/didSave", json!({"textDocument": {"uri": uri(path)}}));
        }
    }

    pub fn did_close(&mut self, path: &Path) {
        let Some(idx) = self.index_for(path) else { return };
        let s = &mut self.servers[idx];
        if s.open.remove(path).is_some() {
            s.notify("textDocument/didClose", json!({"textDocument": {"uri": uri(path)}}));
        }
    }

    /// Returns false when no ready server handles this file.
    pub fn request(&mut self, kind: Request, path: &Path, line: usize, utf16_col: usize) -> bool {
        let params = match kind {
            Request::References => json!({"context": {"includeDeclaration": true}}),
            _ => json!({}),
        };
        let mut params = params;
        params["position"] = json!({"line": line, "character": utf16_col});
        self.request_with(kind, path, params)
    }

    /// Send a request for `path`; `params` gets `textDocument` filled in.
    pub fn request_with(&mut self, kind: Request, path: &Path, mut params: Value) -> bool {
        let Some(idx) = self.index_for(path) else { return false };
        let s = &mut self.servers[idx];
        if !s.ready || (!s.open.contains_key(path) && kind != Request::ExecuteCommand) {
            return false;
        }
        let id = s.next_id;
        s.next_id += 1;
        s.pending.insert(id, kind);
        let method = match kind {
            Request::Definition => "textDocument/definition",
            Request::References => "textDocument/references",
            Request::Hover => "textDocument/hover",
            Request::SignatureHelp => "textDocument/signatureHelp",
            Request::Rename => "textDocument/rename",
            Request::CodeAction => "textDocument/codeAction",
            Request::Formatting => {
                if params.get("range").is_some() { "textDocument/rangeFormatting" } else { "textDocument/formatting" }
            }
            Request::ExecuteCommand => "workspace/executeCommand",
            Request::Diagnostic => "textDocument/diagnostic",
        };
        if kind != Request::ExecuteCommand {
            params["textDocument"] = json!({"uri": uri(path)});
        }
        s.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        true
    }

    pub fn handle(&mut self, ev: LspEvent) -> Response {
        let (idx, msg) = match ev {
            LspEvent::Exited(idx) => {
                if let Some(s) = self.servers.get_mut(idx) {
                    s.ready = false;
                    s.open.clear();
                }
                self.by_lang.retain(|_, v| *v != Some(idx));
                self.by_ext.retain(|_, v| *v != Some(idx));
                return Response::None;
            }
            LspEvent::Message(idx, msg) => (idx, msg),
        };
        let Some(s) = self.servers.get_mut(idx) else { return Response::None };

        // Server → client requests need an answer or some servers stall.
        if let (Some(id), Some(method)) = (msg.get("id"), msg["method"].as_str()) {
            if method == "workspace/applyEdit" {
                s.send(&json!({"jsonrpc": "2.0", "id": id, "result": {"applied": true}}));
                return Response::Edit(parse_workspace_edit(&msg["params"]["edit"]));
            }
            let result = match method {
                "workspace/configuration" => {
                    let n = msg["params"]["items"].as_array().map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; n])
                }
                "workspace/workspaceFolders" => json!([{"uri": uri(&self.root), "name": "root"}]),
                _ => Value::Null,
            };
            s.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
            return Response::None;
        }

        if msg["method"] == "textDocument/publishDiagnostics" {
            let Some(path) = msg["params"]["uri"].as_str().and_then(path_from_uri) else { return Response::None };
            let diags = parse_diagnostics(&msg["params"]["diagnostics"]);
            // Some pull-based servers still send empty pushes; only real ones count.
            if !diags.is_empty() {
                s.pushes = true;
            } else if s.pull && !s.pushes {
                return Response::None;
            }
            if diags.is_empty() {
                self.diagnostics.remove(&path);
            } else {
                self.diagnostics.insert(path, diags);
            }
            return Response::Diagnostics;
        }

        let Some(id) = msg["id"].as_u64() else { return Response::None };
        if id == 0 {
            s.ready = true;
            s.pull = msg["result"]["capabilities"]["diagnosticProvider"].is_object();
            s.send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
            for m in std::mem::take(&mut s.queued) {
                s.send(&m);
            }
            let open: Vec<PathBuf> = s.open.keys().cloned().collect();
            for p in open {
                s.pull_diagnostics(&p);
            }
            return Response::Ready(s.name.clone());
        }
        let Some(kind) = s.pending.remove(&id) else { return Response::None };
        if kind == Request::Diagnostic {
            let Some(path) = s.pull_paths.remove(&id) else { return Response::None };
            if msg.get("error").is_some() || s.pushes {
                return Response::None;
            }
            // "unchanged" reports keep the previous diagnostics.
            if msg["result"]["kind"] == "full" {
                let diags = parse_diagnostics(&msg["result"]["items"]);
                if diags.is_empty() {
                    self.diagnostics.remove(&path);
                } else {
                    self.diagnostics.insert(path, diags);
                }
            }
            return Response::Diagnostics;
        }
        if let Some(err) = msg.get("error") {
            let text = err["message"].as_str().unwrap_or("request failed").lines().next().unwrap_or("").to_string();
            return Response::Error(format!("{}: {text}", s.name));
        }
        let result = &msg["result"];
        match kind {
            Request::Hover => return if result.is_null() { Response::Hover(String::new()) } else { Response::Hover(hover_text(result)) },
            Request::SignatureHelp => {
                let active_sig = result["activeSignature"].as_u64().unwrap_or(0) as usize;
                let Some(sig) = result["signatures"].get(active_sig) else { return Response::Signature(String::new(), None) };
                let label = sig["label"].as_str().unwrap_or("").to_string();
                let active_param = sig["activeParameter"].as_u64().or(result["activeParameter"].as_u64()).unwrap_or(0) as usize;
                let range = sig["parameters"].get(active_param).and_then(|p| match &p["label"] {
                    Value::String(name) => label.find(name.as_str()).map(|b| {
                        let s = label[..b].chars().count();
                        (s, s + name.chars().count())
                    }),
                    Value::Array(a) => Some((a.first()?.as_u64()? as usize, a.get(1)?.as_u64()? as usize)),
                    _ => None,
                });
                return Response::Signature(label, range);
            }
            Request::Rename => return Response::Edit(parse_workspace_edit(result)),
            // Formatting edits apply to the requesting document (empty path).
            Request::Formatting => return Response::Edit(vec![(PathBuf::new(), parse_text_edits(result))]),
            Request::CodeAction => return Response::CodeActions(result.as_array().cloned().unwrap_or_default()),
            Request::ExecuteCommand | Request::Diagnostic => return Response::None,
            Request::Definition | Request::References => {}
        }
        let items: Vec<&Value> = match result {
            Value::Array(a) => a.iter().collect(),
            Value::Object(_) => vec![result],
            _ => Vec::new(),
        };
        let locations = items
            .into_iter()
            .filter_map(|l| {
                let uri = l["uri"].as_str().or(l["targetUri"].as_str())?;
                let range = if l["targetSelectionRange"].is_object() { &l["targetSelectionRange"] } else { &l["range"] };
                Some((
                    path_from_uri(uri)?,
                    range["start"]["line"].as_u64()? as usize,
                    range["start"]["character"].as_u64()? as usize,
                ))
            })
            .collect();
        Response::Locations(kind, locations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_servers_cover_languages_we_have_no_grammar_for() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut lsp = Lsp::new(PathBuf::from("/tmp"), tx);

        let mut cfg = HashMap::new();
        cfg.insert("sql".to_string(), "sqls".to_string());
        cfg.insert(".KT".to_string(), "kotlin-language-server --stdio".to_string());
        cfg.insert("java".to_string(), "jdtls -data /tmp/ws".to_string());
        cfg.insert("blank".to_string(), "   ".to_string());
        lsp.configure(&cfg);

        // Leading dots and case are accepted: users write both.
        assert_eq!(lsp.configured.get("sql").unwrap(), &vec!["sqls".to_string()]);
        assert_eq!(
            lsp.configured.get("kt").unwrap(),
            &vec!["kotlin-language-server".to_string(), "--stdio".to_string()],
            "a leading dot and capitals are normalised"
        );
        // Arguments survive, which is the whole point for jdtls.
        assert_eq!(lsp.configured.get("java").unwrap().len(), 3);
        assert!(!lsp.configured.contains_key("blank"), "an empty command is not a server");

        // SQL has no Lang, so only configuration can give it a server.
        assert!(Lang::for_path(Path::new("q.sql")).is_none(), "no grammar for SQL");
        assert_eq!(Lsp::ext_of(Path::new("/a/q.SQL")).as_deref(), Some("sql"));

        // Nothing has been started, so nothing resolves yet.
        assert_eq!(lsp.index_for(Path::new("/a/q.sql")), None);
        assert_eq!(lsp.server_name(Path::new("/a/q.sql")), None);
    }

    #[test]
    fn built_in_servers_are_named_per_language() {
        // Guards the table itself: a typo here silently disables a language.
        for (lang, bin) in [
            (Lang::Rust, "rust-analyzer"),
            (Lang::Python, "pyright-langserver"),
            (Lang::Go, "gopls"),
            (Lang::Java, "jdtls"),
        ] {
            let candidates: Vec<&str> = match lang {
                Lang::Rust => vec!["rust-analyzer"],
                Lang::Python => vec!["pyright-langserver", "pylsp"],
                Lang::Go => vec!["gopls"],
                Lang::Java => vec!["jdtls"],
                _ => vec![],
            };
            assert!(candidates.contains(&bin), "{lang:?} should try {bin}");
            assert_eq!(language_id(lang), match lang {
                Lang::Rust => "rust",
                Lang::Python => "python",
                Lang::Go => "go",
                Lang::Java => "java",
                _ => unreachable!(),
            });
        }
    }

    #[test]
    fn uris() {
        let p = Path::new("/home/me/my project/a#b.rs");
        assert_eq!(uri(p), "file:///home/me/my%20project/a%23b.rs");
        assert_eq!(path_from_uri(&uri(p)).unwrap(), p);
    }

    /// Needs rust-analyzer on PATH: `cargo test lsp_live -- --ignored`.
    #[test]
    #[ignore]
    fn lsp_live_rust_analyzer() {
        let (tx, rx) = std::sync::mpsc::channel();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut lsp = Lsp::new(root.clone(), tx);
        let file = root.join("src/theme.rs");
        let text = std::fs::read_to_string(&file).unwrap() + "\nfn broken() -> u32 { \"no\" }\n";
        lsp.did_open(&file, &text);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut got_ready = false;
        while std::time::Instant::now() < deadline {
            let Ok(Bg::Lsp(ev)) = rx.recv_timeout(std::time::Duration::from_secs(5)) else { continue };
            match lsp.handle(ev) {
                Response::Ready(_) => got_ready = true,
                Response::Diagnostics if !lsp.diagnostics.is_empty() => {
                    println!("{:?}", lsp.diagnostics);
                    assert!(got_ready);
                    return;
                }
                _ => {}
            }
        }
        panic!("no diagnostics");
    }
}


#[cfg(test)]
mod probe {
    use super::*;

    /// `LSP_PROBE=<root>:<rel file>:<line>:<utf16 col> cargo test lsp_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lsp_probe() {
        let spec = std::env::var("LSP_PROBE").unwrap();
        let mut it = spec.split(':');
        let root = PathBuf::from(it.next().unwrap());
        let file = root.join(it.next().unwrap());
        let line: usize = it.next().unwrap().parse().unwrap();
        let col: usize = it.next().unwrap().parse().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut lsp = Lsp::new(root, tx);
        lsp.did_open(&file, &std::fs::read_to_string(&file).unwrap());
        let start = std::time::Instant::now();
        let mut stage = 0;
        let pos = json!({"line": line, "character": col});
        while start.elapsed() < std::time::Duration::from_secs(100) && stage < 6 {
            let ev = match rx.recv_timeout(std::time::Duration::from_secs(3)) {
                Ok(Bg::Lsp(ev)) => Some(ev),
                _ => None,
            };
            if let Some(LspEvent::Message(_, m)) = &ev {
                if m.get("method").is_none() && m["result"].get("items").is_some() {
                    println!("RAWDIAG {}", m.to_string().chars().take(400).collect::<String>());
                }
                if let Some(method) = m["method"].as_str() {
                    if method.starts_with("window/") || method == "$/typescriptVersion" {
                        println!("NOTE {method} {}", m["params"].to_string().chars().take(700).collect::<String>());
                    }
                }
            }
            let resp = ev.map(|e| lsp.handle(e));
            match resp {
                Some(Response::Ready(n)) => println!("READY {n}"),
                Some(Response::Diagnostics) => println!("DIAG {:?}", lsp.diagnostics.get(&file).map(|d| d.iter().map(|x| (x.line, x.message.clone())).collect::<Vec<_>>())),
                Some(Response::Hover(h)) => println!("HOVER {}", h.lines().take(3).collect::<Vec<_>>().join(" | ")),
                Some(Response::Locations(k, l)) => println!("LOC {k:?} {l:?}"),
                Some(Response::Edit(e)) => println!("EDIT {:?}", e.iter().map(|(p, x)| (p.display().to_string(), x.len())).collect::<Vec<_>>()),
                Some(Response::CodeActions(a)) => println!("ACTIONS {:?}", a.iter().map(|x| x["title"].as_str().unwrap_or("?").to_string()).collect::<Vec<_>>()),
                Some(Response::Error(e)) => println!("ERROR {e}"),
                _ => {}
            }
            if start.elapsed() > std::time::Duration::from_secs(12 + stage * 4) {
                let sent = match stage {
                    0 => lsp.request_with(Request::Hover, &file, json!({"position": pos})),
                    1 => lsp.request(Request::Definition, &file, line, col),
                    2 => lsp.request(Request::References, &file, line, col),
                    3 => lsp.request_with(Request::Formatting, &file, json!({"options": {"tabSize": 4, "insertSpaces": true}})),
                    4 => {
                        let diags: Vec<Value> = lsp.diagnostics.get(&file).map(|d| d.iter().map(|x| x.raw.clone()).collect()).unwrap_or_default();
                        lsp.request_with(Request::CodeAction, &file, json!({"range": {"start": pos, "end": pos}, "context": {"diagnostics": diags}}))
                    }
                    _ => lsp.request_with(Request::Rename, &file, json!({"position": pos, "newName": "sumValues"})),
                };
                println!("SENT stage {stage}: {sent}");
                stage += 1;
            }
        }
    }
}
