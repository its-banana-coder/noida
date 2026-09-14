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

**Agents**
- **Real CLIs in real PTYs**: `claude`, `codex` and shells in tabs, with split panes (`Alt+v`)
- **Sessions** (`Alt+g`): switch tabs, or resume any past Claude/Codex conversation for the project. Tabs and conversations are restored on the next launch.
- **Exact status via hooks**: NOIDA attaches Claude Code hooks to the agents it starts (per session, your settings are untouched) and a Codex `notify` hook. Tabs show `⠙` working, `✓` finished, `!` needs permission.
- **Permission alerts**: when an agent waits for approval, the status bar says what it wants to run
- **Activity & timeline** (`Alt+a`): every prompt, read, edit, created file and command per turn. Past turns are kept in **Agent: History**.
- **Handoff**: *Ask codex to review claude's changes* (and vice versa) from the command palette
- **Worktrees**: start an agent in an isolated git worktree, review its diff, apply its changes to the project, or remove it

**Agent ↔ code navigation**
- **Clickable file references**: `src/app.rs:42`, `app.rs#L10-20`, `src/app.rs (lines 10-20)`, `index.ts around line 120`, `Read(src/app.rs)`, `a/src/app.rs`. Shortened paths are matched against project files, and if several match you get a picker.
- **Clickable symbols**: `resolveAnimatedLayout` or `refresh_session` in agent output jumps to the definition
- **Jump by keyboard**: `Alt+j` labels every reference on screen
- **Send context**: `Alt+s` sends `@path#L10-20`, `Alt+S` the whole file, `Alt+e` asks the agent to explain the selection, and the palette has refactor / find bugs / write tests / optimize / review
- **Fix with agent**: `Alt+F` sends the language-server problem at the cursor to the agent

**Git**
- Branch and change count in the status bar, `M`/`U`/`D`/`✓` markers in the tree, change bars in the editor gutter
- **Review changes** (`Alt+r`): per-hunk **accept** (stage) or **reject** (revert on disk), per file or everything. After an agent finishes, `Alt+r` opens just the files it changed.
- Switch branch, new branch, commit staged, all from the command palette

**Code intelligence**
- **Tree-sitter** symbols for Rust, TypeScript/TSX, JavaScript, Python, Go and Java: file outline (`Alt+l`), project symbols (`Alt+k`), structural selection (`Alt+Shift+→` / `←`)
- **Language servers** when installed (rust-analyzer, typescript-language-server, pyright/pylsp, gopls): diagnostics with underlines and a problems list (`Alt+i`), hover (`Alt+h`), signature help, go to definition (`F12`, `Ctrl+click`), find references (`Shift+F12`), rename across files (`F2`), quick fixes and code actions (`Alt+.`), format document (`Alt+F`), format selection, organize imports. Without a server, definitions come from the tree-sitter index and references from a project-wide search.

**Editor**
- **Multiple cursors**: `Ctrl+D` next occurrence, `Alt+click`, `Ctrl+Alt+↑/↓`, `Alt+drag` column selection, select all occurrences
- **Find / replace** (`Ctrl+F` / `Ctrl+H`): match case, whole word, regex with `$1` captures, in selection, match count, replace one/all
- **Workspace search / replace** (`Alt+/`): include globs, results grouped by file, a live preview of each replacement, and matches can be excluded before replacing
- Auto-closing brackets, quotes and JSX/HTML tags, bracket matching and pair colors, toggle comment (`Ctrl+/`), move line (`Alt+↑/↓`), copy line (`Alt+Shift+↑/↓`), delete line (`Ctrl+K`)
- Folding (click `▾` in the gutter), word wrap, sticky scroll, breadcrumbs
- Tabs: preview tabs (single click in the tree), pin, close others/all/to the right, reopen closed (`Alt+T`), recent files (`Alt+E`)
- Open files reload automatically when an agent edits them
- **Command palette** (`Alt+x`) and fuzzy file open (`Alt+o` / `Ctrl+P`)
- NOIDA Dark and Light themes, settings in `~/.config/noida/settings.json` (palette: *Preferences: Open Settings*)

**File tree**
- New file/folder, rename/move, delete, copy relative/absolute path, reveal in the OS file manager
- **Outline** panel (`Tab` in the tree) that follows the cursor

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

NOIDA's global shortcuts use **Alt**, so ordinary keys still reach the agent. They avoid Claude Code's own Meta bindings (`Alt+p`, `Alt+t`, `Alt+b`, `Alt+f`, `Alt+m`). **`Alt+x` opens the command palette, which lists every command with its shortcut.**

| Key | Action |
|---|---|
| `Alt+x` | Command palette |
| `Alt+1` / `Alt+2` / `Alt+3` | Focus files / editor / agent (`Alt+0` toggles the tree) |
| `Alt+o`, `Ctrl+P` | Open file (type `name:120` to jump to a line) |
| `Alt+j` | Label file/symbol references in agent output; type a label (`Enter` = newest) |
| `Alt+g` | Sessions: switch tab, new tab, resume a past conversation |
| `Alt+n` / `Alt+v` / `Alt+w` | Next agent tab / split agent panes / other pane |
| `Alt+s` / `Alt+S` | Send selection / current file to the agent |
| `Alt+e` | Ask agent… (explain, refactor, find bugs, write tests, fix problem, send symbol) |
| `Alt+/` | Search and replace in workspace |
| `Alt+E` / `Alt+T` | Recent files / reopen closed tab |
| `Alt+r` | Review changes (`a` accept hunk, `x` reject, `A`/`X` whole file, `Enter` open) |
| `Alt+a` | Agent activity and timeline |
| `Alt+l` / `Alt+k` | Symbols in file / project |
| `Alt+i` | Problems |
| `Alt+-` | Go back |
| `Alt+z` / `Alt+<` / `Alt+>` | Zoom pane / grow / shrink agent pane (or drag dividers) |
| `Alt+q` | Quit (asks again if files are unsaved) |

**Editor**

| Key | Action |
|---|---|
| `Ctrl+S` | Save |
| `Ctrl+F` / `Ctrl+H`, `F3` / `Shift+F3` | Find / replace, next / previous (in the widget: `Alt+c` case, `Alt+w` word, `Alt+r` regex, `Alt+l` in selection, `Alt+a` replace all) |
| `Ctrl+D`, `Alt+click`, `Ctrl+Alt+↑/↓` | Add cursor (next occurrence / at click / above, below); `Esc` back to one |
| `Ctrl+/` | Toggle comment |
| `Alt+↑/↓`, `Alt+Shift+↑/↓`, `Ctrl+K`, `Ctrl+L` | Move line, copy line, delete line, select line |
| `Alt+h`, `F2`, `Alt+.`, `Alt+F` | Hover, rename symbol, code actions, format document |
| `Ctrl+G` | Go to `line[:col]` |
| `F12`, `Ctrl+click` / `Shift+F12` | Go to definition / find references |
| `Alt+Shift+→` / `Alt+Shift+←` | Expand / shrink selection by syntax |
| `Alt+←` / `Alt+→` | Back / forward |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+C` / `Ctrl+X` | Copy / cut (line if nothing selected) to system clipboard via OSC 52 |
| `Ctrl+A` | Select all |
| `Shift+arrows`, mouse drag | Select |
| `Tab` / `Shift+Tab` | Indent / dedent |
| `Ctrl+W`, middle-click tab | Close file |
| `Ctrl+PgUp` / `Ctrl+PgDn` | Previous / next open file |

**File tree**: `↑/↓` or `j/k` move, `Enter` opens, `Space` previews, `←/→` collapse/expand, `a` new file, `A` new folder, `r`/`F2` rename, `d` delete, `y`/`Y` copy path, `o` reveal in file manager, `Tab` outline, `R` refresh.

**Agent pane**: every key goes to the agent. Mouse wheel scrolls back through output.

## How agent integration works

- NOIDA starts `claude` with `--settings '<hooks>'` and `--session-id <uuid>`. The hooks run `noida hook`, which forwards the event over a local Unix socket and prints nothing, so it never affects Claude's decisions or permission prompts. Your own settings and hooks still apply.
- `codex` is started with `-c notify=[noida, hook-codex]` to report finished turns.
- Workspace state lives in `~/.local/share/noida/workspaces/`, agent history in `~/.local/share/noida/history/`. Start with `--fresh` to skip restoring.

## Known limitations

- `Shift+Enter` can't be told apart from `Enter` in most terminals. Use the agent's own newline shortcut (in Claude Code, `\` then `Enter`).
- The Alt shortcuts above aren't passed through to the agent.
- Language features need the language server installed (e.g. `rustup component add rust-analyzer`, `npm i -g typescript-language-server typescript`).

## Development

```bash
cargo run -- .    # run from source
cargo test        # unit tests
```

Code layout:

| File | Purpose |
|---|---|
| `src/main.rs` | CLI parsing, terminal setup, event loop |
| `src/app/` | App state, input routing, commands (`mod.rs`), layout (`draw.rs`), review/activity views (`views.rs`) |
| `src/actions.rs`, `src/picker.rs` | Command registry, fuzzy picker |
| `src/hooks.rs`, `src/activity.rs`, `src/sessions.rs` | Agent hooks, activity/history, past session discovery |
| `src/git.rs` | Status, diffs, hunk stage/revert, branches, worktrees |
| `src/symbols.rs`, `src/lsp.rs` | Tree-sitter symbols, LSP client |
| `src/workspace.rs`, `src/events.rs` | Session restore, background event bus |
| `src/agent.rs` | PTY processes, vt100 emulation, key encoding, terminal query replies |
| `src/refs.rs` | File reference detection in agent output |
| `src/editor/` | Multi-cursor buffer, search, folding (`mod.rs`), layout and rendering (`view.rs`), language conventions (`lang.rs`) |
| `src/app/search.rs`, `findbar.rs`, `files.rs`, `lsp_ui.rs` | Workspace search, find widget, tabs/tree ops/outline, LSP editor features |
| `src/settings.rs`, `src/theme.rs` | Settings file, dark/light palettes |
| `src/tree.rs` | File tree |

## License

[MIT](LICENSE)
