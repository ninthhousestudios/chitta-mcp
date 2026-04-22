#!/usr/bin/env bash
set -euo pipefail

cd /workspace/agent-memory-benchmark
set -a && . .env && set +a

uv run python -c "
import traceback
from memory_bench.memory.chitta_mcp import ChittaMCPMemoryProvider
from memory_bench.dataset import get_dataset

ds = get_dataset('personamem')
docs = ds.load_documents('32k')
print(f'Loaded {len(docs)} documents')

p = ChittaMCPMemoryProvider()
p.initialize()
print('Provider initialized')

for i, d in enumerate(docs):
    try:
        profile = p._profile(d.user_id)
        tags = []
        if d.timestamp:
            tags.append(f'date:{d.timestamp}')
        prefix = f'[Date: {d.timestamp}]' if d.timestamp else None
        metadata = {'doc_id': d.id}
        messages = p._extract_messages(d)
        content_len = len(d.content)
        msg_count = len(messages) if messages else 0
        print(f'[{i+1}/{len(docs)}] id={d.id} content_len={content_len} messages={msg_count} profile={profile}')

        if messages:
            chunks = p._chunk_messages(messages)
            for ci, chunk in enumerate(chunks):
                text = f'{prefix}\n{chunk}' if prefix else chunk
                key = f'{d.id}_msg_{ci}'
                p.mcp.call_tool('store_memory', {
                    'profile': profile,
                    'content': text,
                    'idempotency_key': key,
                    'source': 'amb',
                    'tags': tags or None,
                    'metadata': metadata,
                })
            print(f'  stored {len(chunks)} message chunks')
        else:
            p._store(profile, d.content, tags, metadata, prefix)
            print(f'  stored as text chunks')
    except Exception as e:
        print(f'  FAILED: {e}')
        traceback.print_exc()
        break

print('Done')
p.cleanup()
"
