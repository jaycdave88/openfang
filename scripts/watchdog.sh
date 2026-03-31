#!/bin/bash
# MOMO Health Watchdog - runs every 60 seconds
# Monitors all services and auto-restarts failures
# CRITICAL: If a service is down, RESTART it (never skip)

LOG=~/.openfang/watchdog.log
TS=$(date '+%Y-%m-%d %H:%M:%S')

# Ensure log directory exists
mkdir -p ~/.openfang

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

# 2. OpenFang
if ! curl -s -m 5 http://127.0.0.1:4200/api/health > /dev/null 2>&1; then
    echo "$TS: RESTARTING OpenFang (not healthy)" >> $LOG
    pkill -9 -f 'openfang start'
    sleep 3
    cd /Users/momo/intent/workspaces/https-github/repo
    source /Users/momo/.cargo/env
    export DISCORD_BOT_TOKEN=$(grep DISCORD_BOT_TOKEN /Users/momo/intent/workspaces/https-github/repo/.env | cut -d= -f2)
    export MEM0_API_TOKEN=v9WNtaNzYepldseeMANP3SJ2oUFmBIcA
    export NEO4J_USER=neo4j
    export NEO4J_PASSWORD=RA3MeCEx7mqmaVptrwRocCQNst5vLhC5
    export SSHBOX_USER=agent
    export SSHBOX_PASSWORD=0xoydw9yw8jUWCJzJdZ2RBVSYsMV4EAQ
    export MATRIX_ACCESS_TOKEN=AcXsn88QaYQy4YAqMdufwCK6qu1ZIZfo
    export MATRIX_APPSERVICE_TOKEN=MqlKKaXVqjGUP89Cd2pMt7HdmUnj7zCvv2aCagpLZxhwGhgCJvMLrYTSm68BmAHU
    nohup repos/openfang/target/release/openfang start >> ~/.openfang/openfang.log 2>&1 &
    echo "$TS: OpenFang restarted" >> $LOG
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

echo "$TS: Watchdog check complete" >> $LOG

