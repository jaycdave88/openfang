# OpenFang Security Review & Mac Studio Integration Guide

**Date:** 2026-03-25
**Scope:** Security audit for local operation + Mac Studio capabilities with local models + integration feasibility with DeerFlow, TradingAgents, and MicroFish

---

## Part 1: Security Vulnerabilities Assessment

### Overall Verdict: Safe for local use with minor hardening

OpenFang has a **strong defense-in-depth security foundation**: constant-time crypto (`subtle`), parametrized SQLite queries, SSRF protection, command injection prevention, GCRA rate limiting, and comprehensive security headers. The codebase is well-designed for security.

### Vulnerabilities Found

#### 1. Default Unauthenticated API (HIGH)

**File:** `crates/openfang-api/src/middleware.rs`

When `api_key` is empty (the default config), **all API endpoints are public** except `/api/shutdown`. Combined with the Docker-oriented default bind of `0.0.0.0:4200`, any device on your LAN can control your agents, spawn processes, and access your data.

**Fix:** Set `api_key` in `~/.openfang/config.toml` before first run:
```toml
api_key = "your-strong-random-key-here"
```

#### 2. SHA256 Password Hashing Instead of Argon2 (MEDIUM)

**File:** `crates/openfang-api/src/session_auth.rs`

Dashboard authentication uses bare SHA256 (fast, no salt) for password hashing. Argon2 is already in the dependency tree but not used here. This allows rainbow table attacks against the dashboard password.

**Recommendation:** Migrate `hash_password()` to use Argon2 with proper salt generation.

#### 3. Config File Permissions Not Enforced (MEDIUM)

**File:** `crates/openfang-kernel/src/config.rs`

`~/.openfang/config.toml` may contain plaintext API keys and the OFP `shared_secret`. No check verifies that file permissions are restrictive. On macOS, default file permissions are 0644 (world-readable).

**Fix:** `chmod 600 ~/.openfang/config.toml`

**Recommendation:** Add startup warning if config file is world-readable.

#### 4. No File Upload Size Limits (MEDIUM)

**File:** `crates/openfang-api/src/routes.rs`

`/api/agents/{id}/upload` accepts files without an explicit maximum size. Could exhaust disk space if exploited.

**Recommendation:** Add `max_upload_bytes` config field and enforce it.

#### 5. CSP Allows unsafe-inline/unsafe-eval (LOW)

**File:** `crates/openfang-api/src/middleware.rs`

Required for the Alpine.js SPA dashboard, but weakens XSS protection. Low risk for local-only use since the dashboard is served from compiled-in `include_str!()` assets with no user-generated content injection.

#### 6. Environment Variables Not Zeroized After Read (LOW)

API keys read via `std::env::var()` remain in the process environment. The `zeroize` crate is available and documented in SECURITY.md but not universally applied to all secret reads.

### Security Strengths (No Action Needed)

| Control | Implementation |
|---------|---------------|
| **Crypto** | HMAC-SHA256 peer auth with nonce replay protection, constant-time via `subtle` |
| **SSRF Protection** | Blocks all private IPs (10/172/192), cloud metadata endpoints, loopback |
| **Command Injection** | Shell metacharacter blocklist + environment sandboxing + taint tracking |
| **SQL Injection** | All SQLite queries use `rusqlite::params![]` placeholders |
| **Path Traversal** | `safe_resolve_path()` on config includes and file operations |
| **Rate Limiting** | GCRA algorithm, 500 tokens/min per IP, cost-weighted operations |
| **Security Headers** | HSTS (2yr), X-Frame-Options: DENY, CSP, nosniff |
| **Static Assets** | Compiled in via `include_str!()` — no directory traversal possible |
| **Agent Manifests** | Ed25519 signed for identity verification |
| **Audit Trail** | Merkle hash chain with tamper detection at `/api/audit/verify` |
| **WASM Sandbox** | Fuel limits + epoch interruption with watchdog thread |
| **Docker Sandbox** | Capability dropping, container isolation, image allowlisting |

---

## Part 2: Mac Studio Features with Local Models

### What OpenFang Is

An **Agent Operating System** — a local Rust daemon (14 crates, 1744+ tests) that manages multiple AI agents with their own tools, memory, skills, and communication channels. A self-hosted platform for running AI agents entirely on your hardware.

### Local LLM Provider Support

OpenFang supports **35+ LLM providers**. These local providers require **no API key**:

| Provider | Default Base URL | Notes |
|----------|-----------------|-------|
| **Ollama** | `http://localhost:11434` | Auto-discovers all pulled models at $0.00 cost |
| **vLLM** | `http://localhost:8000` | OpenAI-compatible, good for batch inference |
| **LM Studio** | `http://localhost:1234` | GUI-based, easy model management |
| **Lemonade** | `http://localhost:8080` | Lightweight local inference |

All use OpenAI-compatible API drivers (`key_required: false`).

### Quick Start on Mac Studio

```bash
# Terminal 1: Start Ollama with models
ollama serve
ollama pull llama3.2        # General purpose
ollama pull codellama       # Code generation
ollama pull mistral:latest  # Fast reasoning

# Terminal 2: Build and run OpenFang
cd openfang
cargo build --release -p openfang-cli
./target/release/openfang start
```

```toml
# ~/.openfang/config.toml
api_key = "your-secret-key"
api_listen = "127.0.0.1:4200"

[default_model]
provider = "ollama"
model = "llama3.2"
```

### Core Capabilities on Mac Studio

1. **Multi-Agent Management** — 30+ templates (code-reviewer, researcher, planner, debugger, security-auditor, etc.) with per-agent model overrides
2. **Tool Execution with Sandboxing** — Subprocess, Docker, and WASM sandboxes with taint tracking
3. **Memory & Knowledge Graph** — SQLite-backed persistent memory, entity/relationship storage, configurable decay
4. **Budget & Cost Tracking** — Per-agent quotas (hourly/daily/monthly), local models at $0.00
5. **Web Dashboard** — Alpine.js SPA at `http://127.0.0.1:4200`
6. **44+ Channel Adapters** — Telegram, Discord, Slack, Teams, WhatsApp, Signal, Email, SMS, Matrix, IRC, etc.
7. **A2A Protocol** — Discover and communicate with external agent services
8. **OFP Peer Networking** — Connect multiple OpenFang instances across machines
9. **OpenAI-Compatible API** — `/v1/chat/completions` endpoint; any OpenAI client library works
10. **MCP Server Support** — Extend agents with Model Context Protocol tools
11. **Browser Automation** — macOS-specific support via `openfang-hands` crate
12. **Docker Sandbox** — Isolated code execution with capability dropping

### Mac Studio Hardware Utilization

- **Apple Silicon (M-series):** Compiles natively on `aarch64-apple-darwin`. Ollama/vLLM handle Metal GPU acceleration for inference; OpenFang handles orchestration.
- **CPU:** Rust release binary with LTO — efficient multi-agent concurrency via Tokio async runtime
- **Memory:** SQLite for persistence, in-memory caching for active agents. Mac Studio's unified memory benefits both OpenFang and Ollama simultaneously.
- **GPU:** Not directly used by OpenFang. GPU acceleration happens in Ollama (Metal) or vLLM (MPS). This is the correct architecture — OpenFang orchestrates, local inference engines compute.

---

## Part 3: Integration with DeerFlow, TradingAgents, and MicroFish

### Your Vision

A local Mac Studio running OpenFang as the orchestration layer, with:
- **DeerFlow** for deep research and planning
- **TradingAgents** for financial analysis and trading decisions
- **MicroFish** for product spec validation and code output decisions
- **Ollama** powering all LLM inference locally
- **OpenFang** tying everything together, controlling native Mac apps, organizing your life, and coding for you

### Integration Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Mac Studio (Local)                        │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Ollama (Metal GPU Acceleration)          │  │
│  │  llama3.2 | codellama | mistral | deepseek-coder    │  │
│  └──────────────────────────────────────────────────────┘  │
│         ▲              ▲              ▲              ▲      │
│         │              │              │              │      │
│  ┌──────┴──────┐ ┌─────┴─────┐ ┌─────┴─────┐ ┌─────┴───┐ │
│  │  OpenFang   │ │ DeerFlow  │ │ Trading   │ │MicroFish│ │
│  │  (Rust)     │ │ (Python)  │ │ Agents    │ │(Python) │ │
│  │  Port 4200  │ │ Port 8100 │ │ Port 8200 │ │Port 8300│ │
│  └──────┬──────┘ └─────┬─────┘ └─────┬─────┘ └────┬────┘ │
│         │     A2A      │    A2A      │   A2A      │      │
│         ├──────────────►│◄───────────►│◄──────────►│      │
│         │              │              │              │      │
│  ┌──────┴──────────────┴──────────────┴──────────────┴───┐ │
│  │              OpenFang Orchestration Layer              │ │
│  │  Agents | Memory | Budget | Channels | Dashboard      │ │
│  │  Browser Automation | MCP | File System | Scheduler   │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### DeerFlow Integration

**What it is:** ByteDance's open-source SuperAgent harness (LangGraph-based) for deep research, coding, and creative tasks. 37,000+ GitHub stars.

**Integration method:** A2A Protocol (best fit)

- Deploy DeerFlow locally as a Python service
- Wrap its HTTP API with an A2A-compliant adapter (`/.well-known/agent.json`)
- OpenFang discovers and delegates research tasks to it
- Point DeerFlow at your local Ollama instance for zero-cost inference

```toml
# OpenFang config
[[a2a.external_agents]]
name = "deerflow-researcher"
url = "http://localhost:8100"
```

**Use cases:** Deep research for investment analysis, multi-step planning, code generation with execution

### TradingAgents Integration

**What it is:** Multi-agent LLM financial trading framework (TauricResearch/TradingAgents). Simulates trading firm dynamics with 7 specialized roles: Fundamentals Analyst, Sentiment Analyst, News Analyst, Technical Analyst, Researcher, Trader, Risk Manager.

**Integration method:** A2A Protocol or MCP Server

- Deploy as a local Python service
- OpenFang agents submit trading analysis requests
- TradingAgents returns structured investment recommendations
- All inference through local Ollama models

```toml
[[a2a.external_agents]]
name = "trading-analyst"
url = "http://localhost:8200"
```

**Use cases:** Stock analysis, portfolio recommendations, risk assessment, market sentiment analysis

**Important caveat:** Trading decisions based on LLM analysis carry significant financial risk. Use for research/insights, not automated execution.

### MicroFish Integration

**What it is:** Agent framework for validation and simulation tasks.

**Integration method:** A2A Protocol or subprocess tool

- Deploy locally, connect to Ollama for inference
- OpenFang agents invoke MicroFish for spec validation
- Results feed back into planning/coding workflows

```toml
[[a2a.external_agents]]
name = "microfish-validator"
url = "http://localhost:8300"
```

**Use cases:** Product spec validation, code output review, decision quality scoring

### All Three Projects: Shared Setup Pattern

Each Python-based project follows the same integration pattern:

1. **Clone and install** the project locally
2. **Configure LLM** to point at Ollama (`http://localhost:11434`) instead of cloud APIs
3. **Add an A2A adapter** (thin Flask/FastAPI wrapper exposing `/.well-known/agent.json` and `POST /tasks/send`)
4. **Register in OpenFang** via `config.toml` `[[a2a.external_agents]]` entries
5. **Create specialized OpenFang agents** that delegate to the appropriate external service

### What This Gives You on Mac Studio

| Capability | How |
|-----------|-----|
| **Deep research** | OpenFang agent delegates to DeerFlow for multi-step research |
| **Financial analysis** | OpenFang agent delegates to TradingAgents for investment insights |
| **Code validation** | OpenFang agent delegates to MicroFish for spec/code review |
| **Coding assistance** | OpenFang's built-in code-reviewer + debugger agents with codellama |
| **Life organization** | Channel adapters (email, Slack, Telegram) + calendar/task tools |
| **Mac native control** | `openfang-hands` crate with macOS browser automation + `openfang-desktop` |
| **Privacy** | Everything runs locally — no data leaves your Mac Studio |
| **Zero API cost** | All inference through Ollama at $0.00/token |

---

## Part 4: Recommended Hardening Steps

Before running this stack on your Mac Studio:

1. **Set API key:** `api_key = "strong-random-string"` in config.toml
2. **Bind localhost only:** `api_listen = "127.0.0.1:4200"`
3. **Restrict config permissions:** `chmod 600 ~/.openfang/config.toml`
4. **Set OFP shared_secret** if networking between machines
5. **Enable Docker sandbox** for untrusted tool execution
6. **Set budget limits** if mixing local + cloud models
7. **Firewall:** Ensure ports 8100/8200/8300 (DeerFlow/TradingAgents/MicroFish) are localhost-only
8. **Monitor:** Use OpenFang's Merkle audit trail (`/api/audit/verify`) to track all agent actions

---

## Summary

OpenFang is **production-quality for local use** with the hardening steps above. It's architecturally well-suited to be the orchestration hub on a Mac Studio, with DeerFlow, TradingAgents, and MicroFish connected as external A2A agents, all powered by local Ollama models. The security foundation is solid — the main risks are operational (default-open auth, config permissions) rather than code-level vulnerabilities.
