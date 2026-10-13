"""Tests for tools/gmail_seed.py (T-1120b).

Every test fails if the behaviour it names is removed. No network and no
credentials: the Gmail REST API and the token endpoint are a local
`http.server`, and the refresh token in the environment is a fixture.
"""

import base64
import collections
import http.server
import io
import json
import pathlib
import stat
import sys
import tempfile
import threading
import unittest
import urllib.parse
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import gmail_seed as seed  # noqa: E402

ACCOUNT = "demo-freelancer"
ADDRESS = "owner@example.com"
OTHER_ADDRESS = "someone-else@example.com"
# Fixture values, not credentials. Deliberately not shaped like a real Google
# OAuth access token: the CI semgrep secrets ruleset flags that shape, and
# nothing in the tool depends on it.
FAKE_TOKEN = "access-token-zzq"
SUBMITTED_SECRET = "refresh-token-zzq"
API_PREFIX = "/gmail/v1"
SUBJECT_CANARY = "canary-subject-zzq"
BODY_CANARY = "canary-body-zzq"
BASELINE = {"INBOX", "UNREAD", seed.SYNTHETIC_LABEL}
ENV = {
    seed.CLIENT_ID_VAR: "client-id.apps.googleusercontent.com",
    seed.CLIENT_SECRET_VAR: "client-secret-zzq",
    seed.REFRESH_TOKEN_VAR: SUBMITTED_SECRET,
}


# --------------------------------------------------------------------------- #
# The fake Gmail REST API
# --------------------------------------------------------------------------- #


class FakeGmail:
    """An in-memory Gmail REST API with a receipts-free view of the world."""

    def __init__(self, address=ADDRESS):
        self.address = address
        self.labels = {}
        self.messages = {}
        self.calls = []
        self.fail_next = collections.deque()
        self.token_invalid = False
        self.page_size = None
        self._serial = 0

    # -- test-side helpers -------------------------------------------------
    def _next_ids(self):
        self._serial += 1
        return f"gm-{self._serial:06d}", f"th-{self._serial:06d}"

    def add_message(self, rfc822, provenance, labels=("INBOX", "UNREAD"), in_trash=False):
        gmail_id, thread = self._next_ids()
        label_set = set(labels)
        if in_trash:
            label_set.add("TRASH")
        self.messages[gmail_id] = {
            "id": gmail_id,
            "thread": thread,
            "rfc822": rfc822,
            "provenance": provenance,
            "labels": label_set,
        }
        return gmail_id

    def by_rfc822(self, rfc822):
        for message in self.messages.values():
            if message["rfc822"] == rfc822:
                return message
        return None

    def set_labels(self, gmail_id, labels):
        self.messages[gmail_id]["labels"] = set(labels)

    def set_provenance(self, gmail_id, value):
        self.messages[gmail_id]["provenance"] = value

    def remove(self, gmail_id):
        self.messages.pop(gmail_id, None)

    def paths(self):
        return [call["path"] for call in self.calls]

    @staticmethod
    def _strip(path):
        return path[len(API_PREFIX):] if path.startswith(API_PREFIX) else path

    def user_paths(self):
        return [self._strip(path) for path in self.paths() if self._strip(path).startswith("/users/")]

    def count(self, suffix):
        """Writes to a path suffix: the tool's mutating calls are POSTs."""
        return len(
            [
                call
                for call in self.calls
                if call["method"] == "POST" and self._strip(call["path"]).endswith(suffix)
            ]
        )

    def mutations(self):
        """Every write this tool is allowed to make, the token exchange apart."""
        return [call for call in self.calls if call["method"] == "POST" and call["path"] != "/token"]

    def list_ids(self, include_spam_trash, label_ids=None, query=None):
        params = {
            "includeSpamTrash": ["true" if include_spam_trash else "false"],
        }
        if label_ids:
            params["labelIds"] = list(label_ids)
        if query:
            params["q"] = [query]
        return [entry["id"] for entry in self._list(params)["messages"]]

    # -- the API -----------------------------------------------------------
    def handle(self, method, path, params, body):
        if self.fail_next:
            suffix, status = self.fail_next[0]
            if path.endswith(suffix):
                self.fail_next.popleft()
                return status, {"error": {"code": status, "message": "temporary"}}
        if path == "/token":
            if self.token_invalid:
                return 400, {"error": "invalid_grant", "error_description": "expired"}
            return 200, {"access_token": FAKE_TOKEN, "expires_in": 3600, "token_type": "Bearer"}
        p = path[len(API_PREFIX):] if path.startswith(API_PREFIX) else path
        if p == "/users/me/profile":
            return 200, {"emailAddress": self.address, "messagesTotal": len(self.messages)}
        if p == "/users/me/labels":
            if method == "GET":
                return 200, {"labels": [{"id": n, "name": n, "type": "user"} for n in sorted(self.labels)]}
            name = (body or {}).get("name")
            self.labels[name] = None
            return 200, {"id": name, "name": name, "type": "user"}
        if p in ("/users/me/messages/send", "/users/me/drafts/send"):
            return 200, {"id": "gm-sent", "threadId": "th-sent"}
        if p == "/users/me/messages":
            return 200, self._list(params)
        if p == "/users/me/messages/import":
            return 200, self._import(body)
        if p.startswith("/users/me/messages/"):
            parts = p.split("/")
            gmail_id = parts[4]
            action = parts[5] if len(parts) > 5 else None
            message = self.messages.get(gmail_id)
            if message is None:
                return 404, {"error": {"code": 404, "message": "no such message"}}
            if action is None and method == "GET":
                return 200, self._metadata(gmail_id)
            if action == "modify":
                message["labels"] |= set((body or {}).get("addLabelIds") or [])
                message["labels"] -= set((body or {}).get("removeLabelIds") or [])
                return 200, {"id": gmail_id, "labelIds": sorted(message["labels"])}
            if action == "untrash":
                message["labels"].discard("TRASH")
                return 200, {"id": gmail_id, "labelIds": sorted(message["labels"])}
            if action == "trash":
                message["labels"].add("TRASH")
                return 200, {"id": gmail_id, "labelIds": sorted(message["labels"])}
        return 404, {"error": {"code": 404, "message": "not found"}}

    def _list(self, params):
        query = (params.get("q") or [None])[0]
        include = (params.get("includeSpamTrash") or ["false"])[0] == "true"
        label_ids = set(params.get("labelIds") or [])
        token = (params.get("pageToken") or [None])[0]
        matched = []
        for message in self.messages.values():
            if not include and ({"TRASH", "SPAM"} & message["labels"]):
                continue
            if label_ids and not label_ids <= message["labels"]:
                continue
            if query:
                if not query.startswith("rfc822msgid:"):
                    continue
                if message["rfc822"] != query.split(":", 1)[1]:
                    continue
            matched.append(message["id"])
        matched.sort()
        start = int(token) if token else 0
        size = self.page_size or (len(matched) or 1)
        page = matched[start : start + size]
        payload = {"messages": [{"id": i, "threadId": self.messages[i]["thread"]} for i in page]}
        if start + size < len(matched):
            payload["nextPageToken"] = str(start + size)
        return payload

    def _import(self, body):
        body = body or {}
        encoded = (body.get("raw") or "").encode("ascii")
        raw = base64.urlsafe_b64decode(encoded + b"=" * (-len(encoded) % 4))
        rfc822 = (seed.header_value(raw, "Message-ID") or "").strip("<>")
        provenance = seed.header_value(raw, seed.PROVENANCE_HEADER)
        labels = set(body.get("labelIds") or [])
        if body.get("neverMarkSpam"):
            labels.discard("SPAM")
        gmail_id, thread = self._next_ids()
        self.messages[gmail_id] = {
            "id": gmail_id,
            "thread": thread,
            "rfc822": rfc822,
            "provenance": provenance,
            "labels": labels,
        }
        return {"id": gmail_id, "threadId": thread, "labelIds": sorted(labels)}

    def _metadata(self, gmail_id):
        message = self.messages[gmail_id]
        return {
            "id": gmail_id,
            "threadId": message["thread"],
            "labelIds": sorted(message["labels"]),
            "payload": {
                "headers": [
                    {"name": "Message-ID", "value": f"<{message['rfc822']}>"},
                    {"name": seed.PROVENANCE_HEADER, "value": message["provenance"]},
                ]
            },
        }


class _Handler(http.server.BaseHTTPRequestHandler):
    """Routes one request to the fake and answers it."""

    def log_message(self, format, *args):  # noqa: A002 - the stdlib's name
        return

    def _dispatch(self, method):
        fake = self.server.fake
        path, _, query = self.path.partition("?")
        params = urllib.parse.parse_qs(query)
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        content_type = self.headers.get("Content-Type", "")
        if content_type.startswith("application/json"):
            body = json.loads(raw) if raw else None
        elif content_type.startswith("application/x-www-form-urlencoded"):
            body = dict(urllib.parse.parse_qsl(raw.decode("ascii")))
        else:
            body = raw or None
        fake.calls.append({"method": method, "path": path, "params": params, "body": body})
        status, payload = fake.handle(method, path, params, body)
        data = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):  # noqa: N802 - the stdlib's name
        self._dispatch("GET")

    def do_POST(self):  # noqa: N802 - the stdlib's name
        self._dispatch("POST")


# --------------------------------------------------------------------------- #
# Corpus fixtures
# --------------------------------------------------------------------------- #


def eml_without_provenance(message_id, subject, body):
    """The pre-hash bytes of one message (no provenance header yet)."""
    head = (
        "From: sender@example.org\r\n"
        f"To: {ADDRESS}\r\n"
        f"Subject: {subject}\r\n"
        f"Message-ID: <{message_id}>\r\n"
        "MIME-Version: 1.0\r\n"
    )
    return (head + "\r\n" + body + "\r\n").encode("utf-8")


def with_provenance(base, digest):
    """Insert the provenance header as the last top-level header."""
    separator = base.find(b"\r\n\r\n")
    line = f"{seed.PROVENANCE_HEADER}: {digest}\r\n".encode("ascii")
    return base[: separator + 2] + line + base[separator + 2 :]


def manifest_line(index, message_id, path, labels, hostile=False):
    return {
        "id": f"m{index:06d}",
        "path": path,
        "message_id": message_id,
        "thread": 1,
        "thread_pos": 1,
        "from": "sender@example.org",
        "to": ADDRESS,
        "subject": f"subj-{index}-{SUBJECT_CANARY}",
        "labels": list(labels),
        "internal_date": "2026-09-01T00:00:00Z",
        "expected": {
            "class": "notice",
            "bulk_score_min": 0,
            "bulk_score_max": 9,
            "method": "none",
            "reason": "fixture",
            "canary": f"canary-{index}",
            "hostile": hostile,
        },
    }


class SeedTestCase(unittest.TestCase):
    """A temp corpus, a temp registry and a fake Gmail server per test."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.state = self.root / "state"
        self.fake = FakeGmail()
        self.server = http.server.HTTPServer(("127.0.0.1", 0), _Handler)
        self.server.fake = self.fake
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.server.shutdown)
        port = self.server.server_address[1]
        self._saved = (seed.API_BASE, seed.TOKEN_ENDPOINT)
        seed.API_BASE = f"http://127.0.0.1:{port}{API_PREFIX}"
        seed.TOKEN_ENDPOINT = f"http://127.0.0.1:{port}/token"
        self.addCleanup(self._restore)
        self.manifest = self.root / "manifest.jsonl"
        self.registry = self.root / "synthetic_accounts.json"
        self.digest = ""

    def _restore(self):
        seed.API_BASE, seed.TOKEN_ENDPOINT = self._saved

    def corpus(
        self,
        count=2,
        labels=("INBOX", "UNREAD"),
        hostile=False,
        slice_name="demo",
        approved=None,
        registry_address=ADDRESS,
        header_persona="freelancer",
        registry_persona="freelancer",
    ):
        """Write a T-1120a-shaped manifest plus eml files, and the registry."""
        eml_dir = self.root / "eml"
        eml_dir.mkdir(parents=True, exist_ok=True)
        bases = []
        lines = []
        for index in range(1, count + 1):
            message_id = f"synth-m{index:06d}@news.example.org"
            path = f"eml/m{index:06d}.eml"
            line = manifest_line(index, message_id, path, labels, hostile=hostile and index == count)
            lines.append(json.dumps(line, sort_keys=True))
            raw = eml_without_provenance(message_id, line["subject"], f"body-{index}-{BODY_CANARY}")
            bases.append((path, raw))
        body = ("\n".join(lines) + "\n").encode("utf-8")
        digest = seed.corpus_digest(bases, body)
        for path, raw in bases:
            (self.root / path).write_bytes(with_provenance(raw, digest))
        header = {
            "corpus_hash": digest,
            "persona": header_persona,
            "seed": 1,
            "slice": slice_name,
            "generator_version": "synth-mailbox/1",
            "messages": count,
        }
        self.manifest.write_bytes((json.dumps(header) + "\n").encode("utf-8") + body)
        self.registry.write_bytes(
            json.dumps(
                {
                    "accounts": {
                        ACCOUNT: {
                            "persona": registry_persona,
                            "seed": 1,
                            "slice": slice_name,
                            "corpus_sha256": approved or digest,
                            "address_sha256": seed.address_digest(registry_address),
                        }
                    }
                }
            ).encode("utf-8")
        )
        self.digest = digest
        return digest

    def run_tool(self, mode, *, apply_changes=False, account=ACCOUNT, adopt=False, reimport=False, confirm=None):
        out, err, sleeps = io.StringIO(), io.StringIO(), []
        argv = [
            mode,
            "--account",
            account,
            "--manifest",
            str(self.manifest),
            "--registry",
            str(self.registry),
            "--state-dir",
            str(self.state),
        ]
        if apply_changes:
            argv.append("--apply")
        if adopt:
            argv.append("--adopt")
        if reimport:
            argv.append("--reimport-missing")
        code = seed.main(argv, out=out, err=err, confirm=confirm, sleep=sleeps.append, env=ENV)
        return code, out.getvalue(), err.getvalue(), sleeps

    def seed_once(self, count=2, **kwargs):
        """A corpus seeded with --apply: the state every later mode starts from."""
        self.corpus(count=count, **kwargs)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        return out


class DryRunTest(SeedTestCase):
    def test_b1_dry_run_makes_no_mutating_call(self):
        self.corpus(count=2)
        for mode in ("seed", "reset"):
            code, out, err, _ = self.run_tool(mode)
            self.assertEqual(code, 0, err)
            self.assertIn("DRY", out)
        self.assertEqual(self.fake.mutations(), [])
        self.assertEqual(self.fake.messages, {})
        # the OAuth token exchange is allowed, and it happened
        self.assertIn("/token", self.fake.paths())

    def test_b1_seed_follows_every_list_page(self):
        self.fake.page_size = 1
        self.corpus(count=3)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), 3)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), 3)


class GuardTest(SeedTestCase):
    def test_b2_unknown_account_exits_2_without_any_call(self):
        self.corpus(count=1)
        code, out, err, _ = self.run_tool("seed", apply_changes=True, account="not-in-registry")
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, [])
        self.assertIn("unknown account", err)

    def test_b2_address_hash_mismatch_exits_2_after_profile_only(self):
        self.corpus(count=1, registry_address=OTHER_ADDRESS)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.user_paths(), ["/users/me/profile"])
        self.assertEqual(self.fake.mutations(), [])
        self.assertNotIn(ADDRESS, err)

    def test_b2_profile_address_is_lowercased_before_hashing(self):
        self.corpus(count=1)
        self.fake.address = ADDRESS.upper()
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)

    def test_b9_manifest_disagreeing_with_the_registry_is_refused_before_any_write(self):
        self.corpus(count=2, approved="0" * 64)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, [])
        self.assertEqual(self.fake.mutations(), [])

    def test_b9_persona_disagreement_is_refused(self):
        self.corpus(count=1, registry_persona="student")
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, [])

    def test_b9_a_tampered_eml_changes_the_recomputed_digest(self):
        self.corpus(count=1)
        target = self.root / "eml" / "m000001.eml"
        data = target.read_bytes().replace(b"body-1", b"body-2")
        target.write_bytes(data)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, [])

    def test_b9_hostile_message_is_refused(self):
        self.corpus(count=2, hostile=True)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, [])

    def test_b9_foreign_provenance_marker_is_refused(self):
        self.corpus(count=1)
        target = self.root / "eml" / "m000001.eml"
        target.write_bytes(target.read_bytes().replace(self.digest.encode("ascii"), b"f" * 64))
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, [])


class SeedTest(SeedTestCase):
    def test_b3_seed_twice_imports_each_message_once(self):
        self.corpus(count=3)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), 3)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), 3)
        self.assertEqual(len(self.fake.messages), 3)

    def test_seed_creates_the_label_once_and_labels_the_messages(self):
        self.corpus(count=2)
        self.run_tool("seed", apply_changes=True)
        self.assertEqual(self.fake.count("/labels"), 1)
        for message in self.fake.messages.values():
            self.assertTrue(BASELINE <= message["labels"])

    def test_seed_writes_mode_600_receipts_and_never_prints_them(self):
        self.corpus(count=1)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        path = self.state / f"{ACCOUNT}.json"
        self.assertTrue(path.exists())
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertEqual(len(json.loads(path.read_bytes())["messages"]), 1)
        for message in self.fake.messages:
            self.assertNotIn(message, out)


class ResetTest(SeedTestCase):
    def test_b4_reset_restores_the_baseline_and_never_deletes(self):
        self.seed_once(count=3)
        ids = sorted(self.fake.messages)
        # the fake server changed labels, trashed messages and read some
        self.fake.set_labels(ids[0], [seed.SYNTHETIC_LABEL])
        self.fake.set_labels(ids[1], ["TRASH", "SPAM"])
        self.fake.set_labels(ids[2], ["INBOX", "UNREAD", seed.SYNTHETIC_LABEL, "CATEGORY_PROMOTIONS"])
        imports_before = self.fake.count("/messages/import")
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        for message in self.fake.messages.values():
            self.assertEqual(message["labels"], BASELINE)
        self.assertEqual(self.fake.count("/messages/import"), imports_before)
        self.assertEqual(self.fake.count("/untrash"), 1)
        for path in self.fake.paths():
            self.assertFalse(path.endswith("/delete") or path.endswith("/batchDelete"))
            self.assertFalse(path.endswith("/send"))

    def test_b5_verify_exits_1_on_drift_and_0_after_reset(self):
        self.seed_once(count=2)
        code, out, err, _ = self.run_tool("verify")
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(out.splitlines()[1]), {"present": 2, "missing": 0, "drifted": 0, "ambiguous": 0, "extra": 0})
        self.fake.set_labels(sorted(self.fake.messages)[0], ["INBOX"])
        code, out, err, _ = self.run_tool("verify")
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out.splitlines()[1])["drifted"], 1)
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        code, out, err, _ = self.run_tool("verify")
        self.assertEqual(code, 0, err)

    def test_reset_reports_missing_messages_and_reimports_on_request(self):
        self.seed_once(count=2)
        gone = self.fake.by_rfc822("synth-m000002@news.example.org")["id"]
        self.fake.remove(gone)
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertIn("missing=1", out)
        self.assertEqual(self.fake.count("/messages/import"), 2)
        code, out, err, _ = self.run_tool("reset", apply_changes=True, reimport=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), 3)

    def test_reset_counts_extra_messages_without_touching_them(self):
        self.seed_once(count=2)
        extra = self.fake.add_message("synth-m999999@news.example.org", self.digest, labels=[seed.SYNTHETIC_LABEL])
        self.fake.set_labels(extra, [seed.SYNTHETIC_LABEL, "STARRED"])
        code, out, err, _ = self.run_tool("verify")
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(out.splitlines()[1])["extra"], 1)
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.messages[extra]["labels"], {seed.SYNTHETIC_LABEL, "STARRED"})
        self.assertIn("extra=1", out)

    def test_reset_recreates_a_deleted_label(self):
        self.seed_once(count=2)
        creates_before = self.fake.count("/labels")
        self.fake.labels.clear()
        for message in self.fake.messages.values():
            message["labels"].discard(seed.SYNTHETIC_LABEL)
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/labels"), creates_before + 1)
        self.assertIn(seed.SYNTHETIC_LABEL, self.fake.labels)
        for message in self.fake.messages.values():
            self.assertEqual(message["labels"], BASELINE)

    def test_reset_without_receipts_refuses_to_modify_anything(self):
        self.corpus(count=1)
        self.run_tool("seed", apply_changes=True)
        receipts = self.state / f"{ACCOUNT}.json"
        receipts.unlink()
        before_labels = [set(m["labels"]) for m in self.fake.messages.values()]
        before_calls = len(self.fake.calls)
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 2)
        self.assertEqual([set(m["labels"]) for m in self.fake.messages.values()], before_labels)
        self.assertEqual(self.fake.mutations()[before_calls:], [])
        self.assertIn("--adopt", err)


class AdoptTest(SeedTestCase):
    def test_b9_forged_header_is_untouched_without_adopt_and_adopted_with_it(self):
        self.seed_once(count=2)
        original = self.fake.by_rfc822("synth-m000001@news.example.org")
        self.fake.remove(original["id"])
        planted = self.fake.add_message(
            "synth-m000001@news.example.org", self.digest, labels=[seed.SYNTHETIC_LABEL]
        )
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.messages[planted]["labels"], {seed.SYNTHETIC_LABEL})
        self.assertEqual(self.fake.count("/modify"), 0)

        code, out, err, _ = self.run_tool(
            "reset", apply_changes=True, adopt=True, confirm=lambda prompt: ACCOUNT
        )
        self.assertEqual(code, 0, err)
        self.assertIn("HEADER TRUST", out)
        self.assertEqual(self.fake.messages[planted]["labels"], BASELINE)

    def test_adopt_refuses_when_the_account_name_is_not_typed(self):
        self.seed_once(count=1)
        receipts = self.state / f"{ACCOUNT}.json"
        receipts.unlink()
        code, out, err, _ = self.run_tool(
            "reset", apply_changes=True, adopt=True, confirm=lambda prompt: "wrong-name"
        )
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.count("/modify"), 0)
        self.assertFalse(receipts.exists())


class TransportTest(SeedTestCase):
    def test_b6_429_and_5xx_are_retried_with_exponential_back_off(self):
        self.corpus(count=1)
        self.fake.fail_next.append(("/messages/import", 429))
        self.fake.fail_next.append(("/messages/import", 503))
        code, out, err, sleeps = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), 3)
        self.assertGreaterEqual(len(sleeps), 2)
        self.assertGreater(sleeps[0], 0)
        self.assertGreater(sleeps[1], sleeps[0])

    def test_b6_a_persistent_5xx_gives_up_with_a_content_free_error(self):
        self.corpus(count=1)
        for _ in range(seed.MAX_ATTEMPTS):
            self.fake.fail_next.append(("/messages/import", 503))
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, seed.EXIT_TRANSPORT)
        self.assertNotIn(ADDRESS, err)
        self.assertNotIn("synth-", err)

    def test_token_exchange_uses_a_refresh_token_grant(self):
        self.corpus(count=1)
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, 0, err)
        token_calls = [call for call in self.fake.calls if call["path"] == "/token"]
        self.assertEqual(len(token_calls), 1)
        self.assertEqual(token_calls[0]["body"]["grant_type"], "refresh_token")
        self.assertFalse([path for path in self.fake.paths() if "send" in path])

    def test_b6_invalid_grant_has_its_own_exit_code(self):
        self.corpus(count=1)
        self.fake.token_invalid = True
        code, out, err, _ = self.run_tool("seed", apply_changes=True)
        self.assertEqual(code, seed.EXIT_CREDENTIALS)
        self.assertIn("needs re-authorising", err)
        self.assertNotIn(FAKE_TOKEN, out + err)
        self.assertEqual(self.fake.user_paths(), [])


class NoSendTest(SeedTestCase):
    def test_scopes_never_include_send(self):
        self.assertEqual(set(seed.SCOPES), {"gmail.modify", "openid", "email"})
        self.assertFalse([scope for scope in seed.SCOPES if "send" in scope])

    def test_b10_http_layer_refuses_send_paths(self):
        http = seed.Http(base=f"{seed.API_BASE}", sleep=lambda seconds: None)
        for path in ("/users/me/messages/send", "/users/me/drafts/send"):
            with self.assertRaises(seed.SendRefused):
                http.call("POST", path, token="t", json_body={})
        self.assertEqual(self.fake.calls, [])


class PrivacyTest(SeedTestCase):
    def test_b7_no_stream_carries_a_token_address_subject_body_or_identifier(self):
        self.seed_once(count=2)
        ids = sorted(self.fake.messages)
        self.fake.set_labels(ids[0], [])
        self.fake.set_labels(ids[1], ["TRASH"])
        streams = []
        for mode, kwargs in (
            ("verify", {}),
            ("reset", {"apply_changes": True}),
            ("seed", {"apply_changes": True}),
            ("verify", {}),
            ("reset", {"apply_changes": True, "adopt": True, "confirm": lambda prompt: ACCOUNT}),
        ):
            code, out, err, _ = self.run_tool(mode, **kwargs)
            streams.append(out + err)
        self.fake.token_invalid = True
        _code, out, err, _ = self.run_tool("seed", apply_changes=True)
        streams.append(out + err)
        text = "".join(streams)
        forbidden = [
            FAKE_TOKEN,
            SUBMITTED_SECRET,
            ENV[seed.CLIENT_SECRET_VAR],
            ADDRESS,
            SUBJECT_CANARY,
            BODY_CANARY,
            self.digest,
        ]
        forbidden += list(self.fake.messages)
        forbidden += [m["thread"] for m in self.fake.messages.values()]
        forbidden += [f"synth-m{i:06d}@news.example.org" for i in (1, 2)]
        for item in forbidden:
            self.assertNotIn(item, text)


class FakeServerTest(SeedTestCase):
    def test_b8_trash_and_spam_are_hidden_unless_include_spam_trash(self):
        self.seed_once(count=2)
        for message in self.fake.messages.values():
            if message["rfc822"].endswith("m000001@news.example.org"):
                message["labels"].add("TRASH")
            else:
                message["labels"].add("SPAM")
        self.assertEqual(self.fake.list_ids(include_spam_trash=False), [])
        self.assertEqual(len(self.fake.list_ids(include_spam_trash=True)), 2)

    def test_b8_reset_restores_trashed_and_spammed_messages_without_importing(self):
        self.seed_once(count=2)
        imports_before = self.fake.count("/messages/import")
        for message in self.fake.messages.values():
            if message["rfc822"].endswith("m000001@news.example.org"):
                message["labels"].add("TRASH")
            else:
                message["labels"].add("SPAM")
        code, out, err, _ = self.run_tool("reset", apply_changes=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.fake.count("/messages/import"), imports_before)
        for message in self.fake.messages.values():
            self.assertEqual(message["labels"], BASELINE)


if __name__ == "__main__":
    unittest.main()
