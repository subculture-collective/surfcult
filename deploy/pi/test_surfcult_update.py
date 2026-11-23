"""Tests for surfcult-update against a fake GitHub API. Run: make updater-test"""
import hashlib
import http.server
import importlib.machinery
import importlib.util
import io
import json
import os
import struct
import tempfile
import threading
import types
import unittest
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
loader = importlib.machinery.SourceFileLoader("surfcult_update", os.path.join(HERE, "surfcult-update"))
spec = importlib.util.spec_from_loader("surfcult_update", loader)
u = importlib.util.module_from_spec(spec)
loader.exec_module(u)

REPO = "o/r"
SHA1 = "1" * 40
SHA2 = "2" * 40


def elf(machine=0xB7, payload=b"surfcult"):
    header = bytearray(64)
    header[:4] = b"\x7fELF"
    header[4], header[5], header[6] = 2, 1, 1
    struct.pack_into("<H", header, 16, 2)
    struct.pack_into("<H", header, 18, machine)
    return bytes(header) + payload


def build_zip(binary, checksum=None, names=("surfcult", "surfcult.sha256")):
    checksum = checksum if checksum is not None else hashlib.sha256(binary).hexdigest()
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as archive:
        archive.writestr(names[0], binary)
        archive.writestr(names[1], checksum + "  surfcult\n")
    return out.getvalue()


class Fake:
    """State shared with the request handler."""
    def __init__(self):
        self.runs = []
        self.archive = b""
        self.expired = False
        self.auth_seen = {}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _json(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        fake = self.server.fake
        path = self.path.split("?")[0]
        fake.auth_seen[path] = self.headers.get("Authorization")
        base = "http://127.0.0.1:%d" % self.server.server_port
        if path == "/repos/%s/actions/workflows/pr-ci.yml/runs" % REPO:
            self._json({"workflow_runs": fake.runs})
        elif path.endswith("/artifacts") and path.startswith("/repos/%s/actions/runs/" % REPO):
            self._json({"artifacts": [{"name": "surfcult-aarch64", "expired": fake.expired,
                                       "archive_download_url": base + "/repos/%s/actions/artifacts/9/zip" % REPO}]})
        elif path == "/repos/%s/actions/artifacts/9/zip" % REPO:
            if self.headers.get("Authorization") != "Bearer secret-token":
                self.send_response(401)
                self.end_headers()
                return
            self.send_response(302)
            self.send_header("Location", base + "/blob/signed")
            self.end_headers()
        elif path == "/blob/signed":
            self.send_response(200)
            self.send_header("Content-Length", str(len(fake.archive)))
            self.end_headers()
            self.wfile.write(fake.archive)
        else:
            self.send_response(404)
            self.end_headers()


class UpdaterTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.fake = self.server.fake = Fake()
        threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        token = os.path.join(self.tmp.name, "token")
        with open(token, "w") as handle:
            handle.write("secret-token\n")
        u.API = "http://127.0.0.1:%d" % self.server.server_port
        u.REPO, u.WORKFLOW, u.ARTIFACT = REPO, "pr-ci.yml", "surfcult-aarch64"
        u.ROOT, u.TOKEN_FILE, u.KEEP, u.SETTLE_SECONDS = os.path.join(self.tmp.name, "opt"), token, 2, 0
        u.smoke = lambda path: True
        self.service = types.SimpleNamespace(exists=False, active=["active"], calls=[])
        u.systemctl = self.fake_systemctl
        self.fake.archive = build_zip(elf())

    def fake_systemctl(self, *args):
        self.service.calls.append(args)
        if args[0] == "cat":
            return types.SimpleNamespace(returncode=0 if self.service.exists else 1, stdout="")
        if args[0] == "is-active":
            state = self.service.active.pop(0) if len(self.service.active) > 1 else self.service.active[0]
            return types.SimpleNamespace(returncode=0, stdout=state + "\n")
        return types.SimpleNamespace(returncode=0, stdout="")

    def run_main(self, sha=SHA1, run_id=5):
        self.fake.runs = [{"id": run_id, "head_sha": sha, "head_branch": "main", "event": "push", "conclusion": "success"}]
        return u.main()

    def installed(self):
        return u.current_release()

    def test_installs_and_switches_current(self):
        self.assertEqual(self.run_main(), 0)
        self.assertEqual(self.installed(), SHA1)
        binary = os.path.join(u.ROOT, "current", "surfcult")
        with open(binary, "rb") as handle:
            self.assertEqual(handle.read(), elf())
        self.assertTrue(os.access(binary, os.X_OK))

    def test_token_goes_to_the_api_and_not_to_the_signed_url(self):
        self.run_main()
        self.assertEqual(self.fake.auth_seen["/repos/%s/actions/artifacts/9/zip" % REPO], "Bearer secret-token")
        self.assertIsNone(self.fake.auth_seen["/blob/signed"])

    def test_same_build_is_a_no_op(self):
        self.run_main()
        self.fake.archive = b"would fail if it were downloaded"
        self.assertEqual(self.run_main(), 0)
        self.assertEqual(self.installed(), SHA1)

    def test_refuses_a_wrong_checksum(self):
        self.fake.archive = build_zip(elf(), checksum="0" * 64)
        self.assertEqual(self.run_main(), 1)
        self.assertIsNone(self.installed())
        self.assertEqual(os.listdir(os.path.join(u.ROOT, "releases")), [])

    def test_refuses_a_binary_for_another_architecture(self):
        self.fake.archive = build_zip(elf(machine=0x3E))
        self.assertEqual(self.run_main(), 1)
        self.assertIsNone(self.installed())

    def test_refuses_unexpected_archive_contents(self):
        self.fake.archive = build_zip(elf(), names=("../evil", "surfcult.sha256"))
        self.assertEqual(self.run_main(), 1)
        self.assertIsNone(self.installed())

    def test_refuses_a_commit_id_that_is_not_a_sha(self):
        self.assertEqual(self.run_main(sha="../../etc"), 1)
        self.assertIsNone(self.installed())

    def test_refuses_a_failed_smoke_test_and_keeps_the_old_release(self):
        self.run_main(SHA1)
        u.smoke = lambda path: False
        self.fake.archive = build_zip(elf(payload=b"new"))
        self.assertEqual(self.run_main(SHA2), 1)
        self.assertEqual(self.installed(), SHA1)
        self.assertEqual(sorted(os.listdir(os.path.join(u.ROOT, "releases"))), [SHA1])

    def test_expired_artifact_leaves_things_alone(self):
        self.fake.expired = True
        self.assertEqual(self.run_main(), 0)
        self.assertIsNone(self.installed())

    def test_restarts_the_unit_and_keeps_the_build_when_it_comes_up(self):
        self.service.exists = True
        self.assertEqual(self.run_main(), 0)
        self.assertIn(("restart", u.UNIT), self.service.calls)
        self.assertEqual(self.installed(), SHA1)

    def test_rolls_back_when_the_unit_does_not_come_up_and_does_not_retry(self):
        self.run_main(SHA1)
        self.service.exists = True
        self.service.active = ["failed", "active"]
        self.fake.archive = build_zip(elf(payload=b"bad"))
        self.assertEqual(self.run_main(SHA2), 1)
        self.assertEqual(self.installed(), SHA1)
        with open(os.path.join(u.ROOT, "failed")) as handle:
            self.assertEqual(handle.read().split(), [SHA2])
        self.fake.archive = b"must not be downloaded again"
        self.assertEqual(self.run_main(SHA2), 0)
        self.assertEqual(self.installed(), SHA1)

    def test_keeps_only_the_newest_releases_plus_current_and_previous(self):
        for index, sha in enumerate(["3" * 40, "4" * 40, "5" * 40, "6" * 40]):
            self.fake.archive = build_zip(elf(payload=bytes([index])))
            self.assertEqual(self.run_main(sha), 0)
            os.utime(os.path.join(u.ROOT, "releases", sha), (index, index))
        names = sorted(os.listdir(os.path.join(u.ROOT, "releases")))
        self.assertEqual(names, ["5" * 40, "6" * 40])
        self.assertEqual(self.installed(), "6" * 40)

    def test_a_missing_token_file_is_reported_not_raised(self):
        os.remove(u.TOKEN_FILE)
        self.assertEqual(self.run_main(), 1)


if __name__ == "__main__":
    unittest.main()
