"""Directory tests against a fake login broker on loopback: python -m unittest test_directory (lead)."""

import json
import os
import socket
import struct
import threading
import time
import unittest
import urllib.request

os.environ["DIRECTORY_ALLOW_LOOPBACK"] = "1"
os.environ["DIRECTORY_PROXY_HOPS"] = "0"

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey  # noqa: E402

import directory  # noqa: E402


def raw(public_key):
    from cryptography.hazmat.primitives import serialization

    return public_key.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)


class FakeBroker:
    """Answers AUTH2 type 3 like osfr-login with --listing-key-file: the signature in a 69-byte reply."""

    def __init__(self, server_id, key):
        self.server_id, self.key = server_id, key
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen()
        self.port = self.sock.getsockname()[1]
        threading.Thread(target=self.run, daemon=True).start()

    def run(self):
        while True:
            conn, _ = self.sock.accept()
            with conn:
                length = struct.unpack("<I", conn.recv(4))[0]
                body = conn.recv(length)
                reply = bytearray(69)
                if body[:6] == b"AUTH2\x03" and len(body) == 38:
                    reply[0] = 1
                    reply[1:65] = self.key.sign(b"osfr-listing-v1" + self.server_id + body[6:38])
                conn.sendall(bytes(reply))


def signed(key, broker_port, identity, **overrides):
    a = {
        "name": "Housing Party",
        "description": "Come decorate with us",
        "address": "127.0.0.1:20042",
        "broker": f"127.0.0.1:{broker_port}",
        "identity": identity.hex(),
        "listing_key": raw(key.public_key()).hex(),
        "time": int(time.time()),
    }
    a.update(overrides)
    a["sig"] = key.sign(directory.announce_message(a)).hex()
    return a


class DirectoryTests(unittest.TestCase):
    def setUp(self):
        directory.entries.clear()
        directory.callers.clear()
        self.key = Ed25519PrivateKey.generate()
        self.server_id = os.urandom(32)
        self.broker = FakeBroker(self.server_id, self.key)

    def announce(self, a, source="198.51.100.7"):
        return directory.announce(a, source, int(time.time()))

    def test_a_realm_that_proves_its_key_is_listed_without_the_key(self):
        status, reply = self.announce(signed(self.key, self.broker.port, self.server_id))
        self.assertEqual((status, reply["listed"]), (200, True))
        rows = directory.listing(int(time.time()))
        self.assertEqual([r["name"] for r in rows], ["Housing Party"])
        self.assertNotIn("listing_key", rows[0])

    def test_a_forged_announce_signature_is_refused(self):
        a = signed(self.key, self.broker.port, self.server_id)
        a["name"] = "Changed After Signing"
        self.assertEqual(self.announce(a)[0], 403)

    def test_nobody_can_list_a_realm_they_do_not_run(self):
        # The attacker signs with their own key, but the realm's broker signs with the host's key.
        thief = Ed25519PrivateKey.generate()
        self.assertEqual(self.announce(signed(thief, self.broker.port, self.server_id))[0], 403)
        self.assertEqual(directory.listing(int(time.time())), [])

    def test_a_wrong_identity_fails_the_proof(self):
        self.assertEqual(self.announce(signed(self.key, self.broker.port, os.urandom(32)))[0], 403)

    def test_private_and_mismatched_addresses_are_refused(self):
        a = signed(self.key, self.broker.port, self.server_id, address="10.0.0.5:20042", broker="10.0.0.5:20043")
        self.assertEqual(self.announce(a)[0], 400)
        b = signed(self.key, self.broker.port, self.server_id, address="127.0.0.2:20042")
        self.assertEqual(self.announce(b)[0], 400, "realm and broker share one address")

    def test_a_live_listing_keeps_its_key(self):
        self.assertEqual(self.announce(signed(self.key, self.broker.port, self.server_id))[0], 200)
        other = Ed25519PrivateKey.generate()
        self.assertEqual(self.announce(signed(other, self.broker.port, self.server_id))[0], 409)

    def test_delist_needs_the_listing_key(self):
        self.announce(signed(self.key, self.broker.port, self.server_id))
        now = int(time.time())
        d = {"identity": self.server_id.hex(), "time": now}
        d["sig"] = Ed25519PrivateKey.generate().sign(directory.delist_message(d)).hex()
        directory.delist(d, now)
        self.assertEqual(len(directory.listing(now)), 1, "a stranger's delist does nothing")
        d["sig"] = self.key.sign(directory.delist_message(d)).hex()
        directory.delist(d, now)
        self.assertEqual(directory.listing(now), [])

    def test_listings_expire_without_announces(self):
        self.announce(signed(self.key, self.broker.port, self.server_id))
        self.assertEqual(directory.listing(int(time.time()) + directory.TTL + 1), [])

    def test_announces_are_rate_limited_per_caller(self):
        a = signed(self.key, self.broker.port, self.server_id)
        statuses = [self.announce(a)[0] for _ in range(7)]
        self.assertEqual(statuses[:6], [200] * 6)
        self.assertEqual(statuses[6], 429)

    def test_stale_announces_and_bad_names_are_refused(self):
        old = signed(self.key, self.broker.port, self.server_id, time=int(time.time()) - 3600)
        self.assertEqual(self.announce(old)[0], 400)
        bad = signed(self.key, self.broker.port, self.server_id, name="<script>")
        self.assertEqual(self.announce(bad)[0], 400)

    def test_http_round_trip(self):
        server = directory.ThreadingHTTPServer(("127.0.0.1", 0), directory.Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{server.server_address[1]}"
        try:
            body = json.dumps(signed(self.key, self.broker.port, self.server_id)).encode()
            request = urllib.request.Request(base + "/v1/announce", data=body, method="POST")
            with urllib.request.urlopen(request) as response:
                self.assertEqual(json.load(response)["listed"], True)
            with urllib.request.urlopen(base + "/v1/servers") as response:
                self.assertEqual(json.load(response)["servers"][0]["name"], "Housing Party")
        finally:
            server.shutdown()


if __name__ == "__main__":
    unittest.main()
