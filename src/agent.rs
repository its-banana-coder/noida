//! Agent panes: real PTYs running `claude`, `codex`, or a shell.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::actions::AgentKind;
use crate::events::Bg;
use crate::refs::FileRef;
use crate::theme;

const SCROLLBACK: usize = 10_000;

/// Answers terminal queries (cursor position, device attributes, colors) that
/// TUIs like codex send on startup, and forwards clipboard writes to the host.
#[derive(Default)]
pub struct Responder {
    pub to_pty: Vec<u8>,
    pub to_host: Vec<u8>,
    pub bell: bool,
}

impl vt100::Callbacks for Responder {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
        let p0 = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (i1, i2, c, p0) {
            (None, None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            (None, None, 'n', 5) => "\x1b[0n".into(),
            (None, None, 'c', _) => "\x1b[?62;22c".into(),
            (Some(b'>'), None, 'c', _) => "\x1b[>1;10;0c".into(),
            (Some(b'?'), Some(b'$'), 'p', mode) => format!("\x1b[?{mode};0$y"),
            (None, None, 't', 18) => {
                let (rows, cols) = screen.size();
                format!("\x1b[8;{rows};{cols}t")
            }
            (None, None, 't', 14) => {
                let (rows, cols) = screen.size();
                format!("\x1b[4;{};{}t", rows as u32 * 16, cols as u32 * 8)
            }
            (None, None, 't', 16) => "\x1b[6;16;8t".into(),
            _ => return,
        };
        self.to_pty.extend_from_slice(reply.as_bytes());
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        let color = match params {
            [b"10", b"?"] => Some(("10", theme::FG())),
            [b"11", b"?"] => Some(("11", theme::BG())),
            [b"12", b"?"] => Some(("12", theme::FG())),
            _ => None,
        };
        if let Some((code, Color::Rgb(r, g, b))) = color {
            let reply = format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\");
            self.to_pty.extend_from_slice(reply.as_bytes());
        }
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        self.to_host.extend_from_slice(b"\x1b]52;");
        self.to_host.extend_from_slice(ty);
        self.to_host.push(b';');
        self.to_host.extend_from_slice(data);
        self.to_host.push(0x07);
    }
}

struct Process {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

pub struct Agent {
    pub id: usize,
    pub name: String,
    pub command: String,
    pub kind: Option<AgentKind>,
    pub cwd: PathBuf,
    /// Claude conversation id, so the tab can be resumed next launch.
    pub session_id: Option<String>,
    /// (worktree path, branch) for agents isolated in a git worktree.
    pub worktree: Option<(PathBuf, String)>,
    /// Set when hooks report working/idle precisely.
    pub hook_working: Option<bool>,
    started_at: Option<Instant>,
    program: String,
    args: Vec<String>,
    pub parser: vt100::Parser<Responder>,
    proc: Option<Process>,
    pub exited: bool,
    pub error: Option<String>,
    size: (u16, u16),
    last_output: Option<Instant>,
    last_input: Option<Instant>,
    working: bool,
    /// Rang the bell, or finished work while in a background tab.
    attention: Option<Attention>,
    /// Current find match as (absolute line, start col, end col exclusive).
    pub find_match: Option<(usize, u16, u16)>,
    /// Mouse selection as (anchor, head), each (absolute line, col), inclusive.
    pub selection: Option<((usize, u16), (usize, u16))>,
}

#[derive(Clone, Copy, PartialEq)]
enum Attention {
    Bell,
    Finished,
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Agent {
    pub fn new(id: usize, name: &str, command: &str, cwd: PathBuf) -> Self {
        let mut parts = command.split_whitespace().map(String::from);
        let program = parts.clone().next().unwrap_or_default();
        let kind = match Path::new(&program).file_name().and_then(|n| n.to_str()) {
            Some("claude") => Some(AgentKind::Claude),
            Some("codex") => Some(AgentKind::Codex),
            _ => None,
        };
        Self {
            id,
            name: name.to_string(),
            command: command.to_string(),
            kind,
            cwd,
            session_id: None,
            worktree: None,
            hook_working: None,
            started_at: None,
            program: parts.next().unwrap_or_default(),
            args: parts.collect(),
            parser: vt100::Parser::new_with_callbacks(24, 80, SCROLLBACK, Responder::default()),
            proc: None,
            exited: false,
            error: None,
            size: (24, 80),
            last_output: None,
            last_input: None,
            working: false,
            attention: None,
            find_match: None,
            selection: None,
        }
    }

    pub fn running(&self) -> bool {
        self.proc.is_some() && !self.exited
    }

    pub fn started(&self) -> bool {
        self.proc.is_some() || self.error.is_some()
    }

    /// Start the process with extra arguments (hooks, session flags) and environment.
    pub fn start(&mut self, tx: Sender<Bg>, extra_args: &[String], env: &[(String, String)]) {
        self.error = None;
        if let Err(e) = self.try_start(tx, extra_args, env) {
            self.error = Some(format!("{e:#}"));
        }
    }

    /// Exited within a few seconds of starting (e.g. a failed `--resume`).
    pub fn died_quickly(&self) -> bool {
        self.exited && self.started_at.is_some_and(|t| t.elapsed() < Duration::from_secs(5))
    }

    /// Forget the finished process so the tab can be started again.
    pub fn reset(&mut self) {
        self.proc = None;
        self.error = None;
        self.exited = false;
    }

    pub fn alert(&mut self) {
        self.attention = Some(Attention::Bell);
    }

    fn try_start(&mut self, tx: Sender<Bg>, extra_args: &[String], env: &[(String, String)]) -> Result<()> {
        let (rows, cols) = self.size;
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("failed to open pty")?;
        let mut cmd = CommandBuilder::new(&self.program);
        cmd.args(&self.args);
        cmd.args(extra_args);
        cmd.cwd(&self.cwd);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to start `{}` (is it installed and on PATH?)", self.program))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let id = self.id;
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Bg::PtyOutput(id, buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(Bg::PtyExit(id));
        });
        self.parser = vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK, Responder::default());
        self.proc = Some(Process { master: pair.master, writer, child });
        self.exited = false;
        self.started_at = Some(Instant::now());
        Ok(())
    }

    /// Feed PTY output; returns bytes destined for the host terminal (OSC 52).
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        let lines_before = self.selection.is_some().then(|| self.scrollback_len());
        let at_bottom = self.parser.screen().scrollback() == 0;
        let before = self.parser.screen().scrollback();
        self.parser.process(bytes);
        if !at_bottom {
            // Keep the viewport stable while the user reads scrollback.
            self.parser.screen_mut().set_scrollback(before);
        }
        // Output that scrolls lines (or drops them from a full scrollback) moves the selected text.
        if let Some(n) = lines_before {
            let after = self.scrollback_len();
            if after != n || after >= SCROLLBACK {
                self.selection = None;
            }
        }
        self.last_output = Some(Instant::now());
        let cb = self.parser.callbacks_mut();
        if std::mem::take(&mut cb.bell) {
            self.attention = Some(Attention::Bell);
        }
        let reply = std::mem::take(&mut cb.to_pty);
        let host = std::mem::take(&mut cb.to_host);
        if !reply.is_empty() {
            self.write(&reply);
        }
        host
    }

    /// Recompute working/idle; returns true while the tab label needs redrawing.
    pub fn update_status(&mut self, visible: bool) -> bool {
        // Output right after a keystroke is just echo, not the agent working.
        let working = self.running()
            && self.hook_working.unwrap_or_else(|| self.last_output.is_some_and(|out| {
                out.elapsed() < Duration::from_millis(1500)
                    && self.last_input.is_none_or(|inp| out.saturating_duration_since(inp) > Duration::from_millis(700))
            }));
        let changed = working != self.working;
        if self.working && !working && !visible && self.attention.is_none() {
            self.attention = Some(Attention::Finished);
        }
        self.working = working;
        changed || working
    }

    pub fn seen(&mut self) {
        self.attention = None;
    }

    pub fn status_glyph(&self) -> &'static str {
        if self.error.is_some() || self.exited {
            "✗"
        } else if !self.started() {
            "○"
        } else if self.attention == Some(Attention::Bell) {
            "!"
        } else if self.working {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
            SPINNER[(ms / 100) as usize % SPINNER.len()]
        } else if self.attention == Some(Attention::Finished) {
            "✓"
        } else {
            "●"
        }
    }

    pub fn on_exit(&mut self) {
        self.exited = true;
        if let Some(p) = &mut self.proc {
            let _ = p.child.try_wait();
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if let Some(p) = &mut self.proc {
            let _ = p.writer.write_all(bytes).and_then(|_| p.writer.flush());
        }
    }

    pub fn kill(&mut self) {
        if let Some(p) = &mut self.proc {
            let _ = p.child.kill();
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(p) = &self.proc {
            let _ = p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    }

    pub fn scroll(&mut self, delta: isize) {
        let screen = self.parser.screen_mut();
        let cur = screen.scrollback() as isize;
        screen.set_scrollback((cur + delta).max(0) as usize);
    }

    /// Lines above the screen. vt100 has no accessor, so clamp an oversized offset.
    pub fn scrollback_len(&mut self) -> usize {
        let screen = self.parser.screen_mut();
        let cur = screen.scrollback();
        screen.set_scrollback(usize::MAX);
        let len = screen.scrollback();
        screen.set_scrollback(cur);
        len
    }

    /// Absolute line index of visible row `row` (scrollback lines come first).
    pub fn line_at(&mut self, row: u16) -> usize {
        self.scrollback_len() - self.parser.screen().scrollback() + row as usize
    }

    /// Every line of output, scrollback then screen; the index is the absolute line.
    /// vt100 only exposes visible rows, so page the viewport through the scrollback.
    pub fn all_lines(&mut self) -> Vec<String> {
        let len = self.scrollback_len();
        let screen = self.parser.screen_mut();
        let saved = screen.scrollback();
        let (rows, cols) = screen.size();
        let step = (rows as usize).max(1);
        let mut lines = vec![String::new(); len + rows as usize];
        let mut offset = len;
        loop {
            screen.set_scrollback(offset);
            for (i, text) in screen.rows(0, cols).enumerate() {
                lines[len - offset + i] = text;
            }
            if offset == 0 {
                break;
            }
            offset = offset.saturating_sub(step);
        }
        screen.set_scrollback(saved);
        lines
    }

    /// Scroll so absolute `line` is visible, centring it when the view has to move.
    pub fn reveal_line(&mut self, line: usize) {
        let len = self.scrollback_len();
        let rows = self.parser.screen().size().0 as usize;
        let top = len - self.parser.screen().scrollback();
        if line >= top && line < top + rows {
            return;
        }
        let top = line.saturating_sub(rows / 2).min(len);
        self.parser.screen_mut().set_scrollback(len - top);
    }

    /// True when the app enabled mouse reporting, so clicks and drags belong to it.
    pub fn wants_mouse(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// Text of the selection (clipped to the visible screen), trailing spaces trimmed.
    pub fn selection_text(&mut self) -> Option<String> {
        let (a, b) = self.selection?;
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let top = self.line_at(0);
        let (rows, cols) = self.parser.screen().size();
        let bottom = top + rows as usize;
        if end.0 < top || start.0 >= bottom {
            return None;
        }
        let (sr, sc) = if start.0 < top { (0, 0) } else { ((start.0 - top) as u16, start.1) };
        let (er, ec) = if end.0 >= bottom { (rows - 1, cols) } else { ((end.0 - top) as u16, (end.1 + 1).min(cols)) };
        Some(trim_lines(&self.parser.screen().contents_between(sr, sc, er, ec)))
    }

    /// The whole visible screen as text.
    pub fn screen_text(&self) -> String {
        trim_lines(&self.parser.screen().contents()).trim_end_matches('\n').to_string()
    }

    pub fn send_key(&mut self, key: KeyEvent) {
        if self.exited || self.error.is_some() {
            return;
        }
        self.parser.screen_mut().set_scrollback(0);
        self.selection = None;
        self.last_input = Some(Instant::now());
        self.attention = None;
        let bytes = encode_key(key, self.parser.screen().application_cursor());
        self.write(&bytes);
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        self.parser.screen_mut().set_scrollback(0);
        self.selection = None;
        self.last_input = Some(Instant::now());
        if self.parser.screen().bracketed_paste() {
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    /// Forward the mouse to apps that asked for it. Returns false if unhandled.
    pub fn forward_mouse(&mut self, ev: MouseEvent, area: Rect) -> bool {
        let screen = self.parser.screen();
        let col = ev.column.saturating_sub(area.x) + 1;
        let row = ev.row.saturating_sub(area.y) + 1;
        if screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None {
            // Mimic "alternate scroll": wheel sends arrows in full-screen apps.
            if screen.alternate_screen() {
                let arrow: &[u8] = match (ev.kind, screen.application_cursor()) {
                    (MouseEventKind::ScrollUp, true) => b"\x1bOA",
                    (MouseEventKind::ScrollUp, false) => b"\x1b[A",
                    (MouseEventKind::ScrollDown, true) => b"\x1bOB",
                    (MouseEventKind::ScrollDown, false) => b"\x1b[B",
                    _ => return false,
                };
                let bytes = arrow.repeat(3);
                self.write(&bytes);
                return true;
            }
            return false;
        }
        if screen.mouse_protocol_encoding() != vt100::MouseProtocolEncoding::Sgr {
            return false;
        }
        let (code, release) = match ev.kind {
            MouseEventKind::Down(b) => (button_code(b), false),
            MouseEventKind::Up(b) => (button_code(b), true),
            MouseEventKind::Drag(b) => (button_code(b) + 32, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            _ => return false,
        };
        let seq = format!("\x1b[<{code};{col};{row}{}", if release { 'm' } else { 'M' });
        self.write(seq.as_bytes());
        true
    }

    /// `hints` carries (label offset, total refs across panes) while jump labels show.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, focused: bool, refs: &[FileRef], hover: Option<(u16, u16)>, hints: Option<(usize, usize)>) -> Option<(u16, u16)> {
        self.resize(area.height, area.width);
        if let Some(err) = &self.error {
            buf.set_stringn(area.x, area.y, err, area.width as usize, Style::default().fg(theme::ERROR()));
            buf.set_string(area.x, area.y + 1, "press Enter to retry", Style::default().fg(theme::DIM()));
            return None;
        }
        let top = if self.find_match.is_some() || self.selection.is_some() { self.line_at(0) } else { 0 };
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        for row in 0..rows.min(area.height) {
            for col in 0..cols.min(area.width) {
                let Some(cell) = screen.cell(row, col) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default().fg(convert(cell.fgcolor())).bg(convert(cell.bgcolor()));
                let mut m = Modifier::empty();
                if cell.bold() { m |= Modifier::BOLD; }
                if cell.dim() { m |= Modifier::DIM; }
                if cell.italic() { m |= Modifier::ITALIC; }
                if cell.underline() { m |= Modifier::UNDERLINED; }
                if cell.inverse() { m |= Modifier::REVERSED; }
                style = style.add_modifier(m);
                let c = &mut buf[(area.x + col, area.y + row)];
                c.set_style(style);
                c.set_symbol(if cell.has_contents() { cell.contents() } else { " " });
            }
        }

        let hovered = hover.and_then(|(r, c)| refs.iter().position(|f| f.contains(r, c)));
        for (i, r) in refs.iter().enumerate() {
            let mut style = Style::default().add_modifier(Modifier::UNDERLINED);
            if Some(i) == hovered {
                style = style.fg(theme::LINK()).add_modifier(Modifier::BOLD);
            }
            for &(row, s, e) in &r.segments {
                for col in s..e.min(area.width) {
                    buf[(area.x + col, area.y + row)].set_style(style);
                }
            }
            if let Some((base, total)) = hints {
                if let Some(&(row, s, _)) = r.segments.first() {
                    let label = hint_label(base + i, total);
                    buf.set_string(area.x + s, area.y + row, label, Style::default().fg(theme::HINT_FG()).bg(theme::HINT_BG()).add_modifier(Modifier::BOLD));
                }
            }
        }

        if let Some((a, b)) = self.selection {
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            let style = Style::default().bg(theme::SELECT()).remove_modifier(Modifier::REVERSED);
            for row in 0..rows.min(area.height) {
                let line = top + row as usize;
                if line < start.0 || line > end.0 {
                    continue;
                }
                let from = if line == start.0 { start.1 } else { 0 };
                let to = if line == end.0 { end.1 + 1 } else { cols };
                for col in from..to.min(cols).min(area.width) {
                    buf[(area.x + col, area.y + row)].set_style(style);
                }
            }
        }
        if let Some((line, from, to)) = self.find_match {
            if line >= top && line < top + rows.min(area.height) as usize {
                let row = (line - top) as u16;
                let style = Style::default().fg(theme::HINT_FG()).bg(theme::ACCENT()).remove_modifier(Modifier::REVERSED);
                for col in from..to.min(area.width) {
                    buf[(area.x + col, area.y + row)].set_style(style);
                }
            }
        }

        if screen.scrollback() > 0 {
            let tag = format!(" ↑ scrollback {} ", screen.scrollback());
            let x = area.x + area.width.saturating_sub(tag.chars().count() as u16);
            buf.set_string(x, area.y, tag, Style::default().fg(theme::HINT_FG()).bg(theme::ACCENT()));
        } else if self.exited {
            let y = area.y + area.height.saturating_sub(1);
            buf.set_stringn(area.x, y, " process exited — press Enter to restart ", area.width as usize, Style::default().fg(theme::HINT_FG()).bg(theme::ACCENT()));
        }

        if focused && !screen.hide_cursor() && screen.scrollback() == 0 && !self.exited {
            let (r, c) = screen.cursor_position();
            if r < area.height && c < area.width {
                return Some((area.x + c, area.y + r));
            }
        }
        None
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.kill();
    }
}

pub const HINT_KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";

/// Hint labels count from the bottom so the newest reference gets `a`.
pub fn hint_label(i: usize, total: usize) -> String {
    let from_bottom = total - 1 - i;
    let keys: Vec<char> = HINT_KEYS.chars().collect();
    if total <= keys.len() {
        keys[from_bottom].to_string()
    } else {
        format!("{}{}", keys[from_bottom / keys.len() % keys.len()], keys[from_bottom % keys.len()])
    }
}

/// Case-insensitive unless the query has uppercase. Returns (line, start col, end col)
/// in terminal cells, so wide characters count twice.
pub fn find_matches(lines: &[String], query: &str) -> Vec<(usize, u16, u16)> {
    let case = query.chars().any(char::is_uppercase);
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    for (i, line) in lines.iter().enumerate() {
        let hay: Vec<char> = line.chars().map(fold).collect();
        if hay.len() < needle.len() {
            continue;
        }
        let mut cols = Vec::with_capacity(hay.len() + 1);
        let mut col = 0u16;
        for c in line.chars() {
            cols.push(col);
            col = col.saturating_add(unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16);
        }
        cols.push(col);
        let mut s = 0;
        while s + needle.len() <= hay.len() {
            if hay[s..s + needle.len()] == needle[..] {
                let end = s + needle.len();
                out.push((i, cols[s], cols[end].max(cols[s] + 1)));
                s = end;
            } else {
                s += 1;
            }
        }
    }
    out
}

fn trim_lines(text: &str) -> String {
    text.split('\n').map(str::trim_end).collect::<Vec<_>>().join("\n")
}

fn button_code(b: MouseButton) -> u16 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn convert(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let modcode = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let mut out = Vec::new();

    let csi_mod = |final_: char, out: &mut Vec<u8>| {
        if modcode > 1 {
            out.extend_from_slice(format!("\x1b[1;{modcode}{final_}").as_bytes());
        } else if app_cursor {
            out.extend_from_slice(format!("\x1bO{final_}").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{final_}").as_bytes());
        }
    };
    let tilde = |n: u8, out: &mut Vec<u8>| {
        if modcode > 1 {
            out.extend_from_slice(format!("\x1b[{n};{modcode}~").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{n}~").as_bytes());
        }
    };

    match key.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 - b'a' + 1,
                    '@' | ' ' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '/' | '7' => 0x1f,
                    '8' | '?' => 0x7f,
                    other => {
                        out.extend_from_slice(other.encode_utf8(&mut [0; 4]).as_bytes());
                        return out;
                    }
                };
                out.push(b);
            } else {
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt || shift {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => csi_mod('A', &mut out),
        KeyCode::Down => csi_mod('B', &mut out),
        KeyCode::Right => csi_mod('C', &mut out),
        KeyCode::Left => csi_mod('D', &mut out),
        KeyCode::Home => csi_mod('H', &mut out),
        KeyCode::End => csi_mod('F', &mut out),
        KeyCode::Insert => tilde(2, &mut out),
        KeyCode::Delete => tilde(3, &mut out),
        KeyCode::PageUp => tilde(5, &mut out),
        KeyCode::PageDown => tilde(6, &mut out),
        KeyCode::F(n @ 1..=4) => {
            let c = [b'P', b'Q', b'R', b'S'][n as usize - 1];
            if modcode > 1 {
                out.extend_from_slice(format!("\x1b[1;{modcode}{}", c as char).as_bytes());
            } else {
                out.extend_from_slice(&[0x1b, b'O', c]);
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5], &mut out),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = |code, m| encode_key(KeyEvent::new(code, m), false);
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), vec![3]);
        assert_eq!(k(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(k(KeyCode::Left, KeyModifiers::CONTROL), b"\x1b[1;5D");
        assert_eq!(encode_key(KeyEvent::from(KeyCode::Up), true), b"\x1bOA");
        assert_eq!(k(KeyCode::Char('é'), KeyModifiers::NONE), "é".as_bytes());
    }

    #[test]
    fn answers_cursor_query() {
        let mut p = vt100::Parser::new_with_callbacks(10, 20, 0, Responder::default());
        p.process(b"ab\x1b[6n\x1b]11;?\x07");
        let reply = String::from_utf8(p.callbacks().to_pty.clone()).unwrap();
        assert!(reply.starts_with("\x1b[1;3R\x1b]11;rgb:1e1e/1e1e/2e2e"), "{reply:?}");
    }

    #[test]
    fn all_lines_include_scrollback() {
        let mut agent = Agent::new(0, "t", "sh", PathBuf::from("."));
        agent.resize(5, 20);
        let input: String = (1..=30).map(|i| format!("line {i}\r\n")).collect();
        agent.process(input.as_bytes());
        agent.parser.screen_mut().set_scrollback(7);
        let lines = agent.all_lines();
        assert_eq!(agent.parser.screen().scrollback(), 7, "offset restored");
        assert_eq!(agent.scrollback_len(), 26);
        assert_eq!(lines.len(), 31);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[12], "line 13");
        assert_eq!(lines[29], "line 30");
        assert_eq!(lines[30], "");

        assert!(find_matches(&lines, "LINE 3").is_empty(), "uppercase query is case-sensitive");
        let m = find_matches(&lines, "line 3");
        assert_eq!(m.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2, 29]);
        assert_eq!(m[0], (2, 0, 6));

        agent.reveal_line(3);
        assert_eq!(agent.line_at(0), 1);
        agent.reveal_line(29);
        assert_eq!(agent.line_at(0), 26);
    }

    #[test]
    fn selection_copies_text() {
        let mut agent = Agent::new(0, "t", "sh", PathBuf::from("."));
        agent.resize(4, 20);
        agent.process(b"alpha beta\r\ngamma   \r\ndelta");
        let top = agent.line_at(0);
        agent.selection = Some(((top + 2, 2), (top, 6)));
        assert_eq!(agent.selection_text().unwrap(), "beta\ngamma\ndel");
        agent.selection = Some(((top, 0), (top, 4)));
        assert_eq!(agent.selection_text().unwrap(), "alpha");
        assert_eq!(agent.screen_text(), "alpha beta\ngamma\ndelta");
        assert_eq!(find_matches(&["日本 ok".to_string()], "ok"), vec![(0, 5, 7)]);
    }

    #[test]
    fn labels() {
        assert_eq!(hint_label(2, 3), "a");
        assert_eq!(hint_label(0, 3), "d");
    }
}
