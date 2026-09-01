#!/usr/bin/env python3
"""Continuously index synthetic log Hits into several data streams.

Uses only the standard library so it runs in a bare `python:3.12-slim`
container. Every document has the fields Log Lens defaults to — an
`@timestamp` date and a `message` — plus per-flavour extras for exercising
columns, sorting, and query strings.

The streams deliberately differ in shape so the renderer gets stressed:

  <prefix>-app       single-line service logs (the boring baseline)
  <prefix>-nginx     access logs, some with absurdly long URLs / user agents
  <prefix>-java      multi-line stack traces, Linux (LF) newlines
  <prefix>-winevent  Windows event log text, Windows (CRLF) newlines
  <prefix>-payloads  huge messages: thousands of characters, one line or many
"""

import datetime
import json
import os
import random
import string
import time
import urllib.error
import urllib.request

ES = os.environ.get("ES_URL", "http://localhost:9200").rstrip("/")
PREFIX = os.environ.get(
    "DATA_STREAM_PREFIX", os.environ.get("DATA_STREAM", "logs-loglens")
)
RATE = int(os.environ.get("RATE", "20"))
INTERVAL = float(os.environ.get("INTERVAL", "3"))
BACKFILL_MINUTES = int(os.environ.get("BACKFILL_MINUTES", "60"))
# Comma-separated suffixes, e.g. STREAMS=app,java. Empty means "all of them".
ONLY = [s.strip() for s in os.environ.get("STREAMS", "").split(",") if s.strip()]

TEMPLATE = {
    "index_patterns": [f"{PREFIX}-*"],
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
                # multi-line / long-message bookkeeping, handy as columns
                "line_count": {"type": "integer"},
                "message_chars": {"type": "integer"},
                "newline_style": {"type": "keyword"},
                # java
                "logger": {"type": "keyword"},
                "thread": {"type": "keyword"},
                "exception": {"type": "keyword"},
                # nginx
                "client_ip": {"type": "ip"},
                "method": {"type": "keyword"},
                "path": {"type": "keyword", "ignore_above": 1024},
                "bytes": {"type": "long"},
                # windows
                "channel": {"type": "keyword"},
                "provider": {"type": "keyword"},
                "event_id": {"type": "integer"},
                # payloads
                "payload_kind": {"type": "keyword"},
            }
        }
    },
}

SERVICES = ["checkout", "payments", "auth", "catalog", "search", "gateway"]
HOSTS = [f"ip-10-0-{i // 254}-{(i % 254) + 1}" for i in range(8)]
WIN_HOSTS = [f"WIN-APP{i:02d}.corp.example.com" for i in range(1, 5)]
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
WORDS = ("retry backoff shard replica tenant coupon inventory reservation "
         "idempotency ledger settlement webhook throttle quota upstream "
         "downstream partition checkpoint fallback degraded").split()

ALNUM = string.ascii_letters + string.digits


def blob(n):
    return "".join(random.choices(ALNUM + "+/", k=n))


def words(n):
    return " ".join(random.choices(WORDS, k=n))


def base(ts, **extra):
    doc = {
        "@timestamp": ts.isoformat().replace("+00:00", "Z"),
        "host": random.choice(HOSTS),
        "trace_id": f"{random.getrandbits(64):016x}",
    }
    doc.update(extra)
    return doc


# --------------------------------------------------------------------------
# app: single-line service logs
# --------------------------------------------------------------------------

def make_app_doc(ts):
    level = random.choice(LEVELS)
    service = random.choice(SERVICES)
    status = random.choice([200, 200, 200, 201, 204, 400, 404, 500, 503])
    text = random.choice(MESSAGES[level])
    return base(
        ts,
        level=level,
        service=service,
        duration_ms=max(1, int(random.gauss(120, 90))),
        status=status,
        message=f"{text} service={service} status={status}",
    )


# --------------------------------------------------------------------------
# nginx: access logs, occasionally with a monstrous URL or user agent
# --------------------------------------------------------------------------

UAS = [
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
    "curl/8.6.0",
    "Go-http-client/2.0",
    "python-requests/2.31.0",
]
PATHS = ["/api/v1/orders", "/api/v1/catalog/search", "/healthz", "/api/v1/cart",
         "/static/app.js", "/api/v1/payments/authorize"]


def make_nginx_doc(ts):
    method = random.choice(["GET"] * 6 + ["POST"] * 3 + ["PUT", "DELETE"])
    path = random.choice(PATHS)
    if random.random() < 0.15:
        # a pathological query string: one very long line, no newlines
        pairs = [f"filter[{i}]={blob(random.randint(20, 60))}"
                 for i in range(random.randint(40, 160))]
        path = path + "?" + "&".join(pairs)
    status = random.choice([200] * 6 + [206, 301, 304, 400, 404, 429, 500, 502])
    ua = random.choice(UAS)
    if random.random() < 0.08:
        ua = ua + " " + blob(random.randint(500, 2500))
    ip = f"{random.randint(1, 223)}.{random.randint(0, 255)}." \
         f"{random.randint(0, 255)}.{random.randint(1, 254)}"
    nbytes = random.randint(120, 900_000)
    stamp = ts.strftime("%d/%b/%Y:%H:%M:%S +0000")
    return base(
        ts,
        level="INFO" if status < 400 else ("WARN" if status < 500 else "ERROR"),
        service="gateway",
        client_ip=ip,
        method=method,
        path=path[:1024],
        status=status,
        bytes=nbytes,
        duration_ms=max(1, int(random.gauss(90, 70))),
        message=f'{ip} - - [{stamp}] "{method} {path} HTTP/1.1" {status} '
                f'{nbytes} "-" "{ua}"',
    )


# --------------------------------------------------------------------------
# java: multi-line stack traces with Linux (LF) newlines
# --------------------------------------------------------------------------

EXCEPTIONS = [
    "java.lang.NullPointerException",
    "java.util.concurrent.TimeoutException",
    "org.springframework.dao.DataIntegrityViolationException",
    "java.lang.IllegalStateException",
    "com.example.payments.PaymentDeclinedException",
]
EXC_MSGS = [
    "Cannot invoke \"Order.getTotal()\" because \"order\" is null",
    "Timed out waiting for connection from pool after 30000ms",
    "could not execute statement; constraint [orders_idempotency_key]",
    "Circuit breaker 'payments' is OPEN",
    "card issuer responded 51 (insufficient funds)",
]
PKGS = ["checkout", "payments", "auth", "catalog", "http", "persistence"]
CLASSES = ["OrderService", "PaymentClient", "TokenFilter", "IndexWriter",
           "RetryTemplate", "TransactionInterceptor", "ConnectionPool"]
METHODS = ["submit", "authorize", "doFilter", "flush", "execute", "invoke",
           "acquire", "handle"]
FRAMEWORK = [
    "\tat org.springframework.web.servlet.FrameworkServlet.doPost(FrameworkServlet.java:914)",
    "\tat javax.servlet.http.HttpServlet.service(HttpServlet.java:681)",
    "\tat org.apache.catalina.core.StandardWrapperValve.invoke(StandardWrapperValve.java:197)",
    "\tat java.base/java.lang.Thread.run(Thread.java:1583)",
]


def frames(n):
    out = []
    for _ in range(n):
        cls = random.choice(CLASSES)
        out.append(f"\tat com.example.{random.choice(PKGS)}.{cls}."
                   f"{random.choice(METHODS)}({cls}.java:{random.randint(24, 940)})")
    return out


def make_java_doc(ts):
    level = random.choice(["ERROR"] * 6 + ["WARN"] * 3 + ["INFO"])
    service = random.choice(SERVICES)
    cls = random.choice(CLASSES)
    logger = f"com.example.{random.choice(PKGS)}.{cls}"
    thread = random.choice(
        [f"http-nio-8080-exec-{random.randint(1, 32)}", "scheduling-1",
         "kafka-consumer-0", "main"]
    )
    head = (f"{ts.strftime('%Y-%m-%d %H:%M:%S,%f')[:-3]} {level:<5} "
            f"[{service},{random.getrandbits(64):016x}] {thread} {logger} - ")
    if level == "INFO":
        return base(ts, level=level, service=service, logger=logger,
                    thread=thread, newline_style="lf",
                    duration_ms=max(1, int(random.gauss(120, 90))),
                    message=head + random.choice(MESSAGES["INFO"]))

    exc = random.choice(EXCEPTIONS)
    lines = [head + "request processing failed",
             f"{exc}: {random.choice(EXC_MSGS)}"]
    lines += frames(random.randint(6, 18))
    lines += random.sample(FRAMEWORK, k=random.randint(1, len(FRAMEWORK)))
    if random.random() < 0.6:
        lines.append("Caused by: java.net.SocketTimeoutException: connect timed out")
        lines += frames(random.randint(4, 12))
        lines.append(f"\t... {random.randint(12, 60)} common frames omitted")
    if random.random() < 0.25:
        lines.append("\tSuppressed: java.lang.IllegalStateException: "
                     "connection already returned to pool")
        lines += frames(random.randint(3, 8))
    return base(ts, level=level, service=service, logger=logger, thread=thread,
                exception=exc, newline_style="lf",
                duration_ms=max(1, int(random.gauss(400, 200))),
                message="\n".join(lines))


# --------------------------------------------------------------------------
# winevent: Windows event log text with CRLF newlines
# --------------------------------------------------------------------------

WIN_PROVIDERS = ["Application Error", "Service Control Manager",
                 ".NET Runtime", "MSSQLSERVER", "PowerShell"]


def make_windows_doc(ts):
    provider = random.choice(WIN_PROVIDERS)
    level = random.choice(["ERROR"] * 5 + ["WARN"] * 3 + ["INFO"] * 2)
    event_id = random.choice([1000, 1026, 7000, 7031, 7036, 4625, 17063, 4104])
    host = random.choice(WIN_HOSTS)
    if provider == "PowerShell":
        lines = [
            f"Engine state is changed from Available to Stopped.",
            "",
            "Details:",
            f"\tNewEngineState=Stopped",
            f"\tPreviousEngineState=Available",
            "",
            f"\tSequenceNumber={random.randint(10, 9999)}",
            f"\tHostName=ConsoleHost",
            f"\tHostVersion=5.1.20348.{random.randint(100, 2500)}",
            f"\tEngineVersion=5.1.20348.{random.randint(100, 2500)}",
            f"\tRunspaceId={'-'.join(blob(n) for n in (8, 4, 4, 4, 12))}",
            "",
            "Command: Invoke-RestMethod -Uri https://payments.internal/api/v1/authorize "
            "-Method Post -Body $payload",
            "At C:\\Program Files\\Contoso\\Scripts\\Sync-Orders.ps1:"
            f"{random.randint(10, 400)} char:{random.randint(1, 60)}",
            "+ ... $resp = Invoke-RestMethod -Uri $uri -Method Post -Body $payload",
            "+                ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~",
            "    + CategoryInfo          : InvalidOperation: "
            "(System.Net.HttpWebRequest:HttpWebRequest) [Invoke-RestMethod], WebException",
            "    + FullyQualifiedErrorId : WebCmdletWebResponseException,"
            "Microsoft.PowerShell.Commands.InvokeRestMethodCommand",
        ]
    else:
        lines = [
            f"The description for Event ID {event_id} from source {provider} "
            "was found on the local computer.",
            "",
            f"Faulting application name: contoso.svc.exe, version: 10.0."
            f"{random.randint(0, 9)}.{random.randint(100, 9999)}, "
            f"time stamp: 0x{random.getrandbits(32):08x}",
            f"Faulting module name: {random.choice(['KERNELBASE.dll', 'ntdll.dll', 'clr.dll'])}, "
            f"version: 10.0.20348.{random.randint(100, 2500)}, "
            f"time stamp: 0x{random.getrandbits(32):08x}",
            f"Exception code: 0x{random.choice(['c0000005', 'e0434352', 'c0000409'])}",
            f"Fault offset: 0x{random.getrandbits(32):08x}",
            f"Faulting process id: 0x{random.randint(0x400, 0xffff):x}",
            f"Faulting application start time: 0x{random.getrandbits(48):012x}",
            "Faulting application path: C:\\Program Files\\Contoso\\contoso.svc.exe",
            "Faulting module path: C:\\Windows\\System32\\KERNELBASE.dll",
            f"Report Id: {'-'.join(blob(n) for n in (8, 4, 4, 4, 12))}",
            "Faulting package full name:",
            "Faulting package-relative application ID:",
        ]
    return base(
        ts,
        level=level,
        service=random.choice(SERVICES),
        host=host,
        channel=random.choice(["Application", "System", "Security"]),
        provider=provider,
        event_id=event_id,
        newline_style="crlf",
        message="\r\n".join(lines),
    )


# --------------------------------------------------------------------------
# payloads: thousands of characters, single-line and multi-line
# --------------------------------------------------------------------------

def payload_json(ts):
    items = [
        {
            "sku": f"SKU-{random.randint(100000, 999999)}",
            "qty": random.randint(1, 9),
            "price_cents": random.randint(199, 99999),
            "warehouse": random.choice(["EU-CENTRAL", "US-EAST", "AP-SOUTH"]),
            "notes": words(random.randint(6, 20)),
        }
        for _ in range(random.randint(25, 90))
    ]
    body = json.dumps(
        {"order_id": f"ord_{blob(18)}", "customer_id": random.randint(1, 10**7),
         "idempotency_key": blob(43), "items": items},
        indent=2,
    )
    return ("payload validation failed, dumping request body:\n" + body, "json")


def payload_base64(ts):
    # one enormous line, no newlines at all
    return (
        f"attachment scan failed (id={blob(22)}), raw body follows: "
        + blob(random.randint(4000, 20000)),
        "base64",
    )


def payload_sql(ts):
    ids = ", ".join(str(random.randint(10**6, 10**7))
                    for _ in range(random.randint(250, 900)))
    return (
        f"slow query detected ({random.randint(1200, 45000)} ms), plan below:\n"
        "SELECT o.id, o.customer_id, o.total_cents, o.created_at,\n"
        "       l.sku, l.qty, l.price_cents, w.region, p.status\n"
        "FROM orders o\n"
        "  JOIN order_lines l ON l.order_id = o.id\n"
        "  JOIN warehouses w ON w.id = l.warehouse_id\n"
        "  LEFT JOIN payments p ON p.order_id = o.id\n"
        f"WHERE o.id IN ({ids})\n"
        "  AND o.created_at >= now() - interval '30 days'\n"
        "ORDER BY o.created_at DESC\n"
        "LIMIT 1000;",
        "sql",
    )


def payload_http_dump(ts):
    headers = [
        "POST /api/v1/payments/authorize HTTP/1.1",
        "Host: payments.internal",
        "Content-Type: application/json",
        f"X-Request-Id: {blob(26)}",
        f"Authorization: Bearer {blob(random.randint(600, 1800))}",
        f"Cookie: session={blob(random.randint(400, 1200))}; ab={blob(64)}",
        f"User-Agent: {random.choice(UAS)}",
        "",
        json.dumps({"amount_cents": random.randint(100, 10**6),
                    "currency": "EUR",
                    "instrument": blob(random.randint(800, 4000))}),
    ]
    # a Windows-style dump: CRLF between every line
    return ("upstream rejected the request, full exchange follows:\r\n"
            + "\r\n".join(headers), "http")


def payload_wall_of_text(ts):
    paras = ["\n".join(words(random.randint(30, 60)) for _ in range(random.randint(4, 12)))
             for _ in range(random.randint(3, 8))]
    return ("diagnostic dump:\n" + "\n\n".join(paras), "text")


PAYLOADS = [payload_json, payload_base64, payload_sql, payload_http_dump,
            payload_wall_of_text]


def make_payload_doc(ts):
    text, kind = random.choice(PAYLOADS)(ts)
    return base(
        ts,
        level=random.choice(["ERROR"] * 5 + ["WARN"] * 3 + ["DEBUG"] * 2),
        service=random.choice(SERVICES),
        payload_kind=kind,
        duration_ms=max(1, int(random.gauss(900, 400))),
        message=text,
    )


# --------------------------------------------------------------------------


class Stream:
    def __init__(self, suffix, weight, batch, make):
        self.suffix = suffix
        self.name = f"{PREFIX}-{suffix}"
        self.weight = weight
        self.batch = batch
        self.make = make
        self.count = 0

    def doc(self, ts):
        doc = self.make(ts)
        msg = doc["message"]
        doc["message_chars"] = len(msg)
        doc["line_count"] = msg.count("\n") + 1
        doc.setdefault(
            "newline_style",
            "crlf" if "\r\n" in msg else ("lf" if "\n" in msg else "none"),
        )
        self.count += 1
        return doc


ALL_STREAMS = [
    Stream("app", 50, 500, make_app_doc),
    Stream("nginx", 25, 500, make_nginx_doc),
    Stream("java", 12, 200, make_java_doc),
    Stream("winevent", 9, 200, make_windows_doc),
    Stream("payloads", 4, 25, make_payload_doc),
]
STREAMS = [s for s in ALL_STREAMS if not ONLY or s.suffix in ONLY]
if not STREAMS:
    raise SystemExit(f"STREAMS={ONLY} matched none of "
                     f"{[s.suffix for s in ALL_STREAMS]}")
WEIGHTS = [s.weight for s in STREAMS]


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
    with urllib.request.urlopen(request, timeout=60) as resp:
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


def drop_legacy():
    """Remove the single `<prefix>` data stream this script used to write.

    Older revisions wrote everything into one stream named exactly PREFIX;
    it would otherwise linger in the typeahead full of stale docs, and its
    exact-match template blocks nothing but confuses. Only that one name is
    touched — never the `<prefix>-*` streams. Best effort: this is a
    throwaway dev cluster.
    """
    try:
        streams = req("GET", f"/_data_stream/{PREFIX}").get("data_streams", [])
    except (urllib.error.HTTPError, urllib.error.URLError, OSError):
        return
    if not any(ds.get("name") == PREFIX for ds in streams):
        return
    for method, path in (("DELETE", f"/_data_stream/{PREFIX}"),
                         ("DELETE", f"/_index_template/{PREFIX}")):
        try:
            req(method, path)
        except (urllib.error.HTTPError, urllib.error.URLError, OSError) as exc:
            print(f"could not clean up legacy {path}: {exc}", flush=True)
    print(f"dropped the legacy single data stream {PREFIX}", flush=True)


def bulk(stream, docs):
    if not docs:
        return
    lines = []
    for doc in docs:
        lines.append(json.dumps({"create": {}}))
        lines.append(json.dumps(doc))
    result = req("POST", f"/{stream.name}/_bulk", "\n".join(lines) + "\n",
                 ndjson=True)
    if result.get("errors"):
        for item in result["items"]:
            err = item.get("create", {}).get("error")
            if err:
                print(f"bulk error ({stream.name}):", err, flush=True)
                break


def flush(buckets, force=False):
    for stream, docs in buckets.items():
        if docs and (force or len(docs) >= stream.batch):
            bulk(stream, docs)
            docs.clear()


def tick(timestamps):
    """Index one doc per timestamp, spread across the enabled streams."""
    buckets = {s: [] for s in STREAMS}
    for ts in timestamps:
        stream = random.choices(STREAMS, weights=WEIGHTS, k=1)[0]
        buckets[stream].append(stream.doc(ts))
        flush(buckets)
    flush(buckets, force=True)


def backfill():
    if BACKFILL_MINUTES <= 0:
        return
    now = datetime.datetime.now(datetime.timezone.utc)
    start = now - datetime.timedelta(minutes=BACKFILL_MINUTES)
    total = BACKFILL_MINUTES * RATE
    span = now - start
    tick(start + span * (i / total) for i in range(total))
    print(f"backfilled ~{total} hits across the last {BACKFILL_MINUTES} min: "
          + ", ".join(f"{s.name}={s.count}" for s in STREAMS), flush=True)


def main():
    wait_for_es()
    drop_legacy()
    req("PUT", f"/_index_template/{PREFIX}-streams", TEMPLATE)
    print("streams: " + ", ".join(s.name for s in STREAMS), flush=True)
    backfill()
    while True:
        ts = datetime.datetime.now(datetime.timezone.utc)
        tick([ts] * RATE)
        print(f"{ts.isoformat()} indexed {RATE} hits ("
              + ", ".join(f"{s.name}={s.count}" for s in STREAMS) + ")",
              flush=True)
        time.sleep(INTERVAL)


if __name__ == "__main__":
    main()
