#!/usr/bin/env python3
"""Retrieval-only evaluation for chitta-rs benchmarks.

Runs PersonaMem/BEAM/LifeBench ingestion + retrieval without LLM calls.
Measures retrieval quality (recall@k, MRR, gold-term overlap) at zero API cost.

Usage (from AMB directory):
    uv run python /workspace/chitta/bench/retrieval-eval.py \
        --dataset personamem --split 32k

    uv run python /workspace/chitta/bench/retrieval-eval.py \
        --dataset beam --split 100k --query-limit 2

    uv run python /workspace/chitta/bench/retrieval-eval.py \
        --dataset lifebench --split en
"""

import argparse
import json
import os
import re
import sys
import time
from collections import defaultdict
from pathlib import Path


def _find_amb_dir():
    if d := os.environ.get("AMB_DIR"):
        return Path(d)
    for candidate in [
        Path(__file__).resolve().parents[1].parent / "agent-memory-benchmark",
        Path("/workspace/agent-memory-benchmark"),
        Path.home() / "soft" / "agent-memory-benchmark",
    ]:
        if (candidate / "src" / "memory_bench").is_dir():
            return candidate
    return None


def _load_dotenv(path):
    if not path.exists():
        return
    for line in path.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, _, val = line.partition("=")
            os.environ.setdefault(key.strip(), val.strip())


_STOPWORDS = frozenset(
    "the a an is are was were be been being have has had do does did will would "
    "shall should may might must can could to of in for on with at by from as "
    "into through during before after between and but or not so it its he she "
    "they them his her their what which who how all each both more most other "
    "some no only than too very just that this these those such there here about "
    "also been said would could should".split()
)


def _tokenize(text):
    return {
        w
        for w in re.findall(r"\b[a-z][a-z0-9]+\b", text.lower())
        if w not in _STOPWORDS
    }


def gold_term_overlap(gold_answers, context):
    if not gold_answers or not context:
        return 0.0
    gold_terms = _tokenize(gold_answers[0])
    if not gold_terms:
        return 0.0
    context_terms = _tokenize(context)
    return len(gold_terms & context_terms) / len(gold_terms)


def evaluate_query(query, provider):
    profile = provider._profile(query.user_id)
    retrieval_query = query.meta.get("retrieval_query") or query.query

    t0 = time.perf_counter()
    search_result = provider.mcp.call_tool(
        "search_memories",
        {
            "profile": profile,
            "query": retrieval_query,
            "k": provider.k,
            "include_content": True,
        },
    )
    retrieve_ms = (time.perf_counter() - t0) * 1000

    hits = search_result.get("results", [])

    retrieved_doc_ids = []
    similarities = []
    context_parts = []
    for hit in hits:
        similarities.append(hit.get("similarity", 0.0))
        content = hit.get("content") or hit.get("snippet", "")
        doc_id = (hit.get("metadata") or {}).get("doc_id")
        retrieved_doc_ids.append(doc_id)
        context_parts.append(content)

    context = "\n\n".join(context_parts)
    context_tokens = len(context) // 4

    gold_set = set(query.gold_ids)

    def _matches_gold(doc_id):
        if doc_id is None:
            return False
        if doc_id in gold_set:
            return True
        # Chunk IDs like "1_s2_4" should match gold ID "1"
        # but "1_..." must not match "10"
        return any(doc_id == g or doc_id.startswith(g + "_") for g in gold_set)

    matched_gold = {
        g
        for g in gold_set
        if any(
            d is not None and (d == g or d.startswith(g + "_"))
            for d in retrieved_doc_ids
        )
    }

    recall = len(matched_gold) / len(gold_set) if gold_set else 0.0
    n_retrieved = len(retrieved_doc_ids)
    precision = (
        sum(1 for d in retrieved_doc_ids if _matches_gold(d)) / n_retrieved
        if n_retrieved
        else 0.0
    )
    hit = 1.0 if matched_gold else 0.0

    mrr = 0.0
    for rank, doc_id in enumerate(retrieved_doc_ids, 1):
        if _matches_gold(doc_id):
            mrr = 1.0 / rank
            break

    overlap = gold_term_overlap(query.gold_answers, context)

    return {
        "query_id": query.id,
        "query": query.query[:200],
        "gold_ids": query.gold_ids,
        "retrieved_doc_ids": retrieved_doc_ids,
        "similarities": [round(s, 4) for s in similarities],
        "recall": round(recall, 4),
        "precision": round(precision, 4),
        "hit": hit,
        "mrr": round(mrr, 4),
        "gold_term_overlap": round(overlap, 4),
        "retrieve_time_ms": round(retrieve_ms, 1),
        "context_tokens": context_tokens,
        "num_hits": len(hits),
        "meta": {k: v for k, v in query.meta.items() if not k.startswith("_")},
    }


def get_chitta_config():
    return {
        "k": int(os.environ.get("CHITTA_K", "20")),
        "chunk_size": int(os.environ.get("CHITTA_CHUNK_SIZE", "512")),
        "chunk_overlap": int(os.environ.get("CHITTA_CHUNK_OVERLAP", "64")),
        "turns_per_chunk": int(os.environ.get("CHITTA_TURNS_PER_CHUNK", "4")),
        "overlap_turns": int(os.environ.get("CHITTA_OVERLAP_TURNS", "1")),
        "recency_weight": float(os.environ.get("CHITTA_RECENCY_WEIGHT", "0")),
        "recency_half_life_days": float(
            os.environ.get("CHITTA_RECENCY_HALF_LIFE_DAYS", "30")
        ),
        "rrf_fts": os.environ.get("CHITTA_RRF_FTS", "false").lower() == "true",
        "rrf_sparse": os.environ.get("CHITTA_RRF_SPARSE", "false").lower() == "true",
        "rrf_k": int(os.environ.get("CHITTA_RRF_K", "60")),
        "rrf_candidates": int(os.environ.get("CHITTA_RRF_CANDIDATES", "5")),
    }


def compute_summary(args, results, config, ingestion_ms, ingested_docs):
    n = len(results)
    if n == 0:
        return {
            "dataset": args.dataset,
            "split": args.split,
            "eval_type": "retrieval-only",
            "total_queries": 0,
            "results": [],
        }

    def mean(key):
        vals = [r[key] for r in results if r.get(key) is not None]
        return round(sum(vals) / len(vals), 4) if vals else 0.0

    metrics = {
        "mean_recall": mean("recall"),
        "mean_precision": mean("precision"),
        "hit_rate": mean("hit"),
        "mean_mrr": mean("mrr"),
        "mean_gold_term_overlap": mean("gold_term_overlap"),
        "mean_retrieve_time_ms": round(mean("retrieve_time_ms"), 1),
        "mean_context_tokens": round(mean("context_tokens")),
    }

    _CAT_KEYS = {
        "personamem": "question_type",
        "beam": "question_category",
        "lifebench": "category",
    }
    cat_key = _CAT_KEYS.get(args.dataset, "category")
    by_category = defaultdict(list)
    for r in results:
        cat = (r.get("meta") or {}).get(cat_key, "unknown")
        by_category[cat].append(r)

    category_metrics = {}
    for cat, cat_results in sorted(by_category.items()):
        cn = len(cat_results)
        category_metrics[cat] = {
            "count": cn,
            "mean_recall": round(sum(r["recall"] for r in cat_results) / cn, 4),
            "hit_rate": round(sum(r["hit"] for r in cat_results) / cn, 4),
            "mean_mrr": round(sum(r["mrr"] for r in cat_results) / cn, 4),
            "mean_gold_term_overlap": round(
                sum(r["gold_term_overlap"] for r in cat_results) / cn, 4
            ),
        }

    return {
        "dataset": args.dataset,
        "split": args.split,
        "memory_provider": "chitta-mcp",
        "run_name": args.name or "chitta-mcp",
        "eval_type": "retrieval-only",
        "total_queries": n,
        "ingestion_time_ms": round(ingestion_ms, 1),
        "ingested_docs": ingested_docs,
        "config": config,
        "metrics": metrics,
        "category_metrics": category_metrics,
        "results": results,
    }


def save_results(summary, args):
    run_name = args.name or "chitta-mcp"
    path = (
        Path(args.output_dir)
        / summary["dataset"]
        / run_name
        / "retrieval"
        / f"{args.split}.json"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(summary, indent=2))
    print(f"\nSaved → {path}")


def print_summary(summary):
    m = summary["metrics"]
    cfg = summary["config"]
    print(f"\n{'=' * 60}")
    print(f"  {summary['dataset']}/{summary['split']} — retrieval-only eval")
    legs = ["dense"]
    if cfg.get("rrf_fts"):
        legs.append("fts")
    if cfg.get("rrf_sparse"):
        legs.append("sparse")
    legs_str = "+".join(legs)
    rrf_info = f" | rrf_k={cfg['rrf_k']} cand={cfg['rrf_candidates']}" if len(legs) > 1 else ""
    print(f"  {summary['total_queries']} queries | k={cfg['k']} | legs={legs_str}{rrf_info}")
    print(f"{'=' * 60}")
    print(f"  Hit rate:            {m['hit_rate']:.1%}")
    print(f"  Mean recall@k:       {m['mean_recall']:.1%}")
    print(f"  Mean precision@k:    {m['mean_precision']:.1%}")
    print(f"  Mean MRR:            {m['mean_mrr']:.4f}")
    print(f"  Gold term overlap:   {m['mean_gold_term_overlap']:.1%}")
    print(f"  Mean retrieve time:  {m['mean_retrieve_time_ms']:.1f}ms")
    print(f"  Mean context tokens: {m['mean_context_tokens']:.0f}")

    if summary.get("category_metrics"):
        print(f"\n  Per-category breakdown:")
        for cat, cm in sorted(summary["category_metrics"].items()):
            print(
                f"    {cat:30s}  n={cm['count']:3d}"
                f"  hit={cm['hit_rate']:.1%}"
                f"  recall={cm['mean_recall']:.1%}"
                f"  mrr={cm['mean_mrr']:.4f}"
                f"  overlap={cm['mean_gold_term_overlap']:.1%}"
            )
    print()


def main():
    p = argparse.ArgumentParser(
        description="Retrieval-only benchmark evaluation (no LLM cost)"
    )
    p.add_argument("--dataset", required=True, choices=["personamem", "beam", "lifebench"])
    p.add_argument("--split", required=True)
    p.add_argument("--query-limit", type=int, default=None)
    p.add_argument("--query-id", default=None)
    p.add_argument("--category", "-c", default=None)
    p.add_argument("--doc-limit", type=int, default=None)
    p.add_argument("--name", "-n", default=None, help="Run name for output dir")
    p.add_argument("--output-dir", "-o", default="outputs")
    p.add_argument("--k", type=int, default=None, help="Override CHITTA_K")
    p.add_argument("--skip-ingestion", action="store_true")
    p.add_argument("--amb-dir", default=None, help="Path to agent-memory-benchmark")
    args = p.parse_args()

    amb_path = Path(args.amb_dir) if args.amb_dir else _find_amb_dir()
    if amb_path is None or not (amb_path / "src" / "memory_bench").is_dir():
        print(
            "Error: Cannot find agent-memory-benchmark. "
            "Set --amb-dir or AMB_DIR env var.",
            file=sys.stderr,
        )
        sys.exit(1)

    _load_dotenv(amb_path / ".env")
    sys.path.insert(0, str(amb_path / "src"))

    from memory_bench.dataset import get_dataset
    from memory_bench.memory.chitta_mcp import ChittaMCPMemoryProvider

    if args.k:
        os.environ["CHITTA_K"] = str(args.k)

    config = get_chitta_config()
    ds = get_dataset(args.dataset)
    provider = ChittaMCPMemoryProvider()
    provider.initialize()

    print(f"Dataset: {args.dataset}  Split: {args.split}")
    print(f"Config: {json.dumps(config)}")

    queries = ds.load_queries(args.split, category=args.category, limit=args.query_limit)
    if args.query_id:
        queries = [q for q in queries if q.id == args.query_id]
    print(f"Loaded {len(queries)} queries")

    results = []
    ingestion_ms = 0.0
    ingested_docs = 0

    if getattr(ds, "isolation_unit", None) is not None:
        # Unit-sequential (BEAM): ingest one conversation, query, repeat
        user_ids = {q.user_id for q in queries if q.user_id}
        documents = ds.load_documents(
            args.split, limit=args.doc_limit, user_ids=user_ids
        )
        print(f"Loaded {len(documents)} documents")

        docs_by_unit = defaultdict(list)
        for doc in documents:
            if doc.user_id:
                docs_by_unit[doc.user_id].append(doc)

        queries_by_unit = defaultdict(list)
        for q in queries:
            if q.user_id:
                queries_by_unit[q.user_id].append(q)

        total_units = len(docs_by_unit)
        for i, (unit_id, unit_docs) in enumerate(docs_by_unit.items(), 1):
            unit_queries = queries_by_unit.get(unit_id, [])
            if not unit_queries:
                continue

            if not args.skip_ingestion:
                print(f"  [{i}/{total_units}] Ingesting {len(unit_docs)} docs for {unit_id}...")
                t0 = time.perf_counter()
                provider.ingest(unit_docs)
                ingestion_ms += (time.perf_counter() - t0) * 1000
                ingested_docs += len(unit_docs)

            for q in unit_queries:
                results.append(evaluate_query(q, provider))
            print(
                f"  [{i}/{total_units}] {unit_id}: {len(unit_queries)} queries done"
            )
    else:
        # Batch (PersonaMem): ingest all, then query all
        documents = ds.load_documents(args.split, limit=args.doc_limit)
        print(f"Loaded {len(documents)} documents")

        if not args.skip_ingestion:
            print("Ingesting...")
            t0 = time.perf_counter()
            provider.ingest(documents)
            ingestion_ms = (time.perf_counter() - t0) * 1000
            ingested_docs = len(documents)
            print(
                f"  ingested in {ingestion_ms:.0f}ms"
                f" ({ingestion_ms / len(documents):.1f}ms/doc avg)"
            )

        total = len(queries)
        for i, q in enumerate(queries):
            results.append(evaluate_query(q, provider))
            if (i + 1) % 10 == 0 or i + 1 == total:
                print(f"  {i + 1}/{total} queries evaluated")

    provider.cleanup()

    summary = compute_summary(args, results, config, ingestion_ms, ingested_docs)
    print_summary(summary)
    save_results(summary, args)


if __name__ == "__main__":
    main()
