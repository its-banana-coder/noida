mod actions;
mod activity;
mod agent;
mod app;
mod editor;
mod events;
mod git;
mod hooks;
mod lsp;
mod markdown;
mod picker;
mod refs;
mod sessions;
mod settings;
mod symbols;
mod theme;
mod tree;
mod workspace;

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::execute;

const HELP: &str = "\
NOIDA — Navigation-Oriented IDE for Developer Agents

USAGE:
    noida [PATH[:LINE]] [--agent NAME=COMMAND]... [--fresh]

    PATH      project directory (default: current dir), or a file to open
    --agent   agent tab to run, e.g. --agent claude=claude --agent aider=\"aider --no-git\"
              (default: restore last session's tabs, else claude + codex + shell)
    --fresh   don't restore open files, layout and agent sessions

KEYS (Alt+x opens the command palette with everything):
    Alt+1/2/3   focus files / editor / agent     Alt+0   toggle file tree
    Alt+j       jump to a file ref shown by the agent (label, Enter = newest)
    click       a highlighted path or symbol in agent output opens it
    Alt+o       open file (fuzzy), also Ctrl+P outside the agent pane
    Alt+s / S   send selection / file to the agent   Alt+e  ask agent to explain
    Alt+g       sessions: switch tabs or resume past Claude/Codex conversations
    Alt+n       next agent tab       Alt+v split agents     Alt+w other pane
    Alt+r       review changes (a accept hunk, x reject)    Alt+a agent activity
    Alt+l / k   symbols in file / project    F12 definition  Shift+F12 references
    Alt+i       problems             Alt+F  ask agent to fix the problem at cursor
    Alt+, Alt+. resize agent pane    Alt+z zoom    Alt+- back    Alt+q quit
";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        // Hook clients spawned by agents: forward the event and exit silently.
        Some("hook") => {
            let mut payload = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload);
            hooks::forward("claude", &payload);
            if let Some(ctx) = hooks::prompt_context(&payload) {
                println!("{ctx}");
            }
            return Ok(());
        }
        Some("hook-codex") => {
            let payload = std::env::args().nth(2).unwrap_or_default();
            hooks::forward("codex", &payload);
            return Ok(());
        }
        _ => {}
    }

    let mut target: Option<String> = None;
    let mut agents: Vec<(String, String)> = Vec::new();
    let mut restore = true;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("noida {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--fresh" => restore = false,
            "--agent" => {
                let spec = args.next().context("--agent needs NAME=COMMAND")?;
                let (name, cmd) = spec.split_once('=').unwrap_or((&spec, &spec));
                agents.push((name.to_string(), cmd.to_string()));
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n\n{HELP}"),
            a => target = Some(a.to_string()),
        }
    }

    let (path, line) = match &target {
        Some(t) => {
            let (p, l) = refs::split_line_suffix(t);
            (PathBuf::from(p), l)
        }
        None => (PathBuf::from("."), None),
    };
    let path = path.canonicalize().with_context(|| format!("no such path: {}", path.display()))?;
    let (root, open) = if path.is_file() {
        (path.parent().unwrap().to_path_buf(), Some(path))
    } else {
        (path, None)
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("noida"));
    let opts = app::Options { root, agents: (!agents.is_empty()).then_some(agents), restore, exe };

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = run(&mut terminal, opts, open, line);
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, opts: app::Options, open: Option<PathBuf>, line: Option<usize>) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut app = app::App::new(opts, tx);
    if let Some(p) = open {
        app.open_path(&p, line);
    }
    // Draw once so the agent PTY starts at the real pane size.
    terminal.draw(|f| app.draw(f))?;
    app.ensure_agent_started();

    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|f| app.draw(f))?;
            if !app.host_out.is_empty() {
                let mut out = std::io::stdout();
                out.write_all(&app.host_out)?;
                out.flush()?;
                app.host_out.clear();
            }
            dirty = false;
        }

        if event::poll(Duration::from_millis(16))? {
            loop {
                app.on_event(event::read()?);
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            dirty = true;
        }
        while let Ok(ev) = rx.try_recv() {
            app.on_bg(ev);
            dirty = true;
        }
        dirty |= app.tick();
        if app.quit {
            app.save_workspace();
            return Ok(());
        }
    }
}
