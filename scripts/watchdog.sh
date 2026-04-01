#!/bin/bash
# MOMO Health Watchdog - runs every 60 seconds
# Monitors all services and auto-restarts failures
# CRITICAL: If a service is down, RESTART it (never skip)

LOG=~/.openfang/watchdog.log
TS=$(date '+%Y-%m-%d %H:%M:%S')

# Ensure log directory exists
mkdir -p ~/.openfang

log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S'): $1" >> $LOG
}

# Reusable OpenFang restart function
restart_openfang() {
    log "Stopping OpenFang..."
    pkill -9 -f 'openfang start' 2>/dev/null
    sleep 3

    cd /Users/momo/intent/workspaces/https-github/repo

    # Load env vars from .env
    if [ -f /Users/momo/intent/workspaces/https-github/repo/.env ]; then
        set -a
        source /Users/momo/intent/workspaces/https-github/repo/.env
        set +a
    fi

    # Export required tokens
    source /Users/momo/.cargo/env
    export DISCORD_BOT_TOKEN=$(grep DISCORD_BOT_TOKEN /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export MEM0_API_TOKEN=$(grep MEM0_API_TOKEN /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export NEO4J_USER=$(grep '^NEO4J_USER=' /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export NEO4J_PASSWORD=$(grep '^NEO4J_PASSWORD=' /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export SSHBOX_USER=$(grep '^SSHBOX_USER=' /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export SSHBOX_PASSWORD=$(grep '^SSHBOX_PASSWORD=' /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export MATRIX_ACCESS_TOKEN=ZSuy97KcT8pJDsXLfytwxEpvkEI8DYep
    export MATRIX_APPSERVICE_TOKEN=MqlKKaXVqjGUP89Cd2pMt7HdmUnj7zCvv2aCagpLZxhwGhgCJvMLrYTSm68BmAHU

    nohup repos/openfang/target/release/openfang start >> ~/.openfang/openfang.log 2>&1 &
    log "OpenFang starting - waiting 25s for boot..."
    sleep 25
    log "OpenFang restarted"
}

# 1. Ollama
if ! pgrep -f "ollama serve" > /dev/null 2>&1; then
    echo "$TS: STARTING Ollama (was down)" >> $LOG
    OLLAMA_NUM_PARALLEL=1 OLLAMA_FLASH_ATTENTION=1 OLLAMA_MAX_LOADED_MODELS=3 OLLAMA_KEEP_ALIVE=-1 /opt/homebrew/bin/ollama serve > /tmp/ollama-serve.log 2>&1 &
    sleep 15
    # Load models
    curl -s -X POST http://localhost:11434/api/generate -d '{"model":"qwen3:30b-a3b","prompt":"","keep_alive":-1}' > /dev/null 2>&1
    curl -s -X POST http://localhost:11434/api/generate -d '{"model":"nomic-embed-text","prompt":"","keep_alive":-1}' > /dev/null 2>&1
    echo "$TS: Ollama started + models loaded" >> $LOG
elif ! curl -s -m 5 http://localhost:11434/api/tags > /dev/null 2>&1; then
    echo "$TS: RESTARTING Ollama (running but not responding)" >> $LOG
    pkill -9 -f "ollama serve"
    sleep 5
    OLLAMA_NUM_PARALLEL=1 OLLAMA_FLASH_ATTENTION=1 OLLAMA_MAX_LOADED_MODELS=3 OLLAMA_KEEP_ALIVE=-1 /opt/homebrew/bin/ollama serve > /tmp/ollama-serve.log 2>&1 &
    sleep 15
    curl -s -X POST http://localhost:11434/api/generate -d '{"model":"qwen3:30b-a3b","prompt":"","keep_alive":-1}' > /dev/null 2>&1
    curl -s -X POST http://localhost:11434/api/generate -d '{"model":"nomic-embed-text","prompt":"","keep_alive":-1}' > /dev/null 2>&1
    echo "$TS: Ollama hard-restarted + models loaded" >> $LOG
fi

# 2. OpenFang - Smart Health Checks
OPENFANG_NEEDS_RESTART=false

# A. Process check
if ! pgrep -f 'openfang start' > /dev/null 2>&1; then
    log "OpenFang process not running - restarting"
    OPENFANG_NEEDS_RESTART=true
fi

# B. API health check (only if process is running)
if [ "$OPENFANG_NEEDS_RESTART" = false ]; then
    HEALTH=$(curl -s -m 5 http://127.0.0.1:4200/api/health 2>/dev/null)
    if ! echo "$HEALTH" | grep -q "ok"; then
        log "OpenFang process alive but API not responding - restarting"
        OPENFANG_NEEDS_RESTART=true
    fi
fi

# C. Crash count check
if [ "$OPENFANG_NEEDS_RESTART" = false ]; then
    CRASH_COUNT=$(grep -c 'Crashed for recovery' <(tail -100 ~/.openfang/openfang.log) 2>/dev/null || echo '0')
    if [ "$CRASH_COUNT" -gt 10 ]; then
        log "OpenFang has $CRASH_COUNT recent crashes - restarting fresh"
        OPENFANG_NEEDS_RESTART=true
    fi
fi

# D. Stale Matrix check (8am-10pm only)
if [ "$OPENFANG_NEEDS_RESTART" = false ]; then
    HOUR=$(date +%H)
    if [ "$HOUR" -ge 8 ] && [ "$HOUR" -le 22 ]; then
        LAST_MATRIX=$(grep -i 'matrix.*message\|channel.*received\|matrix.*sync' ~/.openfang/openfang.log 2>/dev/null | tail -1 | grep -oE '[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}' | tail -1)
        if [ -n "$LAST_MATRIX" ]; then
            LAST_EPOCH=$(date -j -f '%Y-%m-%dT%H:%M' "$LAST_MATRIX" '+%s' 2>/dev/null || echo '0')
            NOW_EPOCH=$(date '+%s')
            STALE=$(( NOW_EPOCH - LAST_EPOCH ))
            if [ "$STALE" -gt 7200 ]; then
                log "OpenFang Matrix stale (${STALE}s) - restarting"
                OPENFANG_NEEDS_RESTART=true
            fi
        fi
    fi
fi

# Restart if any check failed
if [ "$OPENFANG_NEEDS_RESTART" = true ]; then
    restart_openfang
fi

# 3. Bridge (check for actual binary, not monitor scripts)
if ! pgrep -x "mautrix-imessage" > /dev/null 2>&1; then
    echo "$TS: RESTARTING bridge" >> $LOG
    bash /Users/momo/mautrix-imessage/restart-bridge.sh &
    echo "$TS: Bridge restart initiated" >> $LOG
fi

# 4. Wsproxy
if ! pgrep -f "mautrix-wsproxy" > /dev/null 2>&1; then
    echo "$TS: RESTARTING wsproxy" >> $LOG
    cd /Users/momo/mautrix-wsproxy
    nohup ./mautrix-wsproxy -config config.yaml > /tmp/wsproxy.log 2>&1 &
    echo "$TS: Wsproxy restarted" >> $LOG
fi

# 5. A2A Agents — auto-restart if not responding
A2A_DIR="/Users/momo/intent/workspaces/https-github/repo/repos"

restart_a2a_agent() {
    local name="$1" dir="$2" port="$3" venv_path="${4:-.venv}"
    log "Restarting A2A agent: $name (port $port)"
    # Kill any existing process on the port
    lsof -ti:$port | xargs kill -9 2>/dev/null
    sleep 2
    cd "$A2A_DIR/$dir"
    if [ -d "$venv_path" ]; then
        source "$venv_path/bin/activate"
        nohup python a2a_server.py >> ~/.openfang/a2a-$name.log 2>&1 &
        deactivate 2>/dev/null || true
    fi
    log "A2A agent $name restarted on port $port"
}

# DeerFlow (port 2024) — venv is in backend/.venv
if ! curl -s -m 5 http://localhost:2024/.well-known/agent.json > /dev/null 2>&1; then
    restart_a2a_agent "deer-flow" "deer-flow" 2024 "backend/.venv"
fi

# TradingAgents (port 8100)
if ! curl -s -m 5 http://localhost:8100/.well-known/agent.json > /dev/null 2>&1; then
    restart_a2a_agent "trading-agents" "TradingAgents" 8100
fi

# MicroFish (port 5001)
if ! curl -s -m 5 http://localhost:5001/.well-known/agent.json > /dev/null 2>&1; then
    restart_a2a_agent "microfish" "MicroFish-En" 5001
fi

# WorldMonitor (port 5174)
if ! curl -s -m 5 http://localhost:5174/.well-known/agent.json > /dev/null 2>&1; then
    restart_a2a_agent "worldmonitor" "worldmonitor" 5174
fi

# AgenticTrading (port 8200)
if ! curl -s -m 5 http://localhost:8200/.well-known/agent.json > /dev/null 2>&1; then
    restart_a2a_agent "agentic-trading" "AgenticTrading" 8200
fi

echo "$TS: Watchdog check complete" >> $LOG

