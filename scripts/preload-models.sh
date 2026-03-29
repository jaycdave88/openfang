#!/bin/bash
# Pre-load Ollama models for OpenFang
# Run after Ollama starts to ensure fallback models are ready
#
# This script ensures that the primary 32B coder model and fallback models
# are loaded into VRAM simultaneously, preventing "404 model not found" errors
# when OpenFang needs to use fallback models.
#
# The Mac Studio M3 Ultra has 256GB unified memory, which is plenty for:
# - qwen2.5-coder:32b-instruct-q4_K_M (~31GB VRAM) [PRIMARY]
# - qwen3.5:9b (~10GB VRAM) [FALLBACK 1]
# - llama3.1:70b-instruct-q4_K_M (~102GB VRAM) [FALLBACK 2]
# - nomic-embed-text (~0.6GB VRAM)
# Total: ~144GB / 256GB

echo 'Pre-loading Ollama models...'

# Wait for Ollama to be ready
echo 'Waiting for Ollama to start...'
for i in {1..30}; do
    if curl -s http://localhost:11434/api/tags > /dev/null 2>&1; then
        echo "✅ Ollama is ready"
        break
    fi
    sleep 2
done

# Load primary model (32B coder) with infinite keep_alive (-1 = never unload)
echo 'Loading primary model: qwen2.5-coder:32b-instruct-q4_K_M...'
curl -s -X POST http://localhost:11434/api/generate \
    -d '{"model":"qwen2.5-coder:32b-instruct-q4_K_M","prompt":"","keep_alive":-1}' > /dev/null 2>&1

# Wait a bit for the first model to start loading
sleep 5

# Load fallback model (9B) with infinite keep_alive
echo 'Loading fallback model: qwen3.5:9b...'
curl -s -X POST http://localhost:11434/api/generate \
    -d '{"model":"qwen3.5:9b","prompt":"","keep_alive":-1}' > /dev/null 2>&1

sleep 5

# Load fallback model (70B) with infinite keep_alive
echo 'Loading fallback model: llama3.1:70b-instruct-q4_K_M...'
curl -s -X POST http://localhost:11434/api/generate \
    -d '{"model":"llama3.1:70b-instruct-q4_K_M","prompt":"","keep_alive":-1}' > /dev/null 2>&1

# Wait for models to finish loading
sleep 10

echo ''
echo 'Models pre-loaded:'
curl -s http://localhost:11434/api/ps | python3 -c "
import sys, json
d = json.load(sys.stdin)
total_vram = 0
for m in d.get('models', []):
    name = m.get('name', '?')
    size = m.get('size_vram', 0) / 1e9
    total_vram += size
    print(f'  ✅ {name} ({size:.1f}GB VRAM)')
print(f'\nTotal VRAM: {total_vram:.1f}GB / 256GB')
"

