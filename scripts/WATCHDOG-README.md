# MOMO Health Watchdog

The health watchdog monitors all MOMO services every 60 seconds and automatically restarts failed services.

## Monitored Services

1. **Ollama** - LLM inference server (port 11434)
2. **OpenFang** - Agent operating system (port 4200)
3. **Bridge** - Matrix-iMessage bridge (mautrix-imessage process)
4. **Wsproxy** - WebSocket proxy (wsproxy process)
5. **Conduit** - Matrix homeserver (port 6167)
6. **Ollama Generation Check** - Verifies Ollama can actually generate responses (not just running but frozen)

## Installation

### 1. Copy the LaunchAgent plist
```bash
cp repos/openfang/scripts/com.momo.watchdog.plist ~/Library/LaunchAgents/
```

### 2. Load the LaunchAgent
```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.momo.watchdog.plist
```

### 3. Verify it's running
```bash
launchctl list | grep com.momo.watchdog
```

## Manual Testing

### Test the script manually
```bash
bash repos/openfang/scripts/watchdog.sh
cat ~/.openfang/watchdog.log
```

### Test auto-restart functionality
```bash
# Kill Ollama
pkill -9 -f ollama

# Wait for watchdog cycle (60 seconds)
sleep 70

# Verify Ollama is back
curl -s http://localhost:11434/api/tags
cat ~/.openfang/watchdog.log
```

## Logs

- **Watchdog events**: `~/.openfang/watchdog.log`
- **Standard output**: `~/.openfang/watchdog-stdout.log`
- **Standard error**: `~/.openfang/watchdog-stderr.log`

## Uninstall

```bash
launchctl bootout gui/$(id -u)/com.momo.watchdog
rm ~/Library/LaunchAgents/com.momo.watchdog.plist
```

## How It Works

Every 60 seconds, the watchdog:

1. Checks if each service is responding/running
2. If a service fails, logs the failure
3. Attempts to restart via `launchctl kickstart`
4. Waits 5 seconds and re-checks
5. Logs recovery status (RECOVERED or STILL DOWN)

For Ollama frozen detection:
- Sends a test chat request with 15-second timeout
- If response is empty/null, force-kills Ollama and restarts it manually
- Reloads the model with keep_alive=-1 to prevent eviction

