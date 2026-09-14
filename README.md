# NOIDA

**Navigation-Oriented IDE for Developer Agents** — a terminal IDE where your coding agent runs on the right, your code sits on the left, and every file reference the agent prints is clickable.

```text
╭ FILES ──────────╮╭ session.ts ─────────────────────╮╭ ● claude  ○ codex  ○ shell ─────────╮
│▾ src            ││ 85                              ││ > Where is the auth logic?          │
│  ▾ auth         ││ 86                              ││                                     │
│    session.ts   ││ 87 function refreshSession() {  ││ The refresh happens in              │
│    token.ts     ││ 88   ...                        ││ src/auth/session.ts:87  ← click it  │
│  ▸ api          ││                                 ││                                     │
╰─────────────────╯╰─────────────────────────────────╯╰─────────────────────────────────────╯
 NOIDA  src/auth/session.ts:87
```

NOIDA doesn't reimplement Claude Code or Codex. It runs the real CLIs in a pseudo-terminal, so your login, config, `CLAUDE.md`, MCP servers and slash commands all work as usual. On top of that it gives you a file tree, an editor, and navigation between what the agent says and the code.

## Features

- **Agent panes**: `claude`, `codex` and a shell in tabs, each in a real PTY. Tab status shows `⠙` working, `●` idle, `✓` finished in the background, `!` wants attention (bell)
- **Clickable file references**: `src/app.rs:42`, `src/app.rs:42:7`, `app.rs#L10-20`, `src/app.rs (lines 10-20)`, `Read(src/app.rs)` `index.ts around line 120` and git-diff style `a/src/app.rs` paths are detected. Shortened paths like `index.ts:146` or `engine/src/index.ts` are matched against project files, and if several files match you get a picker. Only paths that exist get highlighted.
- **Jump by keyboard**: `Alt+j` labels every reference on screen. Press a label to open it, or `Enter` for the newest one.
- **File tree** that respects `.gitignore`, shows files agents create, and reveals whatever you open
- **Editor** with syntax highlighting (the same language set as `bat`), multiple open files, undo/redo, find, go to line and mouse selection
- **Live reload**: open files update when the agent edits them, and NOIDA warns you if you have unsaved changes
- **Send code to the agent**: select lines and press `Alt+s` to insert `@path#L10-20` into the agent prompt, or `Alt+Shift+S` for `@path`
- **Fuzzy file open** (`Alt+o` / `Ctrl+P`), jump history (`Alt+-`), resizable and zoomable panes

## Installation

### 1. Install Rust

NOIDA needs Rust **1.88 or newer** and a C linker.

```bash
# Linux / WSL / macOS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

On Debian/Ubuntu (including WSL), install a linker if you don't have one:

```bash
sudo apt install build-essential
```

On macOS, run `xcode-select --install`.

### 2. Install NOIDA

From GitHub:

```bash
cargo install --git https://github.com/its-banana-coder/noida
```

Or from a local clone:

```bash
git clone https://github.com/its-banana-coder/noida
cd noida
cargo install --path .
```

Either way, `noida` ends up in `~/.cargo/bin`, which rustup adds to your `PATH`.

### 3. Install an agent (optional but recommended)

NOIDA adds a tab for each of these it finds on your `PATH`:

- [Claude Code](https://docs.claude.com/en/docs/claude-code): `claude`
- [Codex CLI](https://github.com/openai/codex): `codex`

A shell tab is always available.

### Terminal requirements

Use a terminal with **truecolor and mouse support**, such as Windows Terminal, iTerm2, WezTerm, kitty, Alacritty or GNOME Terminal. Inside tmux, enable mouse mode (`set -g mouse on`).

Tested on Linux (WSL2). macOS should work but is untested. Native Windows (outside WSL) isn't supported yet.

## Usage

```bash
noida                    # open the current directory
noida ~/code/my-project  # open a project
noida src/main.rs:120    # open a file at a line (its folder becomes the project)
```

### Custom agents

Pass `--agent NAME=COMMAND` (repeatable). This replaces the default tabs:

```bash
noida --agent claude=claude --agent aider="aider --no-git" --agent shell=bash
```

## Keybindings

NOIDA's global shortcuts use **Alt** so that ordinary keys still reach the agent.

| Key | Action |
|---|---|
| `Alt+1` / `Alt+2` / `Alt+3` | Focus files / editor / agent |
| `Alt+0` | Toggle file tree |
| `Alt+j` | Label file references in agent output; type a label to open (`Enter` = newest) |
| Click a highlighted path | Open it at that line |
| `Alt+o`, `Ctrl+P` | Fuzzy open file (type `name:120` to jump to a line) |
| `Alt+s` | Send editor selection (or current line) to the agent as `@path#Lx-y` |
| `Alt+Shift+S` | Send the current file to the agent as `@path` |
| `Alt+-` | Go back to previous location |
| `Alt+n` | Next agent tab (or click a tab) |
| `Alt+z` | Zoom focused pane |
| `Alt+,` / `Alt+.` | Grow / shrink agent pane (or drag the divider) |
| `Alt+q` | Quit (asks again if files are unsaved) |

**Editor**

| Key | Action |
|---|---|
| `Ctrl+S` | Save |
| `Ctrl+F`, `F3` | Find, find next |
| `Ctrl+G` | Go to `line[:col]` |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+C` / `Ctrl+X` | Copy / cut (line if nothing selected) to system clipboard via OSC 52 |
| `Ctrl+A` | Select all |
| `Shift+arrows`, mouse drag | Select |
| `Ctrl+←/→` | Move by word |
| `Tab` / `Shift+Tab` | Indent / dedent |
| `Ctrl+W` | Close file |
| `Ctrl+PgUp` / `Ctrl+PgDn` | Previous / next open file |

To paste, use your terminal's paste shortcut (e.g. `Ctrl+Shift+V`).

**File tree**: `↑/↓` or `j/k` move, `Enter` opens, `←/→` collapse/expand, `r` refreshes.

**Agent pane**: every key goes to the agent. Mouse wheel scrolls back through output.

## Known limitations

- `Shift+Enter` can't be told apart from `Enter` in most terminals. Use the agent's own newline shortcut (in Claude Code, `\` then `Enter`).
- The Alt shortcuts above aren't passed through to the agent.
- There's no git diff viewer, LSP or plugin support yet.

## Development

```bash
cargo run -- .    # run from source
cargo test        # unit tests
```

Code layout:

| File | Purpose |
|---|---|
| `src/main.rs` | CLI parsing, terminal setup, event loop |
| `src/app.rs` | Layout, focus, input routing, quick open, navigation |
| `src/agent.rs` | PTY processes, vt100 emulation, key encoding, terminal query replies |
| `src/refs.rs` | File reference detection in agent output |
| `src/editor.rs` | Text buffer, editing, syntax highlighting |
| `src/tree.rs` | File tree |

## License

[MIT](LICENSE)
