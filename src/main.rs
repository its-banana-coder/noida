mod agent;
mod app;
mod editor;
mod refs;
mod theme;
mod tree;

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
    noida [PATH[:LINE]] [--agent NAME=COMMAND]...

    PATH      project directory (default: current dir), or a file to open
    --agent   agent tab to run, e.g. --agent claude=claude --agent aider=\"aider --no-git\"
              (default: claude and codex if installed, plus a shell)

KEYS:
    Alt+1/2/3   focus files / editor / agent     Alt+0   toggle file tree
    Alt+j       jump to a file ref shown by the agent (label, Enter = newest)
    click       a highlighted path in the agent pane opens it at that line
    Alt+o       open file (fuzzy), also Ctrl+P outside the agent pane
    Alt+s       send selection as @path#Lx-y to the agent
    Alt+S       send current file as @path to the agent
    Alt+n       next agent tab                   Alt+z   zoom pane
    Alt+, Alt+. resize agent pane (or drag divider)
    Alt+-       go back to previous location     Alt+q   quit
    Editor:     Ctrl+S save, Ctrl+F find (F3 next), Ctrl+G go to line,
                Ctrl+Z/Y undo/redo, Ctrl+C/X copy/cut, Ctrl+W close,
                Ctrl+PgUp/PgDn switch file
";

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn main() -> Result<()> {
    let mut target: Option<String> = None;
    let mut agents: Vec<(String, String)> = Vec::new();
    let mut args = std::env::args().skip(1);
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
            "--agent" => {
                let spec = args.next().context("--agent needs NAME=COMMAND")?;
                let (name, cmd) = spec.split_once('=').unwrap_or((&spec, &spec));
                agents.push((name.to_string(), cmd.to_string()));
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n\n{HELP}"),
            a => target = Some(a.to_string()),
        }
    }
    if agents.is_empty() {
        for name in ["claude", "codex"] {
            if on_path(name) {
                agents.push((name.to_string(), name.to_string()));
            }
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
        agents.push(("shell".into(), shell));
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

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = run(&mut terminal, root, agents, open, line);
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    root: PathBuf,
    agents: Vec<(String, String)>,
    open: Option<PathBuf>,
    line: Option<usize>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut app = app::App::new(root, agents, tx);
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
            app.on_pty(ev);
            dirty = true;
        }
        dirty |= app.tick();
        if app.quit {
            return Ok(());
        }
    }
}
