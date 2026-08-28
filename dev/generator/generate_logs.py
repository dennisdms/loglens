#!/usr/bin/env python3
"""Continuously index synthetic log Hits into a data stream.

Uses only the standard library so it runs in a bare `python:3.12-slim` container.
The documents match Log Lens's defaults: an `@timestamp` date field and a
`message` field, plus `level` / `service` / `host` / `duration_ms` / `status`
for exercising columns, sorting, and query strings.
"""

import datetime
import json
import os
import random
import time
import urllib.error
import urllib.request

ES = os.environ.get("ES_URL", "http://localhost:9200").rstrip("/")
DS = os.environ.get("DATA_STREAM", "logs-loglens")
RATE = int(os.environ.get("RATE", "20"))
INTERVAL = float(os.environ.get("INTERVAL", "3"))
BACKFILL_MINUTES = int(os.environ.get("BACKFILL_MINUTES", "60"))

TEMPLATE = {
    "index_patterns": [DS],
    "data_stream": {},
    "priority": 500,
    "template": {
        "mappings": {
            "properties": {
                "@timestamp": {"type": "date"},
                "message": {
                    "type": "text",
                    "fields": {"raw": {"type": "keyword", "ignore_above": 2048}},
                },
                "level": {"type": "keyword"},
                "service": {"type": "keyword"},
                "host": {"type": "keyword"},
                "trace_id": {"type": "keyword"},
                "duration_ms": {"type": "long"},
                "status": {"type": "integer"},
            }
        }
    },
}

SERVICES = ["checkout", "payments", "auth", "catalog", "search", "gateway"]
HOSTS = [f"ip-10-0-{i // 254}-{(i % 254) + 1}" for i in range(8)]
LEVELS = ["INFO"] * 70 + ["DEBUG"] * 15 + ["WARN"] * 10 + ["ERROR"] * 5
MESSAGES = {
    "INFO": ["request completed", "user logged in", "cache hit", "order placed",
             "payment authorized", "session refreshed"],
    "DEBUG": ["entering handler", "db query executed", "feature flag evaluated",
              "response serialized"],
    "WARN": ["slow downstream response", "retrying request",
             "connection pool almost exhausted", "deprecated endpoint called"],
    "ERROR": ["upstream timeout", "unhandled exception", "payment declined",
              "database connection refused", "circuit breaker opened"],
}


def req(method, path, body=None, ndjson=False):
    if ndjson:
        data = body.encode()
        ctype = "application/x-ndjson"
    else:
        data = json.dumps(body).encode() if body is not None else None
        ctype = "application/json"
    request = urllib.request.Request(
        ES + path, data=data, method=method, headers={"Content-Type": ctype}
    )
    with urllib.request.urlopen(request, timeout=30) as resp:
        return json.loads(resp.read().decode() or "{}")


def wait_for_es():
    for _ in range(120):
        try:
            req("GET", "/_cluster/health?wait_for_status=yellow&timeout=5s")
            print("elasticsearch is up", flush=True)
            return
        except (urllib.error.URLError, ConnectionError, OSError) as exc:
            print(f"waiting for elasticsearch: {exc}", flush=True)
            time.sleep(2)
    raise SystemExit("elasticsearch never became ready")


def make_doc(ts):
    level = random.choice(LEVELS)
    service = random.choice(SERVICES)
    text = random.choice(MESSAGES[level])
    status = random.choice([200, 200, 200, 201, 204, 400, 404, 500, 503])
    return {
        "@timestamp": ts.isoformat().replace("+00:00", "Z"),
        "level": level,
        "service": service,
        "host": random.choice(HOSTS),
        "trace_id": f"{random.getrandbits(64):016x}",
        "duration_ms": max(1, int(random.gauss(120, 90))),
        "status": status,
        "message": f"{text} service={service} status={status}",
    }


def bulk(docs):
    if not docs:
        return
    lines = []
    for doc in docs:
        lines.append(json.dumps({"create": {}}))
        lines.append(json.dumps(doc))
    result = req("POST", f"/{DS}/_bulk", "\n".join(lines) + "\n", ndjson=True)
    if result.get("errors"):
        for item in result["items"]:
            err = item.get("create", {}).get("error")
            if err:
                print("bulk error:", err, flush=True)
                break


def backfill():
    if BACKFILL_MINUTES <= 0:
        return
    now = datetime.datetime.now(datetime.timezone.utc)
    start = now - datetime.timedelta(minutes=BACKFILL_MINUTES)
    total = BACKFILL_MINUTES * RATE
    batch = []
    for i in range(total):
        ts = start + (now - start) * (i / total)
        batch.append(make_doc(ts))
        if len(batch) >= 500:
            bulk(batch)
            batch = []
    bulk(batch)
    print(f"backfilled ~{total} hits across the last {BACKFILL_MINUTES} min",
          flush=True)


def main():
    wait_for_es()
    req("PUT", f"/_index_template/{DS}", TEMPLATE)
    backfill()
    while True:
        ts = datetime.datetime.now(datetime.timezone.utc)
        bulk([make_doc(ts) for _ in range(RATE)])
        print(f"{ts.isoformat()} indexed {RATE} hits into {DS}", flush=True)
        time.sleep(INTERVAL)


if __name__ == "__main__":
    main()
