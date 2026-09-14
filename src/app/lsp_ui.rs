//! Editor-facing language server features: hover and signature popups,
//! rename, code actions, formatting and applying workspace edits.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Clear, Widget};
use serde_json::{Value, json};

use super::{App, Mode, PromptKind};
use crate::editor::Doc;
use crate::lsp::{Request, WorkspaceEdit};
use crate::picker::{Item, Kind, Picker, Target};
use crate::refs;
use crate::theme;

#[derive(Clone, Copy, PartialEq)]
pub enum PopupKind {
    Hover,
    Signature,
}

pub struct Popup {
    pub kind: PopupKind,
    pub lines: Vec<String>,
    /// Highlighted char range on the first line (active parameter).
    pub highlight: Option<(usize, usize)>,
}

impl App {
    fn doc_request(&mut self, kind: Request, params: Value) -> bool {
        let Some(doc) = self.docs.get(self.active_doc) else { return false };
        let path = doc.path.clone();
        let (line, col) = (doc.cursor_line0(), doc.utf16_col());
        let mut params = params;
        if params.get("position").is_none() && matches!(kind, Request::Hover | Request::SignatureHelp | Request::Rename) {
            params["position"] = json!({"line": line, "character": col});
        }
        self.lsp.request_with(kind, &path, params)
    }

    fn no_server(&mut self, what: &str) {
        let has = self.docs.get(self.active_doc).and_then(|d| self.lsp.server_name(&d.path)).is_some();
        self.info(if has { format!("{what}: language server is still starting") } else { format!("{what} needs a language server for this file type") });
    }

    pub(super) fn hover(&mut self) {
        if !self.doc_request(Request::Hover, json!({})) {
            self.no_server("hover");
        }
    }

    pub(super) fn signature_help(&mut self) {
        self.doc_request(Request::SignatureHelp, json!({}));
    }

    pub(super) fn prompt_rename_symbol(&mut self) {
        let Some(word) = self.docs.get(self.active_doc).and_then(Doc::word_at_cursor) else { return self.info("no symbol at cursor") };
        self.mode = Mode::Prompt { kind: PromptKind::RenameSymbol, input: word };
    }

    pub(super) fn rename_symbol(&mut self, new_name: &str) {
        if new_name.trim().is_empty() {
            return;
        }
        if !self.doc_request(Request::Rename, json!({"newName": new_name.trim()})) {
            self.no_server("rename");
        }
    }

    fn selection_range(&self) -> Option<Value> {
        let doc = self.docs.get(self.active_doc)?;
        let (s, e) = doc.selected_lines();
        let line = doc.cursor_line0();
        if doc.has_selection() {
            let end_len = doc.line_text(e - 1).map_or(0, |t| t.encode_utf16().count());
            Some(json!({"start": {"line": s - 1, "character": 0}, "end": {"line": e - 1, "character": end_len}}))
        } else {
            let col = doc.utf16_col();
            Some(json!({"start": {"line": line, "character": col}, "end": {"line": line, "character": col}}))
        }
    }

    pub(super) fn code_actions(&mut self, only: Option<&str>) {
        let Some(doc) = self.docs.get(self.active_doc) else { return };
        let line = doc.cursor_line0();
        let diags: Vec<Value> = self.lsp.diagnostics.get(&doc.path).map(|ds| ds.iter().filter(|d| d.line <= line && line <= d.end_line).map(|d| d.raw.clone()).collect()).unwrap_or_default();
        let Some(range) = self.selection_range() else { return };
        let mut context = json!({"diagnostics": diags});
        if let Some(kind) = only {
            context["only"] = json!([kind]);
        }
        self.organize_pending = only.is_some();
        if !self.doc_request(Request::CodeAction, json!({"range": range, "context": context})) {
            self.no_server("code actions");
        }
    }

    pub(super) fn format(&mut self, selection: bool) {
        let Some(doc) = self.docs.get(self.active_doc) else { return };
        let insert_spaces = !doc.line_text(0).is_some_and(|_| doc.uses_tabs());
        let mut params = json!({"options": {"tabSize": crate::settings::tab_width(), "insertSpaces": insert_spaces}});
        if selection {
            params["range"] = self.selection_range().unwrap_or(Value::Null);
        }
        self.pending_format = Some(doc.path.clone());
        if !self.doc_request(Request::Formatting, params) {
            self.pending_format = None;
            self.no_server("format");
        }
    }

    pub(super) fn on_hover(&mut self, text: String) {
        if text.is_empty() {
            return self.info("no hover information");
        }
        let lines: Vec<String> = text.lines().take(20).map(String::from).collect();
        self.popup = Some(Popup { kind: PopupKind::Hover, lines, highlight: None });
    }

    pub(super) fn on_signature(&mut self, label: String, highlight: Option<(usize, usize)>) {
        self.popup = (!label.is_empty()).then(|| Popup { kind: PopupKind::Signature, lines: vec![label], highlight });
    }

    pub(super) fn on_code_actions(&mut self, actions: Vec<Value>) {
        if actions.is_empty() {
            self.organize_pending = false;
            return self.info("no code actions available here");
        }
        self.code_action_list = actions;
        if std::mem::take(&mut self.organize_pending) {
            return self.apply_code_action(0);
        }
        let items = self
            .code_action_list
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let kind = a["kind"].as_str().unwrap_or("");
                let preferred = if a["isPreferred"].as_bool() == Some(true) { "★ " } else { "" };
                Item::new(format!("{preferred}{}", a["title"].as_str().unwrap_or("action")), kind, Target::CodeAction(i))
            })
            .collect();
        self.mode = Mode::Picker(Picker::new("Code Actions", Kind::Static, items).ordered());
    }

    pub(super) fn apply_code_action(&mut self, i: usize) {
        let Some(action) = self.code_action_list.get(i).cloned() else { return };
        if action["edit"].is_object() {
            let edit = crate::lsp::parse_workspace_edit(&action["edit"]);
            self.apply_workspace_edit(edit);
        }
        // Either a Command itself or a CodeAction carrying one.
        let command = if action["command"].is_string() { Some(action.clone()) } else { action.get("command").filter(|c| c.is_object()).cloned() };
        if let Some(cmd) = command {
            let path = self.docs.get(self.active_doc).map(|d| d.path.clone()).unwrap_or_default();
            let params = json!({"command": cmd["command"], "arguments": cmd.get("arguments").cloned().unwrap_or(json!([]))});
            self.lsp.request_with(Request::ExecuteCommand, &path, params);
        }
        self.info(format!("applied: {}", action["title"].as_str().unwrap_or("code action")));
    }

    pub(super) fn apply_workspace_edit(&mut self, edit: WorkspaceEdit) {
        let (mut files, mut edits) = (0, 0);
        for (path, file_edits) in edit {
            if file_edits.is_empty() {
                continue;
            }
            let path: PathBuf = if path.as_os_str().is_empty() {
                match self.pending_format.take() {
                    Some(p) => p,
                    None => continue,
                }
            } else {
                path
            };
            edits += file_edits.len();
            files += 1;
            if let Some(doc) = self.docs.iter_mut().find(|d| d.path == path) {
                doc.apply_lsp_edits(file_edits);
                doc.preview = false;
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let mut doc = Doc::from_text(&path, &text, &self.syntax);
                doc.apply_lsp_edits(file_edits);
                if let Err(e) = doc.save() {
                    self.error(format!("could not write {}: {e}", refs::relative(&self.root, &path)));
                }
            }
        }
        if files > 0 {
            self.info(format!("applied {edits} edits in {files} file{}", if files == 1 { "" } else { "s" }));
        }
    }

    pub(super) fn render_popup(&self, buf: &mut Buffer, bounds: Rect, cursor: Option<(u16, u16)>) {
        let (Some(popup), Some((cx, cy))) = (&self.popup, cursor) else { return };
        let width = popup.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).min(90) as u16 + 2;
        let height = popup.lines.len().min(14) as u16;
        if width < 4 || bounds.width < 10 {
            return;
        }
        let width = width.min(bounds.width);
        let below = cy + 1 + height <= bounds.y + bounds.height;
        let y = if popup.kind == PopupKind::Signature || !below { cy.saturating_sub(height).max(bounds.y) } else { cy + 1 };
        let x = cx.min(bounds.x + bounds.width - width);
        let rect = Rect::new(x, y, width, height);
        Clear.render(rect, buf);
        let bg = Style::default().bg(theme::STATUS_BG()).fg(theme::FG());
        buf.set_style(rect, bg);
        for (i, line) in popup.lines.iter().take(height as usize).enumerate() {
            let yy = y + i as u16;
            if i == 0 && popup.highlight.is_some() {
                let (hs, he) = popup.highlight.unwrap();
                for (ci, ch) in line.chars().enumerate().take(width as usize - 2) {
                    let mut st = bg;
                    if ci >= hs && ci < he {
                        st = st.fg(theme::ACCENT()).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
                    }
                    buf[(x + 1 + ci as u16, yy)].set_char(ch).set_style(st);
                }
            } else {
                buf.set_stringn(x + 1, yy, line, width as usize - 2, bg);
            }
        }
    }
}
