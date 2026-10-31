"""Free Realms Expanded realm directory: hosts announce a listed realm, launchers read the list.

One process, no database: the list lives in memory and every listed realm re-announces each minute, so a
restart or a move to another host refills it within a minute. Runs anywhere a container runs (Cloud Run,
Fly.io, a VPS, Docker on a home PC); the launcher only needs the URL.

Trust: an announce is signed by the realm's listing key, and before a realm is listed the directory connects
to its login broker and has it sign a fresh nonce with that same key (AUTH2 type 3). Only the machine that
runs the realm can answer, so nobody can list a realm they do not run, point a listing at someone else's
address, or rename another host's realm. Only public addresses are contacted.
"""

import ipaddress
import json
import os
import re
import secrets
import socket
import struct
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

TTL = int(os.environ.get("DIRECTORY_TTL", "180"))  # seconds a listing lives without an announce
MAX_ENTRIES = int(os.environ.get("DIRECTORY_MAX_ENTRIES", "500"))
REPROVE = 600  # a listed realm proves its key again after this many seconds
CLOCK_SKEW = 300  # announces carry a timestamp; older or newer ones are replays or bad clocks
PROOF_TIMEOUT = 3.0
# Cloud Run and most proxies append the caller's address to X-Forwarded-For; 0 trusts only the socket.
PROXY_HOPS = int(os.environ.get("DIRECTORY_PROXY_HOPS", "1"))
# Tests run a fake broker on loopback; a deployed directory never sets this.
ALLOW_LOOPBACK = os.environ.get("DIRECTORY_ALLOW_LOOPBACK") == "1"
BLOCKLIST = [
    w.strip().lower()
    for w in open(os.environ["DIRECTORY_BLOCKLIST"], encoding="utf-8").read().splitlines()
    if w.strip()
] if os.environ.get("DIRECTORY_BLOCKLIST") else []

NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 '&!?.,:()_-]{2,31}$")
TEXT = re.compile(r"^[A-Za-z0-9 '&!?.,:;()/_+#@-]{0,100}$")
HEX32 = re.compile(r"^[0-9a-f]{64}$")
HEX64 = re.compile(r"^[0-9a-f]{128}$")

entries = {}  # identity hex -> listing
lock = threading.Lock()
callers = {}  # source address -> (tokens, last time): announce budget per caller


def announce_message(a):
    fields = [a["name"], a["description"], a["address"], a["broker"], a["identity"], a["listing_key"], str(a["time"])]
    return b"osfr-announce-v1\n" + "\n".join(fields).encode()


def delist_message(d):
    return b"osfr-delist-v1\n" + f"{d['identity']}\n{d['time']}".encode()


def endpoint(text):
    """`ip:port` (IPv6 in brackets) on a public address and an unprivileged port, else None."""
    match = re.fullmatch(r"\[([0-9a-fA-F:]+)\]:(\d{1,5})|([0-9.]+):(\d{1,5})", text or "")
    if not match:
        return None
    host, port = (match.group(1), match.group(2)) if match.group(1) else (match.group(3), match.group(4))
    try:
        ip = ipaddress.ip_address(host)
    except ValueError:
        return None
    port = int(port)
    public = ip.is_global or (ALLOW_LOOPBACK and ip.is_loopback)
    return (str(ip), port) if public and 1024 <= port <= 65535 else None


def verify(key_hex, message, sig_hex):
    try:
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(key_hex)).verify(bytes.fromhex(sig_hex), message)
        return True
    except (InvalidSignature, ValueError):
        return False


def prove(broker, identity_hex, key_hex):
    """Ask the realm's login broker to sign a fresh nonce with the listing key (AUTH2 type 3)."""
    nonce = secrets.token_bytes(32)
    body = b"AUTH2\x03" + nonce
    try:
        with socket.create_connection(broker, timeout=PROOF_TIMEOUT) as conn:
            conn.settimeout(PROOF_TIMEOUT)
            conn.sendall(struct.pack("<I", len(body)) + body)
            reply = b""
            while len(reply) < 69:
                chunk = conn.recv(69 - len(reply))
                if not chunk:
                    return False
                reply += chunk
    except OSError:
        return False
    if reply[0] != 1:
        return False
    message = b"osfr-listing-v1" + bytes.fromhex(identity_hex) + nonce
    return verify(key_hex, message, reply[1:65].hex())


def allow_caller(source, now):
    """Six announces a minute per caller address, with a burst of six."""
    tokens, last = callers.get(source, (6.0, now))
    tokens = min(6.0, tokens + (now - last) * 0.1)
    if tokens < 1.0:
        callers[source] = (tokens, now)
        return False
    callers[source] = (tokens - 1.0, now)
    if len(callers) > 10000:
        callers.clear()
    return True


def clean_text(text, pattern, limit):
    text = " ".join(str(text or "").split())[:limit]
    if not pattern.fullmatch(text) or any(word in text.lower() for word in BLOCKLIST):
        return None
    return text


def announce(a, source, now):
    """Returns (HTTP status, reply)."""
    if not allow_caller(source, now):
        return 429, {"error": "Too many announces; one a minute is enough."}
    name = clean_text(a.get("name"), NAME, 32)
    description = clean_text(a.get("description", ""), TEXT, 100)
    address, broker = endpoint(a.get("address")), endpoint(a.get("broker"))
    identity, key, sig = a.get("identity", ""), a.get("listing_key", ""), a.get("sig", "")
    if name is None or description is None:
        return 400, {"error": "Realm names are 3-32 letters, digits and simple punctuation; descriptions up to 100."}
    if not address or not broker or address[0] != broker[0]:
        return 400, {"error": "The realm and its login broker must be on the same public address."}
    if not (HEX32.fullmatch(identity) and HEX32.fullmatch(key) and HEX64.fullmatch(sig)):
        return 400, {"error": "Malformed identity, key or signature."}
    if not isinstance(a.get("time"), int) or abs(a["time"] - now) > CLOCK_SKEW:
        return 400, {"error": "The announce time is off; check this PC's clock."}
    if not verify(key, announce_message(a), sig):
        return 403, {"error": "Bad announce signature."}
    with lock:
        old = entries.get(identity)
        if old and old["listing_key"] != key and now - old["seen"] < TTL:
            return 409, {"error": "This realm is listed with another key."}
        fresh = old and old["listing_key"] == key and old["broker"] == a["broker"] and now - old["proved"] < REPROVE
        if not old and len(entries) >= MAX_ENTRIES:
            return 503, {"error": "The directory is full; try again later."}
    if not fresh and not prove(broker, identity, key):
        return 403, {"error": "Your realm did not answer the listing check. Is it running, with ports forwarded?"}
    with lock:
        entries[identity] = {
            "name": name,
            "description": description,
            "address": a["address"],
            "broker": a["broker"],
            "identity": identity,
            "listing_key": key,
            "seen": now,
            "proved": old["proved"] if fresh else now,
        }
    return 200, {"listed": True, "ttl": TTL}


def delist(d, now):
    identity, time_, sig = d.get("identity", ""), d.get("time"), d.get("sig", "")
    if not (HEX32.fullmatch(identity) and HEX64.fullmatch(sig) and isinstance(time_, int)):
        return 400, {"error": "Malformed delist."}
    if abs(time_ - now) > CLOCK_SKEW:
        return 400, {"error": "The delist time is off; check this PC's clock."}
    with lock:
        old = entries.get(identity)
        if old and verify(old["listing_key"], delist_message(d), sig):
            del entries[identity]
    return 200, {"listed": False}


def listing(now):
    with lock:
        for identity in [i for i, e in entries.items() if now - e["seen"] >= TTL]:
            del entries[identity]
        rows = [
            {k: e[k] for k in ("name", "description", "address", "broker", "identity")}
            for e in entries.values()
        ]
    return sorted(rows, key=lambda r: r["name"].lower())


class Handler(BaseHTTPRequestHandler):
    server_version = "osfr-directory"
    sys_version = ""

    def source(self):
        forwarded = [h.strip() for h in self.headers.get("X-Forwarded-For", "").split(",") if h.strip()]
        if PROXY_HOPS and len(forwarded) >= PROXY_HOPS:
            return forwarded[-PROXY_HOPS]
        return self.client_address[0]

    def reply(self, status, body, cache=0):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", f"public, max-age={cache}" if cache else "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/v1/servers":
            self.reply(200, {"servers": listing(int(time.time()))}, cache=15)
        elif self.path == "/healthz":
            self.reply(200, {"ok": True})
        else:
            self.reply(404, {"error": "Not found."})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if not 0 < length <= 2048:
            return self.reply(413, {"error": "Body too large."})
        try:
            body = json.loads(self.rfile.read(length))
            assert isinstance(body, dict)
        except (ValueError, AssertionError):
            return self.reply(400, {"error": "Body is not a JSON object."})
        now = int(time.time())
        if self.path == "/v1/announce":
            self.reply(*announce(body, self.source(), now))
        elif self.path == "/v1/delist":
            self.reply(*delist(body, now))
        else:
            self.reply(404, {"error": "Not found."})

    def log_message(self, fmt, *args):
        pass  # no caller addresses in logs


def serve(port):
    server = ThreadingHTTPServer(("", port), Handler)
    server.daemon_threads = True
    server.serve_forever()


if __name__ == "__main__":
    serve(int(os.environ.get("PORT", "8080")))
