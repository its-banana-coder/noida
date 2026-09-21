# NOIDA Plugin Ecosystem Roadmap

This roadmap describes how NOIDA can grow from a focused terminal IDE into a plugin platform for agent-driven development. It is inspired by the parts of VS Code's extension model that have proven durable: manifest-first extensions, declarative contribution points, lazy activation, an isolated extension host, a stable command API, packaging tools, integration tests, and a registry path.

The goal is not to clone VS Code. NOIDA should become extensible in a way that fits its own shape: a Rust terminal IDE with real PTYs, agent panes, clickable references, git review tools, an editor, LSP support, and low overhead.

## Goals

- Let other developers add commands, keybindings, agents, language support, themes, snippets, views, and workflow integrations without modifying NOIDA core.
- Keep startup fast by loading plugin metadata first and activating code only when needed.
- Protect users from untrusted code through an explicit permission model, workspace trust, and process isolation.
- Make plugin development familiar to VS Code extension authors while keeping the API terminal-native.
- Support useful compatibility with VS Code assets where it is cheap and safe, especially snippets, TextMate grammars, language configuration, and LSP declarations.
- Build enough tooling that publishing, installing, testing, and debugging plugins feels like a complete ecosystem instead of an internal escape hatch.

## Non-Goals

- Full VS Code API compatibility in the first versions.
- DOM or browser-style webviews as the primary UI extension point.
- Running arbitrary plugin code inside the NOIDA process.
- Supporting native binary plugins loaded by `dlopen`.
- Building a centralized marketplace before the local plugin model and CLI are stable.

## Design Principles

1. **Manifest first**
   NOIDA should discover what a plugin contributes before running plugin code. This allows fast startup, static validation, permission review, and search/indexing.

2. **Lazy activation**
   Plugin code should activate only on specific events such as command invocation, file open, language detection, agent event, or startup completion.

3. **Out-of-process execution**
   Code plugins should run in a separate extension host process and communicate with NOIDA over a versioned protocol. A crashing plugin should not crash the editor.

4. **Declarative when possible**
   Themes, snippets, language configs, commands, menus, keybindings, and agent definitions should work without executable code.

5. **Terminal-native UI**
   Plugin UI should use safe NOIDA view primitives: lists, trees, forms, markdown, status items, pickers, panels, and review views.

6. **Capability-based permissions**
   Plugins should declare whether they need workspace reads, workspace writes, process execution, network, git mutations, agent input, or agent output access.

7. **Stable small API**
   Prefer a small API that remains stable over a large API that changes frequently. Add capability by adding explicit contribution points and methods.

## Recommended Extension Types

### Declarative Extensions

Declarative extensions contain a manifest and data files but no executable plugin code. These should be supported first because they are safer and simpler.

Examples:

- themes
- snippets
- language configuration
- TextMate syntax grammars
- static command palette entries
- keybinding packs
- agent presets
- LSP server declarations
- file icon mappings

### Code Extensions

Code extensions include a runtime entry point and can register commands, inspect editor state, respond to events, create views, or coordinate with agents.

Recommended first runtime:

- TypeScript or JavaScript on Node.js, through a `noida-extension-host` process.

Possible later runtime:

- WASM plugins for stricter sandboxing and language-neutral distribution.

## Manifest

Use a NOIDA-specific manifest named `noida.extension.json`. This avoids pretending to be a Node package while still feeling familiar to VS Code extension authors.

Example:

```json
{
  "name": "rust-tools",
  "publisher": "acme",
  "version": "0.1.0",
  "displayName": "Rust Tools",
  "description": "Extra Rust commands for NOIDA.",
  "license": "MIT",
  "engines": {
    "noida": "^0.1.0"
  },
  "activationEvents": [
    "onCommand:rustTools.expandMacro",
    "onLanguage:rust"
  ],
  "main": "./dist/main.js",
  "capabilities": {
    "workspaceRead": true,
    "workspaceWrite": false,
    "processExecution": false,
    "network": false,
    "gitWrite": false,
    "agentInput": false,
    "agentOutputRead": false
  },
  "contributes": {
    "commands": [
      {
        "command": "rustTools.expandMacro",
        "title": "Rust: Expand Macro",
        "category": "Rust"
      }
    ],
    "keybindings": [
      {
        "command": "rustTools.expandMacro",
        "key": "alt+m",
        "when": "editorLang == rust"
      }
    ],
    "menus": {
      "commandPalette": [
        {
          "command": "rustTools.expandMacro",
          "when": "editorLang == rust"
        }
      ]
    },
    "settings": {
      "rustTools.extraArgs": {
        "type": "array",
        "items": {
          "type": "string"
        },
        "default": []
      }
    }
  }
}
```

Required fields:

- `name`
- `publisher`
- `version`
- `engines.noida`

Recommended derived identifier:

```text
<publisher>.<name>
```

For example:

```text
acme.rust-tools
```

## Contribution Points

Start with contribution points that naturally match NOIDA's current architecture.

### `contributes.commands`

Adds commands to NOIDA's command registry and command palette.

```json
{
  "command": "demo.sayHello",
  "title": "Demo: Say Hello",
  "category": "Demo"
}
```

Implementation notes:

- Core commands already have stable IDs in `src/actions.rs`.
- Plugin commands should share the same lookup path as built-in actions.
- Command IDs should be globally unique and prefixed by the extension ID.

### `contributes.keybindings`

Adds default keybindings.

```json
{
  "command": "demo.sayHello",
  "key": "alt+y",
  "when": "focus == editor"
}
```

Implementation notes:

- Reuse the existing key parsing and custom keymap machinery.
- User keybindings must always override plugin defaults.
- Conflicts should be visible in a diagnostics or extension details view.

### `contributes.menus`

Adds commands to menu-like locations.

Initial locations:

- `commandPalette`
- `editor/context`
- `tree/context`
- `agent/title`
- `agent/context`
- `review/context`
- `activity/title`

NOIDA has a terminal UI, so menu contributions should render through existing pickers, context command lists, title buttons, and view actions.

### `contributes.settings`

Adds typed settings.

```json
{
  "demo.enabled": {
    "type": "boolean",
    "default": true,
    "description": "Enable Demo behavior."
  }
}
```

Implementation notes:

- Plugin settings can live under `~/.config/noida/settings.json`.
- Settings must be namespaced by extension ID.
- Invalid plugin settings should produce a clear status message but not prevent NOIDA from starting.

### `contributes.agents`

Adds agent tab presets.

```json
{
  "id": "aider",
  "title": "Aider",
  "command": "aider --no-git",
  "kind": "terminal-agent"
}
```

Implementation notes:

- This is a NOIDA-native contribution point and should be a first-class differentiator.
- Agent presets should appear in the command palette and session creation UI.
- Presets that execute commands require the `processExecution` capability.

### `contributes.languages`

Declares language IDs and file associations.

```json
{
  "id": "gleam",
  "extensions": [".gleam"],
  "filenames": [],
  "firstLine": "^#!/.*\\bgleam\\b"
}
```

Implementation notes:

- Use this to improve syntax selection, language-specific keybindings, snippets, and LSP matching.
- Keep it separate from programmatic language features.

### `contributes.grammars`

Adds syntax grammars.

```json
{
  "language": "gleam",
  "scopeName": "source.gleam",
  "path": "./syntaxes/gleam.tmLanguage.json"
}
```

Implementation notes:

- If NOIDA keeps using `syntect`, support TextMate grammar imports where feasible.
- Grammar loading should be cached and should fail gracefully.

### `contributes.snippets`

Adds snippets for a language.

```json
{
  "language": "rust",
  "path": "./snippets/rust.json"
}
```

Implementation notes:

- VS Code snippet JSON compatibility is worth supporting.
- Snippets should be available without activating plugin code.

### `contributes.lspServers`

Adds language server definitions.

```json
{
  "language": "gleam",
  "command": "gleam",
  "args": ["lsp"],
  "rootPatterns": ["gleam.toml", ".git"]
}
```

Implementation notes:

- Extend the existing LSP server selection logic in `src/lsp.rs`.
- Starting an LSP process requires `processExecution`.
- A language server should activate only when a matching file opens.

### `contributes.views`

Adds terminal-native views.

```json
{
  "id": "demo.tasks",
  "title": "Demo Tasks",
  "type": "tree"
}
```

Initial view types:

- `tree`
- `list`
- `markdown`
- `form`
- `status`

Avoid arbitrary HTML views for the first version.

### `contributes.themes`

Adds color themes.

```json
{
  "id": "demo.dark",
  "label": "Demo Dark",
  "path": "./themes/demo-dark.json"
}
```

Implementation notes:

- Start with NOIDA-specific theme keys.
- Add a converter for simple VS Code themes later.

## Activation Events

Initial activation events:

- `onStartupFinished`
- `onCommand:<commandId>`
- `onLanguage:<languageId>`
- `onFileOpen:<glob>`
- `onAgent:<agentKind>`
- `onAgentEvent:<eventName>`
- `onView:<viewId>`
- `onSettingChanged:<settingKey>`

Examples:

```json
{
  "activationEvents": [
    "onCommand:demo.sayHello",
    "onLanguage:rust"
  ]
}
```

Activation rules:

- Manifest contributions are available immediately.
- Code entry points activate only when an activation event matches.
- Activations should be deduplicated per extension.
- Slow activation should be reported to the user.
- Failed activation should disable only the failing extension.

## Extension Host

NOIDA should start an extension host process when the first code plugin needs activation.

Recommended architecture:

```text
NOIDA core
  JSON-RPC over stdio
noida-extension-host
  loads plugin JavaScript
  exposes @noida/plugin-api
```

Responsibilities of NOIDA core:

- discover extension manifests
- validate manifests
- enforce permission decisions
- render UI
- own editor, git, terminal, hooks, and LSP state
- expose a protocol to the extension host

Responsibilities of the extension host:

- load code extensions
- call `activate(context)`
- manage subscriptions
- route command handlers
- serialize plugin API requests
- isolate plugin exceptions
- unload plugins when possible

Core-to-host messages:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "extension/activate",
  "params": {
    "extensionId": "acme.rust-tools",
    "activationEvent": "onCommand:rustTools.expandMacro"
  }
}
```

Host-to-core messages:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "commands/register",
  "params": {
    "command": "rustTools.expandMacro"
  }
}
```

Command invocation:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "commands/execute",
  "params": {
    "command": "rustTools.expandMacro",
    "context": {
      "activeFile": "src/main.rs",
      "selection": {
        "startLine": 10,
        "endLine": 20
      }
    }
  }
}
```

## Plugin API

Publish a TypeScript package named `@noida/plugin-api`.

Example plugin:

```ts
import * as noida from "@noida/plugin-api";

export function activate(context: noida.ExtensionContext) {
  context.subscriptions.push(
    noida.commands.registerCommand("demo.sayHello", async () => {
      await noida.window.showInformationMessage("Hello from a NOIDA plugin");
    })
  );
}

export function deactivate() {}
```

Initial namespaces:

### `noida.commands`

- `registerCommand(id, handler)`
- `executeCommand(id, ...args)`
- `getCommands()`

### `noida.window`

- `showInformationMessage(message, ...items)`
- `showWarningMessage(message, ...items)`
- `showErrorMessage(message, ...items)`
- `showQuickPick(items, options)`
- `showInputBox(options)`
- `createTreeView(id, provider)`
- `createMarkdownView(id, provider)`

### `noida.workspace`

- `root`
- `readFile(path)`
- `writeFile(path, bytes)`
- `findFiles(glob, options)`
- `getConfiguration(section)`
- `onDidChangeConfiguration`

### `noida.editor`

- `activeDocument`
- `openDocument(path, options)`
- `reveal(path, position)`
- `getSelection()`
- `replaceSelection(text)`
- `applyWorkspaceEdit(edit)`
- `onDidOpenDocument`
- `onDidChangeDocument`
- `onDidSaveDocument`

### `noida.agent`

- `getAgents()`
- `sendText(agentId, text)`
- `getVisibleOutput(agentId)`
- `onDidReceiveOutput`
- `onDidStartTurn`
- `onDidFinishTurn`
- `onDidRequestPermission`

### `noida.git`

- `status()`
- `diff(options)`
- `stage(path)`
- `unstage(path)`
- `openReview(paths)`

Write operations should require explicit capabilities.

## Permission Model

Manifest capabilities:

```json
{
  "capabilities": {
    "workspaceRead": true,
    "workspaceWrite": false,
    "processExecution": false,
    "network": false,
    "gitWrite": false,
    "agentInput": false,
    "agentOutputRead": false,
    "secrets": false
  }
}
```

Install-time display:

```text
acme.rust-tools requests:
- Read files in this workspace
- Start configured language server processes

Allow?
```

Runtime rules:

- Deny undeclared capability calls.
- Prompt before first use of sensitive declared capabilities if not approved.
- Store grants per extension ID and extension version range.
- Re-prompt when a plugin update adds capabilities.
- Provide a command to inspect and revoke grants.

Restricted workspace mode:

- Code plugins disabled by default in untrusted workspaces.
- Declarative themes, snippets, and keybindings can remain enabled.
- Plugins requiring `processExecution`, `workspaceWrite`, `gitWrite`, or `network` require trust.

## Storage Layout

Recommended user-level layout:

```text
~/.local/share/noida/extensions/
  acme.rust-tools-0.1.0/
    noida.extension.json
    dist/main.js

~/.local/share/noida/extension-cache/
  registry/
  downloads/
  unpacked/

~/.local/share/noida/extension-state/
  acme.rust-tools/
    global.json

<workspace state directory>/
  extension-state/
    acme.rust-tools.json

~/.config/noida/settings.json
```

Plugin state should be isolated by extension ID.

## Packaging

Package format:

```text
.noix
```

A `.noix` file is a zip archive with a `noida.extension.json` at the root.

Required package contents:

```text
noida.extension.json
README.md
LICENSE
```

Optional package contents:

```text
CHANGELOG.md
dist/
syntaxes/
snippets/
themes/
icons/
```

Package validation:

- manifest schema validation
- SemVer validation
- engine compatibility check
- duplicate command ID check
- declared file path existence check
- package size limit
- checksum generation
- permission summary

## CLI Tooling

Add an extension subcommand namespace:

```bash
noida ext init
noida ext validate
noida ext package
noida ext install ./plugin.noix
noida ext uninstall acme.rust-tools
noida ext list
noida ext enable acme.rust-tools
noida ext disable acme.rust-tools
noida ext publish
```

Development mode:

```bash
noida --extensionDevelopmentPath ./my-extension
```

Test mode:

```bash
noida --extensionDevelopmentPath ./my-extension --extensionTestsPath ./tests
```

## Registry

Start with a simple static registry before building a full marketplace.

Example registry entry:

```json
{
  "id": "acme.rust-tools",
  "publisher": "acme",
  "name": "rust-tools",
  "version": "0.1.0",
  "displayName": "Rust Tools",
  "description": "Extra Rust commands for NOIDA.",
  "license": "MIT",
  "download": "https://github.com/acme/noida-rust-tools/releases/download/v0.1.0/rust-tools.noix",
  "sha256": "...",
  "capabilities": ["workspaceRead"],
  "repository": "https://github.com/acme/noida-rust-tools"
}
```

Registry phases:

1. Static JSON index hosted in the NOIDA repository or a separate registry repository.
2. GitHub Releases based distribution with SHA-256 verification.
3. Search and install commands inside NOIDA.
4. Publisher namespaces.
5. Automated scanning.
6. Web marketplace.
7. Optional self-hosted registry server.

Security scanning:

- secret detection
- suspicious executable detection
- package size checks
- license checks
- typosquatting checks
- duplicate command and contribution checks

## VS Code Compatibility Strategy

Support useful VS Code assets before supporting VS Code APIs.

Compatibility order:

1. VS Code snippet JSON
2. language configuration JSON
3. TextMate grammars
4. simple color themes
5. LSP server declarations
6. keybinding translation where possible
7. read-only import of `.vsix` packages for declarative assets

Avoid early support for:

- VS Code webviews
- debug adapters as a full UI
- arbitrary VS Code `vscode` module API
- extension-to-extension dependency graphs

Long-term option:

- Add a compatibility layer that implements a small subset of the `vscode` API on top of `@noida/plugin-api`.

## Testing Ecosystem

Plugin authors need both unit tests and integration tests.

### Unit Tests

Run in the plugin's normal JavaScript or TypeScript test runner.

### Integration Tests

NOIDA should offer a headless or scripted development host:

```bash
noida --extensionDevelopmentPath ./plugin --extensionTestsPath ./tests
```

Test API scenarios:

- open a workspace
- open a file
- invoke a command
- inspect active editor
- send text to an agent
- simulate an agent hook event
- inspect status messages
- assert file changes
- assert no file changes

Add a fixture mode so tests do not touch the user's real config or extension directories.

## Documentation Set

The plugin ecosystem should eventually include:

- Plugin quickstart
- Manifest reference
- Contribution points reference
- API reference
- Security and permissions guide
- Packaging and publishing guide
- Testing guide
- Migration guide for VS Code extension authors
- Sample plugins repository

Sample plugins:

- hello world command
- keybinding pack
- theme
- snippets
- language support with LSP
- custom agent preset
- agent output analyzer
- git review helper
- markdown view

## Implementation Plan

### Phase 0: Architecture Preparation

Scope:

- Define extension manifest schema.
- Define extension ID rules.
- Define extension directories.
- Define activation events and contribution point names.
- Decide whether the first extension host is bundled or installed separately.

Deliverables:

- `docs/plugin-ecosystem-roadmap.md`
- `docs/plugin-manifest-schema.md`
- JSON schema for `noida.extension.json`
- Internal Rust structs for manifest parsing

Exit criteria:

- NOIDA can scan extension directories and report valid or invalid manifests without running plugin code.

### Phase 1: Declarative Plugin Loader

Scope:

- Load installed extension manifests.
- Validate `engines.noida`.
- Register declarative commands, keybindings, settings, themes, snippets, agents, and LSP servers.
- Add an Extensions view or command palette commands for listing installed plugins.

Core files likely involved:

- `src/actions.rs`
- `src/app/mod.rs`
- `src/settings.rs`
- `src/keys.rs`
- `src/lsp.rs`
- `src/agent.rs`

Deliverables:

- `noida ext list`
- `noida ext validate`
- manifest loading at startup
- plugin contribution diagnostics

Exit criteria:

- A no-code plugin can add a command palette entry, default keybinding, settings schema, and agent preset.

### Phase 2: Command Registry Refactor

Scope:

- Make built-in actions and plugin commands share one registry.
- Store title, category, enablement, keybindings, and source extension.
- Support command invocation by string ID.
- Add command arguments.

Deliverables:

- Stable public command IDs.
- Plugin command namespace validation.
- Command palette includes plugin commands.
- Keybindings can target plugin commands.

Exit criteria:

- A declarative plugin command can appear in the palette, bind to a key, and call either a built-in command or a shell-safe action.

### Phase 3: Extension Host MVP

Scope:

- Implement JSON-RPC protocol over stdio.
- Build `noida-extension-host` for Node.js.
- Load JavaScript plugins.
- Support `activate(context)` and `deactivate()`.
- Support `commands.registerCommand`.
- Support messages, quick pick, input box, and basic editor reads.

Deliverables:

- `@noida/plugin-api`
- extension host package
- activation on command
- plugin error reporting
- plugin output log

Exit criteria:

- A TypeScript plugin can register a command that reads active editor context and displays a message.

### Phase 4: Editor, Workspace, and Agent APIs

Scope:

- Expose safe editor APIs.
- Expose workspace read APIs.
- Expose gated workspace write APIs.
- Expose agent APIs for reading output and sending input.
- Expose hook events.

Deliverables:

- `noida.editor`
- `noida.workspace`
- `noida.agent`
- permission checks
- integration tests for API calls

Exit criteria:

- A plugin can inspect a selected code range, send a prompt to the active agent, and open the referenced file returned by the agent.

### Phase 5: Permission and Trust System

Scope:

- Capability declarations.
- Install-time permission review.
- Runtime enforcement.
- Restricted workspace mode.
- Permission management UI.

Deliverables:

- grants database
- trust prompt
- extension details page
- commands to revoke permissions

Exit criteria:

- A plugin cannot read, write, execute, access network, mutate git, or control agents unless permitted.

### Phase 6: Packaging CLI

Scope:

- Package `.noix` archives.
- Install local packages.
- Uninstall and disable plugins.
- Validate package contents.
- Generate checksums.

Deliverables:

- `noida ext init`
- `noida ext package`
- `noida ext install`
- `noida ext uninstall`
- `noida ext enable`
- `noida ext disable`

Exit criteria:

- A developer can create, package, install, enable, disable, and uninstall a plugin locally.

### Phase 7: Development and Test Experience

Scope:

- Development path loading.
- Watch mode.
- Plugin reload.
- Integration test runner.
- Fixture workspaces.

Deliverables:

- `--extensionDevelopmentPath`
- `--extensionTestsPath`
- plugin logs
- sample test harness

Exit criteria:

- A plugin author can develop and test a plugin without manually copying it into the installed extensions directory.

### Phase 8: Registry MVP

Scope:

- Static registry index.
- Search and install from registry.
- SHA-256 verification.
- Update checks.

Deliverables:

- `noida ext search`
- `noida ext install <id>`
- `noida ext update`
- registry index schema

Exit criteria:

- Users can install a plugin from a public registry index with checksum verification.

### Phase 9: Marketplace Hardening

Scope:

- Publisher namespaces.
- Token-based publishing.
- Automated scanning.
- Abuse reporting.
- Signature verification.
- Web UI.

Deliverables:

- `noida ext publish`
- publisher accounts or namespace ownership
- package scanner
- plugin detail pages

Exit criteria:

- Third-party developers can publish plugins without maintainers editing a static registry file.

## Open Design Questions

- Should the JavaScript extension host be bundled with NOIDA releases or installed separately through npm?
- Should plugin manifests allow npm dependencies, or should packages be fully bundled before publishing?
- Should process execution permission distinguish extension-owned binaries from arbitrary workspace commands?
- Should network access be all-or-nothing, host-scoped, or request-scoped?
- How much VS Code theme compatibility is worth supporting before NOIDA has its own complete theme schema?
- Should plugin APIs expose direct git operations or route all git mutation through existing NOIDA review flows?
- How should plugin views be represented in a terminal layout without adding visual clutter?
- Should plugin updates be automatic, prompted, or manual-only?

## Recommended First Milestone

The first public milestone should be deliberately small:

- Manifest discovery
- Declarative commands
- Declarative keybindings
- Declarative agent presets
- Declarative settings
- Local install/list/disable
- No executable plugin code yet

This milestone gives users immediate customization value and lets NOIDA establish manifest, validation, directories, diagnostics, and UX before introducing extension host security concerns.

After that, add code plugins through the isolated extension host.

