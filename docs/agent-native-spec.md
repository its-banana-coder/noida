# NOIDA — Agent-Native IDE: Next-Level Product & Engineering Specification

**Status:** Proposed product/architecture specification  
**Target:** Turn NOIDA into a serious VS Code alternative optimized around agentic software development.  
**Primary thesis:** Do not build "VS Code but smaller." Build the development environment that coding agents should have had from the beginning.

---

# 1. Executive Summary

NOIDA should evolve from a lightweight terminal IDE into an **agent-native development environment**.

Traditional IDE:

```text
Human
  ↓
Editor
  ↓
Compiler / Runtime
  ↓
Git
```

Agentic development:

```text
Human intent
     ↓
Agent
     ↓
Repository understanding
     ↓
Plan
     ↓
Code changes
     ↓
Tests / Runtime / Debugging
     ↓
Evidence
     ↓
Human review
     ↓
Merge
```

NOIDA should own this entire loop.

The editor is still important, but it is no longer the center of the product. The center becomes:

> **Intent → Context → Agent → Execution → Verification → Review**

The product should make Claude Code, Codex, local models, and future coding agents feel like native development tools rather than external terminal processes.

---

# 2. Product Vision

## 2.1 One-line vision

> **NOIDA is the operating environment for coding agents.**

## 2.2 What this means

A developer should be able to:

1. Open a repository.
2. Understand what is happening.
3. Give an agent a task.
4. Let the agent inspect the repository.
5. Let it work in an isolated environment.
6. Watch what it is doing.
7. Review its changes.
8. Run tests and debugging.
9. Ask another agent to review the work.
10. Approve or reject the result.
11. Merge the work.

All without leaving NOIDA.

---

# 3. Product Principles

## P1 — Agents are first-class citizens

An agent is not a chat panel.

An agent has:

```text
Identity
Provider
Model
Session
Task
Status
Context
Permissions
Workspace
Worktree
Terminal
Files
Changes
Tests
Diagnostics
Cost
History
Artifacts
```

---

## P2 — Evidence beats conversation

Agent responses should become navigable development artifacts.

Instead of:

> "The bug seems to be in the authentication layer."

NOIDA should expose:

```text
Finding

auth/service.ts:142

createSession()
  ↓
called by login()
  ↓
called by auth/controller.ts:87

Related test:
auth/login.test.ts:31

Recent change:
Commit 91f2d3 — "session timeout refactor"
```

The agent should be able to point at actual repository evidence.

---

## P3 — Deterministic systems first, AI second

Never use an LLM for something the IDE can answer deterministically.

Use:

- filesystem
- tree-sitter
- LSP
- Git
- test runners
- debugger
- package manifests
- diagnostics

before asking a model.

Use AI for:

- reasoning
- ranking
- summarization
- planning
- intent understanding
- ambiguity resolution
- context compression

---

## P4 — Human remains the authority

Agents can propose and execute work, but NOIDA must make control obvious.

Sensitive operations should require explicit permission:

```text
Production command
Delete files
Reset branch
Force push
Modify secrets
Network access
Install dependencies
Run arbitrary shell
```

---

## P5 — Parallelism should be native

Agentic coding naturally creates parallel work.

NOIDA should make this easy:

```text
                Main task
                   │
       ┌───────────┼───────────┐
       ↓           ↓           ↓
   Agent A      Agent B      Agent C
   Backend      Frontend      Tests
       │           │           │
   Worktree A  Worktree B  Worktree C
       └───────────┼───────────┘
                   ↓
             Review / Merge
```

---

# 4. Target Experience

A NOIDA workspace should look conceptually like:

```text
┌─────────────────────────────────────────────────────────────────────┐
│ NOIDA                                      project / branch / status │
├────────────┬──────────────────────────────────────────┬─────────────┤
│            │                                          │             │
│ EXPLORER   │                EDITOR                    │ AGENTS      │
│            │                                          │             │
│ src/       │ auth/service.ts                         │ ● Claude    │
│ tests/     │                                          │   Working   │
│ package... │ function login(...) {                    │             │
│            │   ...                                    │ ● Codex     │
│ GIT        │ }                                        │   Reviewing │
│            │                                          │             │
│ TESTS      │                                          │             │
│            │                                          │             │
│ PROBLEMS   │                                          │ ACTIVITY    │
│            │                                          │             │
├────────────┴──────────────────────────────────────────┴─────────────┤
│ Terminal / Test Output / Diff / Debugger / Agent Evidence           │
└─────────────────────────────────────────────────────────────────────┘
```

Agent activity should be as visible as Git status.

---

# 5. Agent Hub

## 5.1 Agent registry

Every agent gets a persistent identity.

```yaml
agent:
  id: agent-42
  provider: codex
  model: gpt-5.x
  task: "Implement session expiration"
  status: working
  worktree: .noida/worktrees/session-expiration
  permissions:
    read: true
    write: true
    shell: true
    network: false
```

## 5.2 Agent states

```text
CREATED
  ↓
PLANNING
  ↓
WORKING
  ↓
VERIFYING
  ↓
WAITING_FOR_REVIEW
  ↓
APPROVED
  ↓
MERGED
```

Failure:

```text
WORKING → FAILED → RECOVERING → WORKING
```

Human:

```text
ANY STATE → PAUSED
```

---

# 6. Structured Agent Tasks

A prompt should become a structured task.

```yaml
task:
  title: "Fix session expiration"
  description: |
    Users are occasionally logged out after refreshing.

  scope:
    include:
      - src/auth/**
      - tests/auth/**
    exclude:
      - infrastructure/**

  acceptance:
    - Existing tests pass
    - Add regression test
    - No public API changes

  verification:
    - npm test
    - npm run typecheck
```

NOIDA can then display:

```text
TASK

Fix session expiration

Progress
✓ Repository understanding
✓ Implementation
✓ Regression test
✓ Typecheck
✗ Integration test

Acceptance: 4/5
```

This is much better than a chat transcript.

---

# 7. Context Engine

The Context Engine should become one of NOIDA's core systems.

## 7.1 Problem

Agents do not need the entire repository.

They need the **right repository slice**.

NOIDA should continuously build a machine-readable representation of the project.

```text
Repository
├── Files
├── Directories
├── Symbols
├── Imports
├── Calls
├── Types
├── Tests
├── Git history
├── Diagnostics
├── Runtime traces
├── Agent observations
└── Documentation
```

---

# 8. Repository Knowledge Graph

Build a graph using:

- tree-sitter
- LSP
- Git
- filesystem
- package manifests
- test metadata
- documentation
- diagnostics

Example:

```text
login()
 │
 ├── validateUser()
 │      └── UserRepository
 │
 ├── createSession()
 │      └── SessionStore
 │
 └── login.test.ts
```

Each symbol node should contain:

```text
symbol
file
range
language
definitions
references
dependencies
dependents
tests
diagnostics
recent_changes
agent_mentions
```

---

# 9. Context Retrieval

For:

> "Fix the login bug"

NOIDA should identify:

```text
HIGH
├── auth/controller.ts
├── auth/service.ts
├── auth/session.ts
└── tests/auth/login.test.ts

MEDIUM
├── middleware/auth.ts
└── database/session.ts

LOW
└── unrelated files
```

Ranking signals:

```text
semantic similarity
+ dependency distance
+ call graph relevance
+ current editor location
+ Git recency
+ diagnostics
+ test relevance
+ agent history
+ user-selected context
```

---

# 10. Context Packs

Allow explicit context packs.

```text
CONTEXT PACK
Authentication

Files
✓ auth/service.ts
✓ auth/session.ts
✓ auth/middleware.ts

Symbols
✓ createSession
✓ validateSession
✓ logout

Tests
✓ auth/login.test.ts

Git
✓ Last 10 authentication commits
```

Actions:

```text
[Send to Agent]
[Save]
[Edit]
[Share]
```

Context packs should be reusable.

---

# 11. Local Model

The local model should **not compete with Claude/Codex**.

It should perform cheap, frequent tasks.

Ideal jobs:

- intent classification
- file ranking
- symbol ranking
- context compression
- file summarization
- diff summarization
- duplicate-work detection
- test recommendation
- activity classification
- stale-context detection

Example:

```text
User:
"Why is checkout failing?"

Local Context Model:

Likely relevant:
1. checkout/service.ts
2. payment/service.ts
3. checkout.test.ts
4. payment/webhook.ts

Reason:
payment webhook → checkout state transition
```

Then the expensive agent receives the selected context.

---

# 12. Local Model Architecture

```text
                         NOIDA
                           │
                    Context Request
                           │
                           ▼
                  ┌─────────────────┐
                  │ Context Router  │
                  └────────┬────────┘
                           │
             ┌─────────────┼─────────────┐
             ↓             ↓             ↓
           LSP/AST        Git        Embeddings
             │             │             │
             └─────────────┼─────────────┘
                           ↓
                    Candidate Context
                           │
                           ▼
                  ┌─────────────────┐
                  │ Local Small LLM │
                  │ 0.5B–3B class   │
                  └────────┬────────┘
                           ↓
                     Ranked Context
                           ↓
                      Claude/Codex
```

The IDE must remain fully functional without the local model.

---

# 13. Agent Context Protocol

NOIDA should create a provider-neutral internal protocol.

Do not hard-code the UI around one agent vendor.

Conceptual interface:

```rust
trait CodingAgent {
    fn start(&self, task: AgentTask) -> AgentSession;
    fn send(&self, session: &AgentSession, message: AgentMessage);
    fn pause(&self, session: &AgentSession);
    fn resume(&self, session: &AgentSession);
    fn stop(&self, session: &AgentSession);
    fn permissions(&self, session: &AgentSession) -> Permissions;
    fn capabilities(&self) -> AgentCapabilities;
}
```

Providers:

```text
Claude
Codex
Gemini
OpenRouter
Ollama
Custom CLI
Future agents
```

The agent integration layer should normalize:

```text
messages
tool calls
file reads
file writes
commands
permissions
status
errors
cost
token usage
```

---

# 14. Agent Event Bus

Everything agents do should become structured events.

Example:

```json
{
  "timestamp": "...",
  "agent_id": "agent-42",
  "type": "file_edit",
  "file": "src/auth/service.ts",
  "range": {
    "start": 142,
    "end": 161
  }
}
```

Event types:

```text
agent.created
agent.started
agent.paused
agent.completed
agent.failed

file.read
file.created
file.edited
file.deleted

command.started
command.completed
command.failed

test.started
test.passed
test.failed

diagnostic.created
diagnostic.resolved

git.commit
git.checkout
git.merge

permission.requested
permission.approved
permission.denied
```

This event bus becomes the backbone of the product.

---

# 15. Activity Timeline

Every agent gets a timeline.

```text
Claude — Fix checkout bug

14:31  Read checkout/service.ts
14:32  Read payment/webhook.ts
14:34  Found failing test
14:36  Edited checkout/service.ts
14:37  Added checkout.test.ts
14:38  npm test
14:39  ✗ 1 failure
14:40  Investigating
```

Each event should be clickable.

Clicking:

```text
Read checkout/service.ts
```

opens the file.

Clicking:

```text
Edited checkout/service.ts
```

opens the diff.

Clicking:

```text
npm test
```

opens terminal output.

---

# 16. Agent-to-Code Linking

If an agent says:

```text
The bug is in src/payment/service.ts:142
```

NOIDA should automatically detect:

```text
file:line
file:start-end
symbol
commit
test
URL
```

and render them as clickable objects.

This is one of the highest-value integrations between agent and editor.

---

# 17. Human → Agent Context Sharing

Current editor context should be shareable instantly.

Examples:

```text
Alt+S
```

sends:

```text
Current file
Selected lines
Current symbol
Nearby symbols
Diagnostics
Git diff
```

Potential command:

```text
Explain this
```

should produce:

```text
Context:
src/auth/service.ts
lines 142–161
symbol: createSession

Diagnostics:
TS2345

Recent changes:
commit 91f2d3
```

No manual copy/paste.

---

# 18. Agent → Human Evidence

Agent output should contain structured evidence:

```yaml
finding:
  summary: "Session cookie is created before refresh token validation."
  evidence:
    - file: src/auth/session.ts
      line: 142
    - file: src/auth/refresh.ts
      line: 89
  confidence: high
```

UI:

```text
FINDING

Session cookie is created before refresh
token validation.

Evidence
→ session.ts:142
→ refresh.ts:89

[Open all]
```

---

# 19. Git as a First-Class Agent Primitive

Git should not merely show changed files.

NOIDA should understand:

```text
Agent
  ↓
Worktree
  ↓
Branch
  ↓
Commits
  ↓
Diff
  ↓
Tests
  ↓
Review
```

Agent panel:

```text
Claude

Branch:
agent/session-fix

Changes:
+142
-38

Files:
5

Tests:
18/18

Commits:
2
```

---

# 20. Worktree Manager

Parallel agents should automatically use isolated worktrees.

```text
main
 │
 ├── .noida/worktrees/auth
 ├── .noida/worktrees/payment
 └── .noida/worktrees/tests
```

UI:

```text
WORKTREES

● main
  Current

● auth
  Claude
  6 files changed

● payment
  Codex
  3 files changed

● tests
  Claude
  1 file changed
```

Actions:

```text
[Open]
[Compare]
[Review]
[Merge]
[Delete]
```

---

# 21. Multi-Agent Collaboration

Agents should be able to work as a team.

Example:

```text
Task: Build OAuth login

                 Lead Agent
                     │
        ┌────────────┼────────────┐
        ↓            ↓            ↓
   Architecture   Backend      Tests
        │            │            │
      Agent A      Agent B      Agent C
        └────────────┼────────────┘
                     ↓
                 Reviewer
                     ↓
                 Human
```

Agent roles:

```text
planner
implementer
reviewer
tester
debugger
researcher
```

---

# 22. Agent Handoff

An agent should be able to hand work to another agent.

Example:

```text
Claude:

Implementation complete.

I recommend handing verification to Codex.

Reason:
- 8 files changed
- API contract changed
- Integration tests are failing

[Hand to Codex]
```

The handoff package:

```text
task
plan
changed files
diff
tests
failures
context pack
agent observations
open questions
```

---

# 23. Agent Review Mode

A reviewer agent should receive:

```text
Original task
Acceptance criteria
Diff
Tests
Diagnostics
Relevant architecture
Git history
```

Then return:

```text
REVIEW

✓ Scope respected
✓ Regression test added
✓ Existing API preserved

Issues

⚠ Medium
auth/session.ts:88

Token expiry is hard-coded.

⚠ Low
Missing edge-case test for expired refresh token.

[Open]
```

The reviewer must be able to link every finding to evidence.

---

# 24. Agent Consensus Without Fake Consensus

Do not simply ask three agents for answers and display a winner.

Instead:

```text
Agent A
Agent B
Agent C
   ↓
Independent findings
   ↓
Evidence normalization
   ↓
Contradictions
   ↓
Human decision
```

UI:

```text
QUESTION

Why is checkout failing?

Claude:
Payment webhook race condition.

Codex:
Checkout state is stale.

Local analysis:
Both touch the same transition.

Shared evidence:
checkout/service.ts:184

Contradiction:
Webhook timing vs stale cache.

[Inspect]
```

NOIDA should expose disagreement rather than hide it.

---

# 25. Testing System

Testing needs to become a first-class panel.

```text
TESTS

✓ auth/login.test.ts
✓ auth/logout.test.ts
✓ checkout/cart.test.ts
✗ checkout/payment.test.ts

18 passed
1 failed
```

Actions:

```text
Run
Run File
Run Test
Debug
Run Failed
```

---

# 26. Agentic Testing

When a test fails:

```text
✗ checkout/payment.test.ts

[Explain]
[Fix with Agent]
[Debug]
[Open]
```

"Fix with Agent" automatically supplies:

```text
test
failure
stack trace
relevant source
recent Git changes
related tests
```

---

# 27. Debugger

Implement debugger support using **Debug Adapter Protocol (DAP)**.

Architecture:

```text
NOIDA
  ↓
DAP
  ↓
debugpy / codelldb / delve / js-debug / etc.
```

Required features:

```text
Breakpoints
Conditional breakpoints
Step over
Step into
Step out
Continue
Pause
Call stack
Variables
Watch expressions
Exception breakpoints
Debug console
```

Agent integration:

```text
Agent:
Start debugger
Set breakpoint at payment/service.ts:142
Continue
Inspect variables
```

The debugger should become another agent tool.

---

# 28. Agent Debugging

A particularly valuable feature:

```text
[Debug with Agent]
```

Flow:

```text
Failure
  ↓
Agent receives stack trace
  ↓
NOIDA starts debugger
  ↓
Agent sets breakpoint
  ↓
Agent inspects runtime state
  ↓
Agent forms hypothesis
  ↓
Agent modifies code
  ↓
Agent reruns test
```

This turns debugging into an observable workflow.

---

# 29. Problems / Diagnostics

Create a unified diagnostics system.

Sources:

```text
LSP
Compiler
Linter
Test runner
Debugger
Runtime
Agent
```

Example:

```text
PROBLEMS 7

ERRORS 3
  auth/service.ts:142
  checkout/payment.ts:88

WARNINGS 4
  unused import
  deprecated API
```

Clicking a problem should open:

```text
source
diagnostic
related symbols
agent context
quick fixes
```

---

# 30. Runtime Intelligence

NOIDA should understand application execution.

For supported runtimes:

```text
Process
Ports
Logs
Errors
Requests
Stack traces
Environment
```

Example:

```text
RUNTIME

API
● localhost:3000
  Healthy

Worker
● localhost:4000
  Healthy

Database
● postgres:5432
  Connected
```

Agent action:

```text
Inspect runtime
```

should produce structured evidence rather than dumping an entire terminal.

---

# 31. Logs as Structured Context

Instead of:

```text
[huge terminal output]
```

extract:

```text
ERROR

PaymentService
Timeout after 5000ms

Request:
POST /payments

Trace:
checkout
 → payment
 → gateway

Occurrences:
47
```

This can feed the Context Engine.

---

# 32. Remote Development

Remote SSH should be a major milestone.

Architecture:

```text
LOCAL NOIDA
     │
     │ SSH
     ▼
REMOTE NOIDA RUNTIME
     │
     ├── filesystem
     ├── LSP
     ├── Git
     ├── agents
     ├── tests
     └── debugger
```

The local UI should feel identical to local development.

Target:

```bash
noida ssh://server
```

---

# 33. Dev Containers

Support:

```text
.devcontainer/devcontainer.json
```

Flow:

```text
Open repository
      ↓
Detect devcontainer
      ↓
Start environment
      ↓
Attach NOIDA
      ↓
Start agent
```

Agent context should know:

```text
container
OS
runtime
dependencies
ports
environment
```

---

# 34. Plugin Architecture

Do not clone the entire VS Code extension API.

Build a smaller NOIDA-native plugin API.

Plugins should be able to expose:

```text
Commands
Panels
File decorations
Language integrations
LSP configuration
DAP configuration
Test adapters
Agent tools
Context providers
Runtime providers
```

Example:

```text
noida-python
noida-docker
noida-kubernetes
noida-postgres
noida-terraform
noida-react
```

---

# 35. Agent Tool API

NOIDA should expose a stable internal tool protocol.

Example:

```json
{
  "name": "noida.open_file",
  "arguments": {
    "path": "src/auth/service.ts",
    "line": 142
  }
}
```

Core tools:

```text
noida.search
noida.open_file
noida.symbol
noida.references
noida.definition
noida.diff
noida.git
noida.tests
noida.diagnostics
noida.debug
noida.runtime
noida.context
noida.worktree
```

This allows agents to interact with NOIDA itself.

---

# 36. MCP Integration

NOIDA should support MCP-style tool integration where appropriate.

Potential model:

```text
Agent
  │
  ├── filesystem
  ├── shell
  ├── Git
  └── NOIDA MCP
         ├── symbols
         ├── diagnostics
         ├── tests
         ├── debugger
         ├── runtime
         ├── context
         └── worktrees
```

The agent should not need to infer NOIDA's internal state from terminal output.

---

# 37. Permission System

Every agent action should have a permission class.

```text
READ
WRITE
EXECUTE
NETWORK
GIT
DEBUG
SECRETS
PRODUCTION
```

Policies:

```yaml
permissions:
  read: allow
  write: allow
  execute: ask
  network: deny
  git_commit: ask
  git_push: ask
  secrets: deny
```

Allow project-level configuration:

```text
.noida/policy.yaml
```

---

# 38. Permission UX

When an agent requests something:

```text
PERMISSION REQUEST

Claude wants to run:

npm install stripe

Effects:
- modifies package.json
- modifies package-lock.json
- network access required

[Allow once]
[Allow for project]
[Deny]
```

Do not hide permission decisions in chat.

---

# 39. Secrets Safety

Agents should not automatically receive:

```text
.env
SSH keys
cloud credentials
tokens
private keys
```

NOIDA should support:

```text
secret redaction
secret-aware terminal
permission scopes
environment filtering
```

---

# 40. Agent Cost & Resource Dashboard

For external models:

```text
TODAY

Claude
$1.42

Codex
$0.86

Other
$0.14

Total
$2.42
```

Per task:

```text
Authentication
Duration: 18m
Model calls: 14
Tokens: 82k
Cost: $0.73
```

For local models:

```text
Local model
Tokens: 42k
GPU: 2.1 GB
Time: 31 sec
Cost: local
```

---

# 41. Agent Performance Metrics

Useful metrics:

```text
Time to first useful change
Time to passing tests
Number of iterations
Files touched
Lines changed
Failed commands
Failed tests
Context size
Human interventions
Rework rate
```

Do not turn these into simplistic "agent quality scores."

Use them for debugging the workflow.

---

# 42. Session Replay

Every agent session should be replayable.

```text
SESSION REPLAY

14:31 Read auth.ts
14:32 Read session.ts
14:34 Edited auth.ts
14:35 Test failed
14:36 Read stack trace
14:38 Changed session.ts
14:39 Test passed
```

Click any event to inspect:

```text
before
after
context
agent message
tool call
result
```

This will be extremely valuable for debugging agents.

---

# 43. Agent Checkpoints

Agents should create checkpoints automatically.

```text
CHECKPOINTS

● Initial understanding
● First implementation
● Tests added
● Before refactor
● Final
```

Actions:

```text
Restore
Compare
Fork Agent
Review
```

This makes agent experimentation safer.

---

# 44. Agent Rollback

If an agent makes a mess:

```text
[Rollback Agent Changes]
```

should restore the agent worktree to the last checkpoint.

Never require the user to manually recover from a bad agent session.

---

# 45. Intent Bar

Add a global command/task bar.

Examples:

```text
> Fix the authentication bug

> Explain this function

> Find all usages of UserService

> Run tests related to this file

> Ask Codex to review this diff

> Start a debugging session

> Find why checkout is slow
```

The Intent Bar should route requests to:

```text
deterministic command
local model
agent
multi-agent workflow
```

depending on the request.

---

# 46. Intent Classification

A local lightweight classifier can decide:

```text
"Where is this defined?"
→ LSP

"Run the auth tests"
→ test runner

"Why is auth failing?"
→ context + agent

"Rename this"
→ LSP

"Refactor authentication"
→ agent

"Show me what changed"
→ Git
```

This reduces unnecessary LLM calls.

---

# 47. Agent Command Palette

Commands:

```text
Agent: New
Agent: Pause
Agent: Resume
Agent: Stop
Agent: Review
Agent: Handoff
Agent: Compare
Agent: Retry
Agent: Rollback
Agent: Fork
Agent: Open Context
Agent: Show Activity
```

---

# 48. Agent Forking

If an agent is going in the wrong direction:

```text
Fork Agent

Current state:
Checkpoint #3

New task:
"Try an alternative approach using Redis."

[Create Fork]
```

Result:

```text
Agent A
  └── approach 1

Agent A'
  └── approach 2
```

Both can be compared later.

---

# 49. Diff Intelligence

Normal diff:

```text
+ line
- line
```

Agent-native diff:

```text
WHY

Added session validation.

WHY
Prevents stale sessions after refresh.

RELATED
Issue: #143
Test: auth/session.test.ts

RISK
Medium

TESTED
✓ 18 tests
```

The agent can attach rationale to changes.

---

# 50. Change Groups

Group changes by intent:

```text
CHANGES

Authentication
  ├── auth/service.ts
  ├── auth/session.ts
  └── auth.test.ts

Logging
  ├── logger.ts
  └── config.ts
```

This is more useful than a flat file list.

---

# 51. Semantic Diff

Eventually support:

```text
FUNCTION CHANGED

Before:
createSession(user)

After:
createSession(user, options)

Impact:
12 callers

Tests:
4 affected

Potential breaking change:
Yes
```

This should use AST/LSP analysis rather than an LLM alone.

---

# 52. Architecture View

Add a project architecture view:

```text
APP

Frontend
 ├── components
 ├── pages
 └── state

Backend
 ├── API
 ├── Services
 ├── Database
 └── Workers

Infrastructure
 ├── Docker
 ├── Terraform
 └── CI
```

Then:

```text
[Ask Agent About This Architecture]
```

---

# 53. Dependency Graph

Allow:

```text
Package
Module
File
Symbol
```

levels.

Example:

```text
checkout
   ↓
payment
   ↓
stripe
```

Use cases:

- impact analysis
- context retrieval
- refactoring
- agent planning
- review
- debugging

---

# 54. "Why is this code here?"

For a selected symbol:

```text
Why is this here?

Introduced:
commit 91f2d3

Used by:
8 callers

Tests:
3

Dependencies:
PaymentService
SessionStore

Recent changes:
2 commits

Agent explanation:
...
```

This can become one of the best exploratory features in the IDE.

---

# 55. "What will break if I change this?"

For a symbol:

```text
IMPACT ANALYSIS

Direct callers: 8
Indirect callers: 23
Tests: 11
Public APIs: 1

Potentially affected:
✓ checkout
✓ refunds
⚠ reporting

Suggested tests:
✓ checkout/payment.test.ts
✓ refunds/refund.test.ts
```

Deterministic analysis first, AI explanation second.

---

# 56. Project Memory

NOIDA should maintain durable project-level memory.

Store:

```text
Architecture decisions
Important conventions
Known constraints
Known bugs
Build commands
Test commands
Deployment notes
Agent lessons
```

Potential file:

```text
.noida/
  memory/
    architecture.md
    conventions.md
    decisions.md
    known-issues.md
```

Agents can consume these automatically.

---

# 57. Agent Memory

Each agent can have temporary session memory.

```text
Agent memory

Assumption:
SessionStore is Redis-backed.

Constraint:
Do not change public API.

Known failure:
Integration test requires Docker.
```

At session end:

```text
[Save useful knowledge to project memory]
```

Nothing should be silently persisted as permanent truth.

---

# 58. Repository Onboarding

Opening a new repository should trigger:

```text
ANALYZING PROJECT...

✓ Languages
✓ Frameworks
✓ Package manager
✓ Git
✓ Tests
✓ LSP
✓ Runtime
✓ Dependencies
✓ Architecture
```

Then:

```text
PROJECT SUMMARY

TypeScript
React
Node
Postgres
Docker

Tests:
312

Entrypoints:
3

Services:
5

Suggested commands:
npm run dev
npm test
npm run build
```

This summary can be generated once and cached.

---

# 59. Agent-Ready Repository Index

Index should include:

```text
Files
Symbols
Definitions
References
Imports
Calls
Tests
Packages
Commands
Docs
Git history
Runtime metadata
```

Incrementally update it when files change.

Do not re-index the entire repository after every edit.

---

# 60. Performance Requirements

NOIDA should feel local even while agents run.

Targets:

```text
Editor input latency: < 16ms target
File open: near-instant for normal files
Search: < 100ms target for indexed repositories
Symbol lookup: < 100ms target
Context ranking: < 500ms target with local model
Agent events: streaming
UI never blocked by agent process
```

All agent execution must be asynchronous.

---

# 61. Architecture

Suggested high-level architecture:

```text
┌──────────────────────────────────────────────┐
│                 NOIDA UI                     │
│                                              │
│ Editor │ Explorer │ Git │ Agents │ Tests    │
│ Debug  │ Runtime  │ Problems │ Context      │
└───────────────────────┬──────────────────────┘
                        │
                Application Core
                        │
        ┌───────────────┼────────────────┐
        ↓               ↓                ↓
   Agent Runtime    Context Engine    Workspace
        │               │                │
        ↓               ↓                ↓
   Providers        Code Graph        Git
   Claude           LSP               Worktrees
   Codex            Tree-sitter       Filesystem
   Local            Embeddings
        │               │
        └───────┬───────┘
                ↓
          Event Bus
                │
       ┌────────┼────────┐
       ↓        ↓        ↓
    Timeline  Metrics  Persistence
```

---

# 62. Persistence

NOIDA should persist:

```text
Workspace metadata
Agent sessions
Agent events
Checkpoints
Context packs
Project memory
Permissions
Preferences
```

Avoid storing provider secrets in plain project files.

---

# 63. Suggested Internal Modules

```text
src/
├── app/
├── editor/
├── workspace/
├── filesystem/
├── git/
├── lsp/
├── tree_sitter/
├── diagnostics/
├── testing/
├── debugger/
├── runtime/
├── agents/
│   ├── core/
│   ├── providers/
│   ├── permissions/
│   ├── sessions/
│   └── tools/
├── context/
│   ├── index/
│   ├── graph/
│   ├── retrieval/
│   ├── ranking/
│   └── local_model/
├── worktrees/
├── plugins/
├── memory/
└── telemetry/
```

Names can be adapted to the current codebase.

---

# 64. Data Model

Core entities:

```text
Workspace
Project
Agent
AgentSession
AgentTask
AgentEvent
ContextPack
ContextNode
ContextEdge
Worktree
Checkpoint
ChangeSet
TestRun
Diagnostic
DebugSession
RuntimeProcess
Plugin
Permission
MemoryEntry
```

Relationships:

```text
Workspace
 ├── Projects
 ├── Agents
 ├── Worktrees
 └── Context

Agent
 ├── Sessions
 ├── Tasks
 ├── Events
 ├── Worktree
 ├── Checkpoints
 └── Changes

Task
 ├── Acceptance criteria
 ├── Context
 ├── Tests
 └── Review
```

---

# 65. Plugin API

A minimal plugin API should support:

```rust
trait NoidaPlugin {
    fn metadata(&self) -> PluginMetadata;

    fn commands(&self) -> Vec<Command>;

    fn panels(&self) -> Vec<Panel>;

    fn tools(&self) -> Vec<AgentTool>;

    fn context_providers(&self) -> Vec<ContextProvider>;

    fn test_adapter(&self) -> Option<TestAdapter>;

    fn debugger(&self) -> Option<DebugAdapter>;
}
```

Start narrow.

Expand only when real plugins require it.

---

# 66. Agent Provider API

```rust
trait AgentProvider {
    fn name(&self) -> &str;

    fn capabilities(&self) -> AgentCapabilities;

    fn start(&self, request: StartRequest)
        -> Result<AgentSession>;

    fn send(&self, session: SessionId, message: Message)
        -> Result<()>;

    fn stop(&self, session: SessionId)
        -> Result<()>;
}
```

Capabilities:

```text
streaming
tool_use
file_edits
shell
permissions
structured_output
cost_reporting
session_resume
```

---

# 67. Agent Tool Registry

Agents should discover NOIDA capabilities dynamically.

```text
Tools

noida.open_file
noida.search
noida.symbol
noida.references
noida.definition
noida.diagnostics
noida.git_diff
noida.git_history
noida.run_test
noida.debug
noida.runtime_logs
noida.context
noida.create_worktree
noida.compare_worktrees
```

Tool descriptions should be concise and machine-readable.

---

# 68. Security Model

Threat model:

```text
Untrusted repository
       ↓
Untrusted dependencies
       ↓
Agent-generated commands
       ↓
Potential filesystem/network access
```

Therefore:

```text
Agent
 ↓
Permission broker
 ↓
Policy engine
 ↓
Execution sandbox
```

Never assume an agent command is safe because the model suggested it.

---

# 69. Sandboxing

Long-term:

```text
Agent
 ↓
Sandbox
 ├── filesystem scope
 ├── network scope
 ├── process scope
 └── resource limits
```

Example:

```yaml
sandbox:
  filesystem:
    read:
      - project
    write:
      - project
  network: false
  processes: allow
```

---

# 70. Recovery Architecture

Agent failures are normal.

NOIDA should treat recovery as a first-class workflow.

Failure:

```text
Command failed
     ↓
Capture output
     ↓
Classify failure
     ↓
Attach evidence
     ↓
Agent retry / human intervention
```

UI:

```text
COMMAND FAILED

npm test

Exit code: 1

Likely cause:
Missing DATABASE_URL

[Fix with Agent]
[Open Terminal]
[Retry]
```

---

# 71. Agent Failure Taxonomy

Classify:

```text
Syntax
Type
Test
Dependency
Environment
Network
Permission
Timeout
Resource
Unknown
```

This classification can be deterministic where possible.

---

# 72. Background Intelligence

NOIDA can continuously perform cheap background work:

```text
Index repository
Update dependency graph
Index Git changes
Detect diagnostics
Update test map
Generate summaries
Detect stale agent context
```

But:

> Never make the editor feel busy just because background intelligence is running.

---

# 73. Smart Notifications

Do not spam the user.

Notify only when:

```text
Agent needs permission
Agent finished
Agent failed
Tests changed status
Another agent found a conflict
Important diagnostic appeared
```

Everything else belongs in the activity timeline.

---

# 74. Agent Dashboard

A useful dashboard:

```text
TODAY

Agents
3 active
2 completed
1 failed

Tasks
7 completed
2 waiting

Tests
218 passed
3 failed

Changes
41 files
+1,482
-612

Human interventions
4

Average task duration
12m
```

Metrics are informational, not an "agent quality score."

---

# 75. UX Priority

The interface should emphasize:

```text
1. Current task
2. Agent state
3. Changes
4. Problems
5. Tests
6. Evidence
7. Context
8. Git
```

The chat transcript should not dominate the screen.

---

# 76. Keyboard-First Design

NOIDA is terminal-native.

Make the keyboard a superpower.

Suggested commands:

```text
Ctrl+Space
Intent bar

Alt+A
Agent panel

Alt+S
Send context

Alt+R
Review changes

Alt+T
Run relevant tests

Alt+D
Debug

Alt+W
Worktrees

Alt+C
Context pack

Alt+E
Explain selected code
```

Exact shortcuts should be configurable.

---

# 77. Agent-Native Command Palette

Examples:

```text
> Ask Claude to explain this
> Ask Codex to review this diff
> Create a worktree for this task
> Run relevant tests
> Debug this failure
> Find callers
> Find affected tests
> Create context pack
> Compare agent implementations
```

---

# 78. VS Code Replacement Requirements

To become a credible full-time IDE replacement, NOIDA should eventually cover:

## Editor

- [x] Core editing
- [x] Multi-cursor
- [x] Search
- [x] Split views
- [x] Syntax highlighting
- [x] LSP foundation
- [ ] More language coverage

## Git

- [x] Git basics
- [x] Diff
- [ ] Advanced merge conflict UX
- [ ] Semantic diff
- [ ] Agent-aware change groups

## AI

- [x] Agent terminal integration
- [x] Agent activity
- [x] Worktrees
- [ ] Provider abstraction
- [ ] Context engine
- [ ] Local model
- [ ] Multi-agent orchestration
- [ ] Agent review
- [ ] Agent replay

## Development

- [ ] DAP debugger
- [ ] Test explorer
- [ ] Runtime manager
- [ ] Dev containers
- [ ] Remote SSH
- [ ] Plugin API

## Agent infrastructure

- [ ] Permission broker
- [ ] Agent tool API
- [ ] Event bus
- [ ] Context protocol
- [ ] Checkpoints
- [ ] Rollback
- [ ] Handoff
- [ ] Agent memory

---

# 79. Roadmap

## Phase 0 — Foundation

**Goal:** Make the existing agent experience robust.

### Build

- Agent abstraction
- Agent event model
- Agent state machine
- Activity timeline
- Persistent sessions
- Permission model
- Worktree manager
- Checkpoints

### Exit criteria

A developer can run multiple agents safely and understand exactly what each one is doing.

---

# 80. Phase 1 — Context Engine

**Goal:** Make NOIDA dramatically better at giving agents the right information.

### Build

- Repository index
- Symbol graph
- Dependency graph
- LSP integration
- Context packs
- Context ranking
- Git-aware context
- Test-aware context
- Optional local model

### Exit criteria

For a medium repository, NOIDA can identify relevant files/symbols/tests for a task without requiring the user to manually gather context.

---

# 81. Phase 2 — Verification

**Goal:** Make agent output trustworthy.

### Build

- Test explorer
- Unified diagnostics
- DAP debugger
- Runtime manager
- Agent-driven test execution
- Agent-driven debugging
- Evidence-linked findings

### Exit criteria

An agent can:

```text
Implement
→ Test
→ Debug
→ Fix
→ Retest
→ Present evidence
```

without the human manually stitching the workflow together.

---

# 82. Phase 3 — Multi-Agent

**Goal:** Make parallel agent development native.

### Build

- Worktree orchestration
- Agent handoff
- Agent fork
- Reviewer agent
- Parallel task board
- Conflict detection
- Diff comparison
- Merge assistant

### Exit criteria

A developer can give one large task to multiple agents and inspect the resulting approaches without manually managing branches.

---

# 83. Phase 4 — Remote & Containers

### Build

- SSH
- Remote filesystem
- Remote LSP
- Remote agents
- Dev containers
- Environment detection
- Port management

### Exit criteria

A developer can work on a remote Linux machine from NOIDA without losing agent functionality.

---

# 84. Phase 5 — Plugin Platform

### Build

- Plugin API
- Plugin registry
- Language packages
- Debug adapters
- Test adapters
- Runtime providers
- Context providers

### Initial plugins

```text
Python
JavaScript / TypeScript
Go
Rust
Java
Docker
Kubernetes
Postgres
Terraform
```

---

# 85. Phase 6 — Agent Operating System

This is the long-term differentiator.

NOIDA becomes a system where agents can:

```text
Understand repository
Plan
Create worktree
Implement
Run tests
Debug
Review
Ask another agent
Compare approaches
Recover
Commit
Request approval
Merge
```

The human becomes the orchestrator rather than the typist.

---

# 86. Example End-to-End Workflow

User:

> Fix the checkout failure after payment succeeds.

NOIDA:

```text
1. Detect intent
2. Query diagnostics
3. Find failing tests
4. Inspect Git changes
5. Build context graph
6. Identify checkout/payment paths
7. Create task
8. Create worktree
9. Start Claude
```

Agent:

```text
Investigating...
```

NOIDA timeline:

```text
✓ checkout/service.ts
✓ payment/webhook.ts
✓ checkout/payment.test.ts
```

Agent identifies:

```text
Race condition between webhook
and checkout state transition.
```

NOIDA:

```text
Evidence
→ payment/webhook.ts:91
→ checkout/service.ts:184
→ checkout/payment.test.ts:44
```

Agent edits.

Runs:

```text
npm test
```

Result:

```text
✗ 1 test
```

NOIDA:

```text
[Debug with Agent]
```

Agent starts debugger.

Finds:

```text
state = pending
expected = paid
```

Fixes.

Runs tests:

```text
✓ 312 tests
```

Reviewer agent:

```text
✓ No unrelated changes
✓ Regression test added
✓ Race condition addressed

Potential concern:
Webhook retry behavior is not covered.
```

NOIDA:

```text
TASK COMPLETE

Files changed: 4
Tests: 312/312
Reviewer: 1 concern

[Review Diff]
[Ask Agent]
[Merge]
```

This is the target experience.

---

# 87. What NOT to Build

Do not spend early engineering time on:

- hundreds of themes
- elaborate visual customization
- built-in browser
- proprietary cloud account
- cloud sync
- built-in foundation model
- huge extension marketplace
- visual Docker management
- every obscure VS Code feature
- recreating VS Code's entire extension API

These do not create the core differentiation.

---

# 88. Highest-Leverage Features

If engineering capacity is limited, prioritize:

## Tier 1

1. **Agent event bus**
2. **Context Engine**
3. **Worktree orchestration**
4. **Agent activity timeline**
5. **Permission system**
6. **Test explorer**
7. **DAP debugger**

## Tier 2

8. Agent review
9. Agent handoff
10. Agent checkpoints
11. Agent rollback
12. Local context model
13. Repository architecture graph
14. Remote SSH

## Tier 3

15. Dev containers
16. Plugin API
17. Runtime intelligence
18. Semantic diff
19. Project memory
20. Agent cost dashboard

---

# 89. The Most Important Architectural Decision

Do **not** make the agent terminal the source of truth.

Today, a conventional integration often looks like:

```text
NOIDA
  ↓
PTY
  ↓
Claude/Codex
  ↓
terminal output
```

The future should be:

```text
                    ┌───────────────┐
                    │ Agent Runtime │
                    └───────┬───────┘
                            │
                    Structured Events
                            │
                            ▼
                    ┌───────────────┐
                    │ NOIDA EventBus│
                    └───────┬───────┘
                            │
          ┌─────────────────┼─────────────────┐
          ↓                 ↓                 ↓
       Timeline          Context            Git
          ↓                 ↓                 ↓
       UI / Logs       Code Graph         Changes
          │                 │                 │
          └─────────────────┼─────────────────┘
                            ↓
                      Human Review
```

PTY remains supported because it is useful and compatible.

But NOIDA should increasingly understand the agent's actions structurally.

---

# 90. The Ultimate Product Model

The final NOIDA architecture should feel like:

```text
                         HUMAN
                           │
                           ▼
                     INTENT / TASK
                           │
                           ▼
                    ┌─────────────┐
                    │ NOIDA CORE  │
                    └──────┬──────┘
                           │
          ┌────────────────┼─────────────────┐
          ↓                ↓                 ↓
       CONTEXT          AGENTS            TOOLS
          │                │                 │
       Code Graph      Claude/Codex       Git
       LSP             Local Models       Tests
       AST             Other Agents       Debugger
       Git             Handoff            Runtime
       Tests           Review             Shell
          │                │                 │
          └────────────────┼─────────────────┘
                           ↓
                       EXECUTION
                           │
                           ▼
                       EVIDENCE
                           │
                           ▼
                    HUMAN REVIEW
                           │
                           ▼
                         MERGE
```

The key loop is:

> **Intent → Context → Agent → Execution → Evidence → Review**

Everything in NOIDA should strengthen this loop.

---

# 91. Success Metrics

Do not measure success by feature count.

Measure:

### Developer efficiency

- Time from task creation → first useful change
- Time from first change → passing tests
- Manual context gathering time
- Manual agent coordination time

### Agent effectiveness

- Successful task completion rate
- Number of retries
- Number of human interventions
- Number of reverted changes
- Test pass rate after agent completion

### Context quality

- Relevant files selected
- Context size
- Retrieval latency
- Context corrections by humans

### Product quality

- Editor latency
- Crash rate
- Agent event loss
- Indexing latency
- Startup time

---

# 92. North-Star Metric

A useful product-level metric:

> **Verified Task Completion Time**

Measure:

```text
Task created
        ↓
Agent starts
        ↓
Implementation
        ↓
Tests
        ↓
Review
        ↓
Verified result
```

The objective is not:

> "How many lines did the agent write?"

It is:

> **How quickly can a developer move from intent to a verified, reviewable change?**

---

# 93. Final Product Positioning

NOIDA should eventually be described as:

> **An agent-native IDE for developers who work with coding agents.**

Not:

> A lightweight VS Code clone.

Not:

> A terminal emulator with an editor.

Not:

> Another AI coding assistant.

The differentiator is the **development environment around the agent**.

Claude and Codex can remain the intelligence.

NOIDA owns:

```text
Context
Workspace
Execution
Isolation
Observation
Verification
Review
Recovery
Orchestration
```

That is the product.

---

# 94. Immediate Next Sprint

If implementing this now, the first sprint should be:

### Sprint 1

**Agent Core**

- [ ] Define `AgentProvider`
- [ ] Define `AgentSession`
- [ ] Define `AgentTask`
- [ ] Define `AgentEvent`
- [ ] Define agent state machine
- [ ] Build event bus
- [ ] Persist agent sessions
- [ ] Build activity timeline

### Sprint 2

**Context**

- [ ] Repository index
- [ ] Tree-sitter symbol extraction
- [ ] LSP integration layer
- [ ] Symbol graph
- [ ] File relevance ranking
- [ ] Context Pack UI

### Sprint 3

**Verification**

- [ ] Test adapter abstraction
- [ ] Test explorer
- [ ] Unified diagnostics
- [ ] Agent test tool
- [ ] Failure evidence extraction

### Sprint 4

**Isolation**

- [ ] Worktree lifecycle
- [ ] Agent → worktree mapping
- [ ] Checkpoints
- [ ] Rollback
- [ ] Agent diff view

### Sprint 5

**Debugger**

- [ ] DAP abstraction
- [ ] Breakpoints
- [ ] Call stack
- [ ] Variables
- [ ] Agent debug tool

### Sprint 6

**Multi-Agent**

- [ ] Handoff
- [ ] Fork
- [ ] Reviewer
- [ ] Parallel agent dashboard
- [ ] Compare worktrees

---

# 95. Definition of "NOIDA 2.0"

NOIDA 2.0 is successful when a developer can say:

> "I don't need to copy context into Claude anymore."

> "I don't need to manually manage agent worktrees."

> "I can see exactly what my agents are doing."

> "I can let one agent implement and another review."

> "When a test fails, the agent can investigate it."

> "I can roll an agent back safely."

> "I can work on a remote machine without losing the workflow."

> "The IDE understands my repository."

And eventually:

> **"The editor isn't where the agent works. The entire IDE is the agent's environment."**

That is the level NOIDA should aim for.
