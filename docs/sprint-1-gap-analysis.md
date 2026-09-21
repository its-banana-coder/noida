# Sprint 1 (Agent Core) — what exists, what is missing

[The spec](agent-native-spec.md) is written as if NOIDA were greenfield. It is not:
roughly half of Sprint 1 already ships. This maps each Sprint 1 item onto the
code that exists today, so the work is *finishing* the agent core rather than
rebuilding it.

Read this before starting on §94. Where something already exists, the job is to
make it explicit and structured, not to write it again.

| Sprint 1 item | Today | Where | Work left |
|---|---|---|---|
| `AgentEvent` | **Mostly built** | `src/hooks.rs` | Events arrive as structured JSON from Claude Code hooks and Codex `notify`, not scraped from the PTY — the hard part of §89 is done. Missing the event classes in §14: `test.*`, `git.*`, `command.*`, `permission.approved/denied`, `agent.failed`. |
| Activity timeline | **Built** | `src/activity.rs`, `src/app/views.rs` | `Turn`/`Entry` with clickable file refs, persisted per project as JSONL. Missing: test and command entries, and replay (§42). |
| Persist agent sessions | **Built** | `src/workspace.rs`, `src/sessions.rs` | Tabs, titles and Claude/Codex conversation ids are restored on launch; past sessions are browsable and resumable. |
| Worktree manager | **Built** | `src/git.rs`, `src/app/agent_views.rs` | Per-agent worktrees on `noida/<slug>` branches, with compare and changed-files views. Missing the lifecycle actions in §20: merge, and delete-with-confirmation. |
| Permission model | **Partial** | `src/hooks.rs` (`Notification { permission }`) | NOIDA *observes* permission prompts and banners them, but the agent CLI owns the decision. §37–38 want a broker with a project policy file. That is a much larger change and should not be started before the state machine lands. |
| Event bus | **Partial** | `src/events.rs` (`Bg`) | One channel carries everything off-thread, but it is a transport, not a bus: events are consumed once, never persisted as a stream, and nothing can subscribe. §14 wants an append-only log that the timeline, context engine and git views all read. |
| Agent state machine | **Missing** | `src/agent.rs` | State is implicit across `exited`, `error`, `working`, `hook_working`, `attention` and `started_at`. §5.2 wants one explicit enum. **Start here** — it is small, self-contained, and every later item (checkpoints, handoff, review, dashboard) needs to ask "what is this agent doing?" and get one answer. |
| `AgentProvider` trait | **Missing** | `src/actions.rs` (`AgentKind`) | Providers are an enum of three known CLIs plus a command string. §66 wants a trait with capabilities. Worth doing *after* the state machine, and only far enough to describe Claude, Codex and a bare shell honestly. |
| `AgentTask` | **Missing** | — | No task, scope or acceptance model (§6). This is the biggest genuinely new piece of Sprint 1. |

## Order of work

1. **Agent state machine** — one enum, derived from the signals already collected, with the UI reading it instead of recomputing status from four booleans.
2. **Event classes** — widen `AgentEvent` to the §14 set that hooks can actually observe today.
3. **Persisted event log** — promote the activity JSONL into the append-only stream §14 describes, and make the timeline a reader of it.
4. **`AgentProvider`** — a trait over what Claude, Codex and a shell can each actually do.
5. **`AgentTask`** — scope and acceptance, once there is a state machine to hang it on.

Permissions (§37–38), checkpoints (§43) and handoff (§22) are deliberately *not*
in this list: each needs the state machine and the event log underneath it first.

## A caution about the spec

Two things in it are aspirational in a way worth naming before anyone plans
around them:

- **Cost reporting (§40).** Claude Code hooks do not report token usage or cost,
  so a cost dashboard cannot be built from the current integration. It would
  need the provider to expose it.
- **Structured findings (§18).** Agents emit prose; NOIDA extracts references
  from it. Getting genuine `finding:` YAML back requires the agent to cooperate
  — a prompt or output contract — not just IDE work.
