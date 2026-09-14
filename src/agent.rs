//! Agent panes: real PTYs running `claude`, `codex`, or a shell.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::refs::FileRef;
use crate::theme;

pub enum PtyEvent {
    Output(usize, Vec<u8>),
    Exited(usize),
}

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
            [b"10", b"?"] => Some(("10", theme::FG)),
            [b"11", b"?"] => Some(("11", theme::BG)),
            [b"12", b"?"] => Some(("12", theme::FG)),
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
}

#[derive(Clone, Copy, PartialEq)]
enum Attention {
    Bell,
    Finished,
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Agent {
    pub fn new(id: usize, name: &str, command: &str) -> Self {
        let mut parts = command.split_whitespace().map(String::from);
        Self {
            id,
            name: name.to_string(),
            program: parts.next().unwrap_or_default(),
            args: parts.collect(),
            parser: vt100::Parser::new_with_callbacks(24, 80, 10_000, Responder::default()),
            proc: None,
            exited: false,
            error: None,
            size: (24, 80),
            last_output: None,
            last_input: None,
            working: false,
            attention: None,
        }
    }

    pub fn running(&self) -> bool {
        self.proc.is_some() && !self.exited
    }

    pub fn started(&self) -> bool {
        self.proc.is_some() || self.error.is_some()
    }

    pub fn start(&mut self, cwd: &Path, tx: Sender<PtyEvent>) {
        self.error = None;
        if let Err(e) = self.try_start(cwd, tx) {
            self.error = Some(format!("{e:#}"));
        }
    }

    fn try_start(&mut self, cwd: &Path, tx: Sender<PtyEvent>) -> Result<()> {
        let (rows, cols) = self.size;
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("failed to open pty")?;
        let mut cmd = CommandBuilder::new(&self.program);
        cmd.args(&self.args);
        cmd.cwd(cwd);
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
                        if tx.send(PtyEvent::Output(id, buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(PtyEvent::Exited(id));
        });
        self.parser = vt100::Parser::new_with_callbacks(rows, cols, 10_000, Responder::default());
        self.proc = Some(Process { master: pair.master, writer, child });
        self.exited = false;
        Ok(())
    }

    /// Feed PTY output; returns bytes destined for the host terminal (OSC 52).
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        let at_bottom = self.parser.screen().scrollback() == 0;
        let before = self.parser.screen().scrollback();
        self.parser.process(bytes);
        if !at_bottom {
            // Keep the viewport stable while the user reads scrollback.
            self.parser.screen_mut().set_scrollback(before);
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
            && self.last_output.is_some_and(|out| {
                out.elapsed() < Duration::from_millis(1500)
                    && self.last_input.is_none_or(|inp| out.saturating_duration_since(inp) > Duration::from_millis(700))
            });
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

    pub fn send_key(&mut self, key: KeyEvent) {
        if self.exited || self.error.is_some() {
            if key.code == KeyCode::Enter {
                self.proc = None;
                self.error = None;
            }
            return;
        }
        self.parser.screen_mut().set_scrollback(0);
        self.last_input = Some(Instant::now());
        self.attention = None;
        let bytes = encode_key(key, self.parser.screen().application_cursor());
        self.write(&bytes);
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        self.parser.screen_mut().set_scrollback(0);
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

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, focused: bool, refs: &[FileRef], hover: Option<(u16, u16)>, hints: bool) -> Option<(u16, u16)> {
        self.resize(area.height, area.width);
        if let Some(err) = &self.error {
            buf.set_stringn(area.x, area.y, err, area.width as usize, Style::default().fg(theme::ERROR));
            buf.set_string(area.x, area.y + 1, "press Enter to retry", Style::default().fg(theme::DIM));
            return None;
        }
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
                style = style.fg(theme::LINK).add_modifier(Modifier::BOLD);
            }
            for &(row, s, e) in &r.segments {
                for col in s..e.min(area.width) {
                    buf[(area.x + col, area.y + row)].set_style(style);
                }
            }
            if hints {
                if let Some(&(row, s, _)) = r.segments.first() {
                    let label = hint_label(i, refs.len());
                    buf.set_string(area.x + s, area.y + row, label, Style::default().fg(theme::HINT_FG).bg(theme::HINT_BG).add_modifier(Modifier::BOLD));
                }
            }
        }

        if screen.scrollback() > 0 {
            let tag = format!(" ↑ scrollback {} ", screen.scrollback());
            let x = area.x + area.width.saturating_sub(tag.chars().count() as u16);
            buf.set_string(x, area.y, tag, Style::default().fg(theme::HINT_FG).bg(theme::ACCENT));
        } else if self.exited {
            let y = area.y + area.height.saturating_sub(1);
            buf.set_stringn(area.x, y, " process exited — press Enter to restart ", area.width as usize, Style::default().fg(theme::HINT_FG).bg(theme::ACCENT));
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
    fn labels() {
        assert_eq!(hint_label(2, 3), "a");
        assert_eq!(hint_label(0, 3), "d");
    }
}
