#!/usr/bin/env bash
set -euo pipefail

cd /workspace/agent-memory-benchmark
set -a && . .env && set +a

uv run python -c "
from memory_bench.memory.chitta_mcp import ChittaMCPMemoryProvider
p = ChittaMCPMemoryProvider()
p.initialize()
print('initialized')
p.mcp.call_tool('store_memory', {
    'profile': 'test',
    'content': 'hello world test memory',
    'idempotency_key': 'test-1',
    'source': 'amb',
})
print('stored ok')
"
