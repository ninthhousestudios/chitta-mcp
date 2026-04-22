#!/usr/bin/env bash
set -euo pipefail

cd /workspace/agent-memory-benchmark
set -a && . .env && set +a

uv run python << 'PYEOF'
import json
from memory_bench.dataset import get_dataset

ds = get_dataset('personamem')
docs = ds.load_documents('32k')
d = docs[0]

messages = d.messages
if not messages and d.content.strip().startswith('['):
    messages = json.loads(d.content)

print(f'Messages: {len(messages)}')

def fmt(msgs):
    parts = []
    for m in msgs:
        parts.append(m.get('role', '?') + ': ' + m.get('content', ''))
    return '\n'.join(parts)

turns_per_chunk = 4
overlap_turns = 1
chunks = []
start = 0
iteration = 0
while start < len(messages):
    iteration += 1
    if iteration > 100:
        print('INFINITE LOOP DETECTED')
        break
    end = min(start + turns_per_chunk, len(messages))
    chunk_msgs = messages[start:end]
    text = fmt(chunk_msgs)
    chunks.append(text)
    print(f'  iter {iteration}: start={start} end={end} chunk_len={len(text)}')
    start = end - overlap_turns
    if start >= len(messages) or start <= (end - turns_per_chunk):
        print(f'  breaking: start={start} >= len={len(messages)} or start <= {end - turns_per_chunk}')
        break

print(f'\nTotal chunks: {len(chunks)}')
for i, c in enumerate(chunks):
    print(f'  chunk {i}: {len(c)} chars (~{len(c)//4} tokens)')
PYEOF
