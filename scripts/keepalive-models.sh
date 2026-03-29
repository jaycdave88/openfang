#!/bin/bash
# Keep Ollama models loaded permanently
# Run via cron every 6 hours to prevent eviction even if Ollama restarts

for model in "qwen2.5-coder:32b-instruct-q4_K_M" "qwen3.5:9b" "llama3.1:70b-instruct-q4_K_M"; do
    curl -s -X POST http://localhost:11434/api/generate \
        -d "{\"model\":\"$model\",\"prompt\":\"\",\"keep_alive\":-1}" > /dev/null 2>&1
    echo "$(date): Pinged $model"
done

