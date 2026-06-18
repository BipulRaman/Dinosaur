#!/usr/bin/env python3
"""Generate a large sample data file for testing the viewer.

Writes realistic-looking rows until the file reaches a target size (default
~1 GiB). Uses buffered streaming writes so memory stays flat regardless of the
output size. Supports CSV, TSV, NDJSON and plain-text output.

Usage:
    python gen_csv.py                       # ~1 GiB CSV  -> samples/sample_1gb.csv
    python gen_csv.py --format tsv          # ~1 GiB TSV  -> samples/sample_1gb.tsv
    python gen_csv.py --format json --gb 5  # ~5 GiB NDJSON
    python gen_csv.py --out foo.csv --gb 0.5
"""

from __future__ import annotations

import argparse
import json
import os
import random
import time

FIELDS = ["id", "timestamp", "first_name", "last_name", "email",
          "country", "amount", "currency", "status", "note"]

FIRST = ["Ada", "Alan", "Grace", "Linus", "Margaret", "Dennis", "Ken", "Barbara",
         "Edsger", "Donald", "Katherine", "Tim", "Radia", "Hedy", "Guido", "James"]
LAST = ["Lovelace", "Turing", "Hopper", "Torvalds", "Hamilton", "Ritchie",
        "Thompson", "Liskov", "Dijkstra", "Knuth", "Johnson", "Berners-Lee",
        "Perlman", "Lamarr", "Rossum", "Gosling"]
COUNTRY = ["US", "GB", "DE", "FR", "IN", "JP", "BR", "CA", "AU", "NL", "SE", "ZA"]
CURRENCY = ["USD", "GBP", "EUR", "JPY", "INR", "BRL", "CAD", "AUD"]
STATUS = ["active", "pending", "closed", "refunded", "failed"]
NOTES = ["", "priority customer", "follow up", "VIP", "test record",
         "needs review", "auto-generated", "do not contact"]

EXT = {"csv": "csv", "tsv": "tsv", "json": "json", "txt": "txt"}


def build_fields(i: int, rnd: random.Random) -> list:
    fn = rnd.choice(FIRST)
    ln = rnd.choice(LAST)
    email = f"{fn.lower()}.{ln.lower().replace(' ', '')}{i}@example.com"
    ts = 1_700_000_000 + (i % 31_536_000)  # spread over ~1 year
    amount = rnd.randint(0, 1_000_000) / 100.0
    return [i, ts, fn, ln, email, rnd.choice(COUNTRY),
            f"{amount:.2f}", rnd.choice(CURRENCY), rnd.choice(STATUS),
            rnd.choice(NOTES)]


def header_line(fmt: str) -> str:
    if fmt == "csv":
        return ",".join(FIELDS) + "\n"
    if fmt == "tsv":
        return "\t".join(FIELDS) + "\n"
    return ""  # json/txt have no header line


def build_row(i: int, rnd: random.Random, fmt: str) -> str:
    f = build_fields(i, rnd)
    if fmt == "csv":
        return ",".join(str(x) for x in f) + "\n"
    if fmt == "tsv":
        return "\t".join(str(x) for x in f) + "\n"
    if fmt == "json":
        return json.dumps(dict(zip(FIELDS, f)), separators=(",", ":")) + "\n"
    # txt: one human-readable line per record
    return (f"{f[0]} {f[1]} {f[2]} {f[3]} <{f[4]}> {f[5]} "
            f"{f[6]} {f[7]} {f[8]} {f[9]}".rstrip() + "\n")


# --- Unstructured (nested, heterogeneous) NDJSON --------------------------------
# Each line is still ONE compact JSON value (so the line-based index loads it),
# but records have nested objects, arrays and a randomly varying set of keys.

TAGS = ["alpha", "beta", "gamma", "delta", "prod", "staging", "eu", "us",
        "urgent", "legacy", "beta-flag", "v2", "archived"]
KEY_POOL = ["meta", "address", "prefs", "tags", "history", "scores",
            "contact", "flags", "geo", "items", "parent", "attributes"]


def _scalar(rnd: random.Random):
    return rnd.choice([
        rnd.randint(-1000, 100000),
        round(rnd.uniform(-1e3, 1e6), 3),
        rnd.choice([True, False, None]),
        rnd.choice(STATUS),
        rnd.choice(NOTES) or rnd.choice(FIRST),
    ])


def _value(rnd: random.Random, depth: int):
    # Deeper recursion gets rarer; keeps each line bounded in size.
    if depth <= 0 or rnd.random() < 0.55:
        return _scalar(rnd)
    roll = rnd.random()
    if roll < 0.45:
        return {rnd.choice(KEY_POOL): _value(rnd, depth - 1)
                for _ in range(rnd.randint(1, 4))}
    if roll < 0.8:
        return [_value(rnd, depth - 1) for _ in range(rnd.randint(0, 5))]
    return rnd.sample(TAGS, rnd.randint(0, min(4, len(TAGS))))


def build_unstructured(i: int, rnd: random.Random) -> str:
    fn = rnd.choice(FIRST)
    ln = rnd.choice(LAST)
    rec = {
        "id": i,
        "type": rnd.choice(["user", "order", "event", "node", "record"]),
    }
    # A randomly varying subset of optional, often-nested keys.
    for key in rnd.sample(KEY_POOL, rnd.randint(1, 6)):
        rec[key] = _value(rnd, depth=3)
    # Sometimes include a couple of recognisable fields, sometimes not.
    if rnd.random() < 0.7:
        rec["name"] = f"{fn} {ln}"
    if rnd.random() < 0.5:
        rec["email"] = f"{fn.lower()}.{ln.lower().replace(' ', '')}{i}@example.com"
    if rnd.random() < 0.4:
        rec["amount"] = round(rnd.uniform(0, 10000), 2)
    return json.dumps(rec, separators=(",", ":")) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate a large sample data file.")
    parser.add_argument("--format", choices=sorted(EXT), default="csv",
                        help="Output format: csv, tsv, json (ndjson), txt.")
    parser.add_argument("--gb", type=float, default=1.0,
                        help="Approximate target size in GiB (default: 1.0).")
    parser.add_argument("--out", default=None,
                        help="Output path (default: samples/sample_1gb.<ext>).")
    parser.add_argument("--seed", type=int, default=42, help="RNG seed.")
    parser.add_argument("--unstructured", action="store_true",
                        help="Emit nested, heterogeneous NDJSON (implies --format json).")
    args = parser.parse_args()

    fmt = "json" if args.unstructured else args.format
    default_name = "sample_1gb_unstructured.json" if args.unstructured \
        else f"sample_1gb.{EXT[fmt]}"
    out = args.out or os.path.join("samples", default_name)

    target = int(args.gb * 1024 * 1024 * 1024)
    out_dir = os.path.dirname(out)
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)

    rnd = random.Random(args.seed)
    flush_every = 50_000          # rows per buffer flush
    written = 0
    rows = 0
    start = time.time()
    next_report = start + 1.0

    with open(out, "w", encoding="utf-8", newline="") as f:
        head = "" if args.unstructured else header_line(fmt)
        if head:
            f.write(head)
            written += len(head)
        buf: list[str] = []
        i = 1
        while written < target:
            line = (build_unstructured(i, rnd) if args.unstructured
                    else build_row(i, rnd, fmt))
            buf.append(line)
            written += len(line)
            rows += 1
            i += 1
            if len(buf) >= flush_every:
                f.write("".join(buf))
                buf.clear()
                now = time.time()
                if now >= next_report:
                    pct = written / target * 100
                    mb = written / (1024 * 1024)
                    print(f"\r{pct:5.1f}%  {mb:8.1f} MiB  {rows:,} rows", end="", flush=True)
                    next_report = now + 1.0
        if buf:
            f.write("".join(buf))

    elapsed = time.time() - start
    size_gib = os.path.getsize(out) / (1024 ** 3)
    print(f"\rDone: {out}  {size_gib:.2f} GiB  {rows:,} rows  in {elapsed:.1f}s")


if __name__ == "__main__":
    main()
