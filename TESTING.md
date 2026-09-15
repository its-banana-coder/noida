# Smoke-test checklist

Run this before tagging a release, on at least one Linux terminal and, when possible, one macOS terminal. It takes about 15 minutes. Unit tests (`cargo test`) cover parsing and editing logic; this list covers what only a real terminal shows.

**Setup:** a scratch git repo with a few source files and one commit, `claude` and `codex` on `PATH`, and a language server for the repo's language (e.g. `rustup component add rust-analyzer rustfmt`). Start with `noida <repo> --fresh`.

Note the terminal and OS at the top of your report, e.g. *"Ghostty 1.2 on macOS 15"*.

## 1. Launch and agents
- [ ] One agent pane opens (Claude if installed) and shows its normal UI at the right size
- [ ] `+` on the agent tab bar and `Alt+N` start new sessions; `✕` closes one (asks again while running)
- [ ] After your first prompt the tab is renamed from it; **Agent: Rename Tab…** sets a custom name
- [ ] Tab shows `⠙` while Claude works and `✓` when a background tab finishes
- [ ] A permission prompt shows a red banner naming the command
- [ ] `Alt+g` lists open tabs and past conversations; resuming one opens it in a new tab
- [ ] `Alt+H` opens a past conversation as a readable transcript; `r` resumes it
- [ ] `Alt+v` splits two agents; `Alt+w` switches pane

## 2. Agent ↔ code
- [ ] Ask Claude for a `file:line`; the path is underlined and clicking opens the file at that line
- [ ] A bare `name.ext:line` works; an ambiguous one opens a picker
- [ ] `Alt+j` labels references; typing a label jumps
- [ ] Select lines in the editor, `Alt+s` pastes `@file#Lx-y` into the agent
- [ ] `Alt+?` finds text in agent output including scrollback; dragging with the mouse copies text
- [ ] Ask Claude "what file am I looking at?" with a file open: it knows (editor context sharing)

## 3. Review agent changes
- [ ] After Claude edits files, the banner offers `Alt+r`; the review lists only those files
- [ ] `a` accepts (stages) a hunk, `x` twice rejects it and the file on disk reverts
- [ ] `s` shows staged changes; `u` unstages a hunk
- [ ] `Alt+C` lists changed files per agent and the tab shows `○N` unreviewed
- [ ] **Git: Log…** and **Git: File History…** open read-only commit diffs

## 4. Editor
- [ ] Type, save (`Ctrl+S`), undo; an agent editing the open file reloads it
- [ ] `Ctrl+D` adds cursors; typing edits all of them
- [ ] `Ctrl+F` / `Ctrl+H` find and replace with regex; `Alt+/` workspace replace asks before writing
- [ ] Click `▾` in the gutter folds; `Ctrl+/` comments; `Alt+↑/↓` moves a line
- [ ] `Ctrl+\` splits the editor; each group keeps its own file
- [ ] A `.md` file opens as a rendered preview; `Alt+m` switches to editing

## 5. Code intelligence (with a language server)
- [ ] Errors show in the gutter and `Alt+i`; `F12` goes to definition; `Shift+F12` lists references
- [ ] `Alt+h` hover, `F2` rename across files, `Alt+.` code actions, `Alt+F` format

## 6. Keys and terminal
- [ ] `Alt` shortcuts work, and `Ctrl+]` then a key works as the fallback
- [ ] On macOS: with Option-as-Meta enabled, `⌥x` opens the palette
- [ ] In a kitty-protocol terminal, `Ctrl+Shift+P` opens the palette
- [ ] A custom `keys` entry in `settings.json` works; a bad one is reported at startup
- [ ] **Preferences: Toggle Dark/Light Theme** switches both UI and syntax colors

## 7. Restore and exit
- [ ] `Alt+q` quits (asks again with unsaved files); relaunching without `--fresh` restores files, tabs, tab names and the Claude conversation

Report anything that fails as an issue with the terminal, OS and the exact steps.
