#!/usr/bin/env python3
"""Generate a large sample CSV for testing the viewer.

Writes realistic-looking rows until the file reaches a target size (default
~1 GiB). Uses buffered streaming writes so memory stays flat regardless of the
output size.

Usage:
    python gen_csv.py                 # ~1 GiB -> samples/sample_1gb.csv
    python gen_csv.py --gb 5          # ~5 GiB
    python gen_csv.py --out foo.csv --gb 0.5
"""

from __future__ import annotations

import argparse
import os
import random
import time

HEADER = "id,timestamp,first_name,last_name,email,country,amount,currency,status,note\n"

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


def build_row(i: int, rnd: random.Random) -> str:
    fn = rnd.choice(FIRST)
    ln = rnd.choice(LAST)
    email = f"{fn.lower()}.{ln.lower().replace(' ', '')}{i}@example.com"
    ts = 1_700_000_000 + (i % 31_536_000)  # spread over ~1 year
    amount = rnd.randint(0, 1_000_000) / 100.0
    return (
        f"{i},{ts},{fn},{ln},{email},{rnd.choice(COUNTRY)},"
        f"{amount:.2f},{rnd.choice(CURRENCY)},{rnd.choice(STATUS)},"
        f"{rnd.choice(NOTES)}\n"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate a large sample CSV.")
    parser.add_argument("--gb", type=float, default=1.0,
                        help="Approximate target size in GiB (default: 1.0).")
    parser.add_argument("--out", default=os.path.join("samples", "sample_1gb.csv"),
                        help="Output path (default: samples/sample_1gb.csv).")
    parser.add_argument("--seed", type=int, default=42, help="RNG seed.")
    args = parser.parse_args()

    target = int(args.gb * 1024 * 1024 * 1024)
    out_dir = os.path.dirname(args.out)
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)

    rnd = random.Random(args.seed)
    flush_every = 50_000          # rows per buffer flush
    written = 0
    rows = 0
    start = time.time()
    next_report = start + 1.0

    with open(args.out, "w", encoding="utf-8", newline="") as f:
        f.write(HEADER)
        written += len(HEADER)
        buf: list[str] = []
        i = 1
        while written < target:
            line = build_row(i, rnd)
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
    size_gib = os.path.getsize(args.out) / (1024 ** 3)
    print(f"\rDone: {args.out}  {size_gib:.2f} GiB  {rows:,} rows  in {elapsed:.1f}s")


if __name__ == "__main__":
    main()
