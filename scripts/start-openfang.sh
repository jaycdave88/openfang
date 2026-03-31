#!/bin/bash
# Startup script for OpenFang with dependency waiting
# Used by LaunchAgent to ensure all dependencies are ready before starting

echo "OpenFang startup: waiting for dependencies..."

# Wait for Ollama
echo "Waiting for Ollama..."
for i in $(seq 1 60); do
  if curl -s http://localhost:11434/api/tags > /dev/null 2>&1; then
    echo "✅ Ollama is ready"
    break
  fi
  echo "  Waiting for Ollama... ($i/60)"
  sleep 5
done

# Wait for Docker services (Conduit)
echo "Waiting for Conduit (Docker)..."
for i in $(seq 1 60); do
  if curl -s http://localhost:6167/_matrix/federation/v1/version > /dev/null 2>&1; then
    echo "✅ Conduit is ready"
    break
  fi
  echo "  Waiting for Conduit... ($i/60)"
  sleep 5
done

# Wait for A2A servers (WorldMonitor on 5174)
echo "Waiting for A2A servers..."
for i in $(seq 1 60); do
  if curl -s -m 3 http://localhost:5174/.well-known/agent.json > /dev/null 2>&1; then
    echo "✅ A2A servers are ready"
    break
  fi
  echo "  Waiting for A2A servers... ($i/60)"
  sleep 5
done

echo "All dependencies ready. Starting OpenFang..."

# Start OpenFang (it loads ~/.openfang/.env and ~/.openfang/secrets.env internally)
exec /Users/momo/intent/workspaces/https-github/repo/repos/openfang/target/release/openfang start

