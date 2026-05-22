# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

A teaching project that dissects Claude Code's architecture to demonstrate how to build AI agent harnesses. The core thesis: agency comes from model training, not code orchestration. This repo builds the "vehicle" (harness), not the "driver" (model).

The project has two parts: **Rust agent binaries** that progressively implement harness mechanisms, and a **Next.js web app** that visualizes and documents them.

## Commands

### Rust agents
```bash
cd agents-rs

# Build all binaries
cargo build

# Run an agent (requires ANTHROPIC_API_KEY and MODEL_ID in .env)
cargo run --bin s01_agent_loop
cargo run --bin s02_tool_use
# ... etc for s03 through s12

# Run the full combined harness
cargo run --bin s_full

# Type check only (fast)
cargo check

# Run tests
cargo test
```

### Python agents (legacy, still available)
```bash
python agents/s01_agent_loop.py
python -m pytest tests/test_agents_smoke.py -q
```

### Web app
```bash
cd web
npm ci                    # install dependencies
npm run extract           # extract content from agents-rs/ and docs/ into src/data/generated/
npm run dev               # dev server (auto-runs extract first)
npm run build             # production build (auto-runs extract first)
npx tsc --noEmit          # type check only
```

The `extract` script (`web/scripts/extract-content.ts`) reads `agents-rs/s*/src/main.rs` and `docs/{en,zh,ja}/*.md`, then writes `versions.json` and `docs.json` into `web/src/data/generated/`. It runs automatically before dev/build. If `agents-rs/` is missing (e.g. Vercel build), it falls back to pre-committed generated data.

## Architecture

### Rust workspace (agents-rs/)

A Cargo workspace with 14 crates: one shared `harness` library and 13 binary crates.

**harness/** — shared library crate providing:
- `config.rs` — env/dotenv loading (ANTHROPIC_API_KEY, MODEL_ID, ANTHROPIC_BASE_URL)
- `client.rs` — reqwest-based Anthropic API client with ContentBlock, ToolDef, message helpers
- `tools.rs` — run_bash, run_read, run_write, run_edit with safety checks
- `todo.rs` — TodoManager (s03)
- `skills.rs` — SkillLoader with YAML frontmatter (s05)
- `compact.rs` — micro_compact, auto_compact, estimate_tokens (s06)
- `tasks.rs` — TaskManager with JSON file persistence and dependency graph (s07)
- `messaging.rs` — MessageBus with JSONL inboxes (s09)
- `protocols.rs` — ProtocolTracker for shutdown/plan approval (s10)

**Binary crates (s01–s12 + s_full)** — each progressively adds one mechanism:

| Layer | Scripts | What's added |
|-------|---------|-------------|
| **Tools & Execution** | `s01` → `s02` | Agent loop (`while stop_reason == "tool_use"`), then tool dispatch map |
| **Planning & Coordination** | `s03`, `s04`, `s05`, `s07` | TodoWrite planning, subagent context isolation, on-demand skill loading, task graph with dependencies |
| **Memory** | `s06` | Three-layer context compression (micro/auto/archival) |
| **Concurrency** | `s08` | Background task threads + notification queue |
| **Collaboration** | `s09` → `s12` | Team mailboxes, request-response protocols, autonomous task claiming, worktree directory isolation |

`s_full` combines all mechanisms into one complete harness.

Key patterns across all binaries:
- All use `reqwest`-based AnthropicClient with optional `ANTHROPIC_BASE_URL` override and `MODEL_ID` from env
- All share the same tool safety model: dangerous command blocklist, 120s timeout, 50000-char output truncation, path traversal protection
- Subagents (`s04+`) spawn with fresh `messages[]` and filtered tools (no recursive `task` tool)
- Team agents (`s09+`) communicate through `.team/inbox/` JSONL mailbox files
- All agent loops are async (`tokio::spawn` for background tasks, `Arc<Mutex<...>>` for shared state)

### Web app (web/)

Next.js 16 + React 19 + Tailwind 4. Content pipeline: `agents-rs/s*/src/main.rs` → `extract-content.ts` → `src/data/generated/{versions.json,docs.json}` → React components.

Key directories:
- `src/components/visualizations/` — one interactive visualization per scenario (s01–s12)
- `src/components/simulator/` — interactive agent loop simulator with `useSimulator` hook
- `src/components/architecture/` — architecture diagrams, execution flow, design decisions
- `src/components/diff/` — code diff viewer between scenario versions
- `src/data/annotations/` and `src/data/scenarios/` — structured data for visualizations
- `src/hooks/useSteppedVisualization.ts` — stepped animation controller
- `src/lib/constants.ts` — `VERSION_ORDER`, `VERSION_META` (titles, layers, insights), `LAYERS` — the single source of truth for scenario metadata
- `src/lib/i18n.tsx` / `src/lib/i18n-server.ts` — client/server i18n

Scenario version metadata is defined in `web/src/lib/constants.ts` (`VERSION_META`). If you add or rename an agent crate, update both the directory name convention (must match `s\d+[a-c]?_*`) and `VERSION_META`.

### Multi-provider support

Agent scripts support any Anthropic-compatible API via `ANTHROPIC_BASE_URL` + `MODEL_ID` env vars. See `.env.example` for provider-specific configurations (MiniMax, GLM, Kimi, DeepSeek).

### Team collaboration system

`.team/config.json` defines team members. `.team/inbox/{name}.jsonl` files are message queues for inter-agent communication. This is used by `s09`–`s12` scripts.

### Skills system

`skills/*/SKILL.md` defines loadable skill prompts (code-review, agent-builder, mcp-builder, pdf). Loaded on-demand by `s05`'s `SkillLoader`.

### Docs

`docs/{en,zh,ja}/` — three parallel documentation trees, one markdown file per scenario. Filenames follow `s{NN}-{slug}.md` convention.

## CI

Two GitHub Actions workflows:
- `.github/workflows/ci.yml` — web build (npm ci → tsc --noEmit → next build)
- `.github/workflows/test.yml` — Python smoke tests (pytest on test_agents_smoke.py) + web build

Python 3.11, Node 20, Rust latest stable.
