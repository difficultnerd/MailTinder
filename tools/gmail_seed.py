#!/usr/bin/env python3
"""Seed, reset and verify a synthetic Gmail test mailbox (T-1120b).

    python3 tools/gmail_seed.py --account NAME --manifest DIR/manifest.jsonl \
        (seed|reset|verify) [--apply] [--adopt] [--reimport-missing]

`DIR` is a T-1120a generator output (``manifest.jsonl`` plus ``eml/<id>.eml``).
Without ``--apply`` every mode is a dry run that prints what it would do and
makes no mailbox-mutating request.

Boundary rules this tool lives by:

* No send code path exists, and the HTTP layer REFUSES any request whose path
  ends in ``/messages/send`` or ``/drafts/send``. Google accepts
  ``gmail.modify`` for send, so a missing ``gmail.send`` scope is not the
  boundary; the guard is. The scopes requested are ``gmail.modify``, ``openid``
  and ``email`` only.
* Nothing is ever deleted. Reset restores STATE (labels and trash), it never
  wipes and reloads.
* Output is COUNTS, OUTCOME CODES and the account NAME only. No address, token,
  subject, body, Gmail message id, thread id or RFC 822 Message-ID is printed
  on any stream, including error paths.
* Provenance is bound to what THIS tool did: ``seed`` records the Gmail ids it
  imported in a receipts file under ``~/.local/state/mt-seed/`` (mode 600,
  outside git, never printed). ``--adopt`` is HEADER TRUST, not proof of
  origin: anyone who can write to the mailbox can plant a message that passes
  it. It is a one-time, interactive step.

Stdlib only, like ``gmail_sandbox_reauth.py``.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API_BASE = "https://gmail.googleapis.com/gmail/v1"
TOKEN_ENDPOINT = "https://oauth2.googleapis.com/token"
# `gmail.modify` covers `users.messages.import`; `gmail.send` is deliberately not
# requested, because nothing here sends. The transport guard is the boundary.
SCOPES = ("gmail.modify", "openid", "email")
SYNTHETIC_LABEL = "MT Synthetic"
PROVENANCE_HEADER = "X-MT-Synthetic-Corpus"

RETRY_STATUSES = (429, 500, 502, 503, 504)
MAX_ATTEMPTS = 5
RETRY_BASE_S = 0.5
BATCH_PAUSE_S = 0.25
SEND_SUFFIXES = ("/messages/send", "/drafts/send")

EXIT_OK = 0
EXIT_DRIFT = 1
EXIT_REFUSED = 2
EXIT_CREDENTIALS = 3
EXIT_TRANSPORT = 4

CRLF = b"\r\n"
SEPARATOR = b"\r\n\r\n"
RESERVED_SUFFIXES = (".example.com", ".example.net", ".example.org", ".test", ".invalid")
RESERVED_EXACT = ("example.com", "example.net", "example.org")
MESSAGE_ID_RE = re.compile(r"^synth-[A-Za-z0-9]+@[A-Za-z0-9.-]+$")
ACCOUNT_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")

CLIENT_ID_VAR = "GMAIL_SEED_CLIENT_ID"
CLIENT_SECRET_VAR = "GMAIL_SEED_CLIENT_SECRET"
REFRESH_TOKEN_VAR = "GMAIL_SEED_REFRESH_TOKEN"

DEFAULT_REGISTRY = os.path.join(os.path.dirname(os.path.abspath(__file__)), "synthetic_accounts.json")
DEFAULT_STATE_DIR = os.path.join(os.path.expanduser("~"), ".local", "state", "mt-seed")


class ToolError(Exception):
    """A failure with a fixed, content-free message and its own exit code."""

    def __init__(self, message: str, code: int = EXIT_REFUSED) -> None:
        super().__init__(message)
        self.message = message
        self.code = code


class SendRefused(ToolError):
    """A request to a send endpoint. The tool has no reason to make one."""

    def __init__(self, message: str = "refused: this tool never sends") -> None:
        super().__init__(message, EXIT_REFUSED)


class CredentialsError(ToolError):
    """`invalid_grant`: Google no longer accepts the refresh token."""

    def __init__(self) -> None:
        super().__init__(
            "needs re-authorising: the Gmail credentials were rejected (invalid_grant)",
            EXIT_CREDENTIALS,
        )


# --------------------------------------------------------------------------- #
# Byte-level helpers
# --------------------------------------------------------------------------- #


def sha256_hex(data: bytes) -> str:
    """Lower-case hex SHA-256."""
    return hashlib.sha256(data).hexdigest()


def strip_provenance(data: bytes) -> bytes:
    """Remove the provenance header line, giving back the pre-hash bytes.

    Mirrors ``backend/crates/testkit/src/synth/render.rs``: only the last
    top-level header before the CRLFCRLF separator is removed, byte-exactly.
    """
    sep = data.find(SEPARATOR)
    if sep < 0:
        return data
    head = data[:sep]
    last = head.rfind(CRLF)
    if last < 0:
        return data
    if not head[last + 2 :].startswith(PROVENANCE_HEADER.encode("ascii")):
        return data
    return head[:last] + data[sep:]


def header_value(data: bytes, name: str) -> str | None:
    """The value of a top-level header, or None. Never decodes the body."""
    sep = data.find(SEPARATOR)
    head = data[:sep] if sep >= 0 else data
    prefix = name.encode("ascii") + b":"
    for line in head.split(CRLF):
        if line.startswith(prefix):
            return line[len(prefix) :].strip().decode("utf-8", "replace")
    return None


def corpus_digest(eml_bases, manifest_body: bytes) -> str:
    """The T-1120a `corpus_hash` rule.

    `eml_bases` are (relative path, header-stripped bytes) pairs. Every file is
    hashed, the manifest body (no first line) is hashed, the lines
    `<path> <digest>` are sorted by path and joined with `\\n`, and the result
    is hashed again.
    """
    lines = [(path, sha256_hex(data)) for path, data in eml_bases]
    lines.append(("manifest.jsonl", sha256_hex(manifest_body)))
    lines.sort(key=lambda pair: pair[0])
    joined = "\n".join(f"{path} {digest}" for path, digest in lines)
    return sha256_hex(joined.encode("ascii"))


def is_reserved_host(host: str) -> bool:
    """True for `example.com/net/org`, `*.test` and `*.invalid` only."""
    host = host.lower()
    if host in RESERVED_EXACT:
        return True
    return host.endswith(RESERVED_SUFFIXES)


def rfc822_id_shape_ok(message_id: str) -> bool:
    """`<synth-...@reserved-host>`'s bare id, as the generator writes it."""
    if not MESSAGE_ID_RE.match(message_id):
        return False
    return is_reserved_host(message_id.split("@", 1)[1])


def address_digest(address: str) -> str:
    """The registry's `address_sha256`: the lowercased address, hashed."""
    return sha256_hex(address.strip().lower().encode("utf-8"))


# --------------------------------------------------------------------------- #
# Manifest and registry
# --------------------------------------------------------------------------- #


def load_registry(path: str) -> dict:
    """The registry's `accounts` map. Only hashes live here, never addresses."""
    try:
        with open(path, "rb") as handle:
            data = json.loads(handle.read())
    except FileNotFoundError:
        raise ToolError("the account registry is missing", EXIT_REFUSED) from None
    except (OSError, ValueError):
        raise ToolError("the account registry is unreadable", EXIT_REFUSED) from None
    accounts = data.get("accounts")
    if not isinstance(accounts, dict):
        raise ToolError("the account registry has no accounts map", EXIT_REFUSED)
    return accounts


def read_manifest(path: str):
    """(header, messages, body) from `manifest.jsonl`. Body is the raw bytes
    after the first line, which is what `corpus_hash` covers."""
    try:
        with open(path, "rb") as handle:
            raw = handle.read()
    except OSError:
        raise ToolError("the manifest is unreadable", EXIT_REFUSED) from None
    newline = raw.find(b"\n")
    first = raw if newline < 0 else raw[:newline]
    body = b"" if newline < 0 else raw[newline + 1 :]
    try:
        header = json.loads(first)
    except ValueError:
        raise ToolError("the manifest header is not JSON", EXIT_REFUSED) from None
    if not isinstance(header, dict):
        raise ToolError("the manifest header is not an object", EXIT_REFUSED)
    messages = []
    for line in body.splitlines():
        if not line.strip():
            continue
        try:
            messages.append(json.loads(line))
        except ValueError:
            raise ToolError("a manifest line is not JSON", EXIT_REFUSED) from None
    return header, messages, body


def validate_manifest(entry: dict, header: dict, messages: list, body: bytes, directory: str) -> str:
    """The whole guard, before any write. Returns the corpus digest.

    The registry holds the approved digest: a digest that only the manifest
    supplies is never trusted, and the digest is recomputed from the files with
    the exact T-1120a rule before anything is written.
    """
    for field in ("persona", "seed", "slice"):
        if header.get(field) != entry.get(field):
            raise ToolError("the manifest header disagrees with the registry", EXIT_REFUSED)
    approved = entry.get("corpus_sha256")
    if not isinstance(approved, str) or not approved:
        raise ToolError("the registry has no corpus digest for this account", EXIT_REFUSED)
    if header.get("corpus_hash") != approved:
        raise ToolError("the manifest header disagrees with the registry", EXIT_REFUSED)
    if not entry.get("address_sha256"):
        raise ToolError("the registry has no address digest for this account", EXIT_REFUSED)

    eml_bases = []
    seen_paths = set()
    for index, message in enumerate(messages):
        message_id = message.get("message_id")
        if not isinstance(message_id, str) or not rfc822_id_shape_ok(message_id):
            raise ToolError(f"manifest message {index} has a foreign Message-ID", EXIT_REFUSED)
        expected = message.get("expected")
        if isinstance(expected, dict) and expected.get("hostile"):
            raise ToolError(f"manifest message {index} is hostile and is never loaded", EXIT_REFUSED)
        path = message.get("path")
        if not isinstance(path, str) or not path.startswith("eml/") or ".." in path.split("/"):
            raise ToolError(f"manifest message {index} has a bad path", EXIT_REFUSED)
        if path in seen_paths:
            raise ToolError(f"manifest message {index} repeats a path", EXIT_REFUSED)
        seen_paths.add(path)
        full = os.path.join(directory, *path.split("/"))
        try:
            with open(full, "rb") as handle:
                data = handle.read()
        except OSError:
            raise ToolError("a manifest message file is unreadable", EXIT_REFUSED) from None
        if header_value(data, PROVENANCE_HEADER) != approved:
            raise ToolError("a manifest message carries a foreign provenance marker", EXIT_REFUSED)
        eml_bases.append((path, strip_provenance(data)))

    eml_dir = os.path.join(directory, "eml")
    try:
        on_disk = {f"eml/{name}" for name in os.listdir(eml_dir)}
    except OSError:
        on_disk = set()
    if on_disk != seen_paths:
        raise ToolError("the eml directory and the manifest disagree", EXIT_REFUSED)

    digest = corpus_digest(eml_bases, body)
    if digest != approved:
        raise ToolError("the corpus digest does not match the approved digest", EXIT_REFUSED)
    return digest


def receipts_path(state_dir: str, account: str) -> str:
    """`~/.local/state/mt-seed/<account>.json`."""
    return os.path.join(state_dir, f"{account}.json")


def read_receipts(path: str):
    """The receipts, or None when this runner has never seeded. Never printed."""
    try:
        with open(path, "rb") as handle:
            data = json.loads(handle.read())
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        raise ToolError("the receipts file is unreadable", EXIT_REFUSED) from None
    if not isinstance(data, dict) or not isinstance(data.get("messages"), dict):
        raise ToolError("the receipts file is unreadable", EXIT_REFUSED)
    return data


def write_receipts(path: str, account: str, digest: str, messages: dict) -> None:
    """Write mode 600, outside git, and never print a word of it."""
    payload = {"account": account, "corpus_sha256": digest, "messages": messages}
    directory = os.path.dirname(path)
    if directory:
        os.makedirs(directory, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        os.write(descriptor, json.dumps(payload, sort_keys=True).encode("utf-8"))
    finally:
        os.close(descriptor)
    os.chmod(path, 0o600)


# --------------------------------------------------------------------------- #
# HTTP layer (the send boundary)
# --------------------------------------------------------------------------- #


def refuse_send(path: str) -> None:
    """The boundary: no request to a send endpoint ever leaves this module."""
    for suffix in SEND_SUFFIXES:
        if path.endswith(suffix):
            raise SendRefused()


class Http:
    """The Gmail REST calls this tool is allowed to make, with back-off."""

    def __init__(self, base: str = API_BASE, opener=None, sleep=None, max_attempts: int = MAX_ATTEMPTS) -> None:
        self.base = base
        self.opener = opener or urllib.request.urlopen
        self.sleep = sleep or time.sleep
        self.max_attempts = max_attempts

    def call(self, method: str, path: str, token: str | None = None, params=None, json_body=None) -> dict:
        """One call. Retries 429 and 5xx with exponential back-off."""
        refuse_send(path)
        url = self.base + path
        if params:
            url += "?" + urllib.parse.urlencode(params, doseq=True)
        data = None
        headers = {}
        if json_body is not None:
            data = json.dumps(json_body).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if token is not None:
            headers["Authorization"] = "Bearer " + token
        attempt = 0
        while True:
            request = urllib.request.Request(url, data=data, headers=headers, method=method)
            try:
                with self.opener(request, timeout=60) as response:
                    payload = response.read()
                    return json.loads(payload) if payload else {}
            except urllib.error.HTTPError as error:
                # The exception text and the URL are never surfaced: they can
                # carry the query, which carries an RFC 822 Message-ID.
                if error.code in RETRY_STATUSES and attempt + 1 < self.max_attempts:
                    attempt += 1
                    self.sleep(RETRY_BASE_S * (2**attempt))
                    continue
                raise ToolError(f"gmail api error (HTTP {error.code})", EXIT_TRANSPORT) from None
            except OSError:
                raise ToolError("gmail api unreachable", EXIT_TRANSPORT) from None

    def pause(self) -> None:
        """The pause between the items of a batch of imports."""
        self.sleep(BATCH_PAUSE_S)


def mint_access_token(
    client_id: str,
    client_secret: str,
    refresh_token: str,
    token_endpoint: str = TOKEN_ENDPOINT,
    opener=None,
) -> str:
    """`grant_type=refresh_token`; `invalid_grant` gets its own outcome."""
    opener = opener or urllib.request.urlopen
    body = urllib.parse.urlencode(
        {
            "grant_type": "refresh_token",
            "client_id": client_id,
            "client_secret": client_secret,
            "refresh_token": refresh_token,
        }
    ).encode("ascii")
    request = urllib.request.Request(
        token_endpoint,
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        with opener(request, timeout=30) as response:
            data = json.loads(response.read())
    except urllib.error.HTTPError as error:
        if error.code == 400:
            try:
                payload = json.loads(error.read())
            except (ValueError, OSError):
                payload = {}
            if payload.get("error") == "invalid_grant":
                raise CredentialsError() from None
        raise ToolError(f"the token endpoint answered HTTP {error.code}", EXIT_TRANSPORT) from None
    except OSError:
        raise ToolError("the token endpoint is unreachable", EXIT_TRANSPORT) from None
    token = data.get("access_token")
    if not token:
        raise ToolError("the token endpoint returned no access token", EXIT_TRANSPORT)
    return token


def credentials_from_env(env) -> tuple[str, str, str]:
    """The three per-account values, which the workflow maps in."""
    missing = [
        name
        for name in (CLIENT_ID_VAR, CLIENT_SECRET_VAR, REFRESH_TOKEN_VAR)
        if not env.get(name, "").strip()
    ]
    if missing:
        raise ToolError("the GMAIL_SEED_* credentials are not set", EXIT_REFUSED)
    return env[CLIENT_ID_VAR], env[CLIENT_SECRET_VAR], env[REFRESH_TOKEN_VAR]


# --------------------------------------------------------------------------- #
# Gmail calls
# --------------------------------------------------------------------------- #


def list_pages(http: Http, token: str, params: dict) -> list:
    """Every page of `messages.list` (`nextPageToken` is always followed)."""
    ids: list = []
    page = None
    while True:
        query = dict(params)
        if page:
            query["pageToken"] = page
        data = http.call("GET", "/users/me/messages", token=token, params=query)
        for item in data.get("messages", []):
            identifier = item.get("id")
            if identifier:
                ids.append(identifier)
        page = data.get("nextPageToken")
        if not page:
            return ids


def lookup_by_message_id(http: Http, token: str, message_id: str) -> list:
    """Gmail ids whose RFC 822 id matches, in trash and spam included."""
    return list_pages(
        http,
        token,
        {"q": f"rfc822msgid:{message_id}", "includeSpamTrash": "true", "maxResults": "500"},
    )


def message_metadata(http: Http, token: str, gmail_id: str) -> dict:
    """`format=metadata`: label ids and the two headers the tool reads only."""
    return http.call(
        "GET",
        f"/users/me/messages/{gmail_id}",
        token=token,
        params={"format": "metadata", "metadataHeaders": ["Message-ID", PROVENANCE_HEADER]},
    )


def metadata_header(metadata: dict, name: str) -> str | None:
    """A header value out of a `format=metadata` payload."""
    for header in (metadata.get("payload") or {}).get("headers", []):
        if header.get("name") == name:
            return header.get("value")
    return None


def labels_by_name(http: Http, token: str) -> dict:
    """Every label's name mapped to its id. A read, so a dry run may call it.

    The REST API takes ids everywhere (`labelIds` on import, modify and list):
    a label name is only ever a lookup key, never a value put on the wire.
    """
    data = http.call("GET", "/users/me/labels", token=token)
    mapping = {}
    for label in data.get("labels", []):
        name = label.get("name")
        identifier = label.get("id")
        if isinstance(name, str) and identifier:
            mapping[name] = identifier
    return mapping


def create_label(http: Http, token: str) -> str:
    """Create the label once, returning its id. Never called without `--apply`."""
    created = http.call("POST", "/users/me/labels", token=token, json_body={"name": SYNTHETIC_LABEL})
    identifier = created.get("id")
    if not identifier:
        raise ToolError("the gmail api returned no label id", EXIT_TRANSPORT)
    return identifier


def import_message(http: Http, token: str, raw: bytes, label_ids: list) -> str:
    """`users.messages.import`: the only way seed mail reaches the mailbox."""
    body = {
        "raw": base64.urlsafe_b64encode(raw).decode("ascii"),
        "internalDateSource": "dateHeader",
        "neverMarkSpam": True,
        "labelIds": label_ids,
    }
    imported = http.call("POST", "/users/me/messages/import", token=token, json_body=body)
    return imported.get("id", "")


def modify_labels(http: Http, token: str, gmail_id: str, add: list, remove: list) -> None:
    """Set a message's label set toward the baseline. Never deletes."""
    http.call(
        "POST",
        f"/users/me/messages/{gmail_id}/modify",
        token=token,
        json_body={"addLabelIds": sorted(add), "removeLabelIds": sorted(remove)},
    )


def untrash_message(http: Http, token: str, gmail_id: str) -> None:
    http.call("POST", f"/users/me/messages/{gmail_id}/untrash", token=token)


# --------------------------------------------------------------------------- #
# Planning
# --------------------------------------------------------------------------- #


def baseline_labels(message: dict) -> set:
    """The manifest's labels plus `MT Synthetic`; TRASH and SPAM are never in it."""
    labels = {name for name in message.get("labels", []) if isinstance(name, str)}
    labels = {name for name in labels if name not in ("TRASH", "SPAM")}
    labels.add(SYNTHETIC_LABEL)
    return labels


class Context:
    """Everything one run needs that is not per-message."""

    def __init__(
        self,
        http: Http,
        token: str,
        account: str,
        digest: str,
        messages: list,
        directory: str,
        receipts,
        label_id,
        labels=None,
        state_path: str = "",
    ):
        self.http = http
        self.token = token
        self.account = account
        self.digest = digest
        self.messages = messages
        self.directory = directory
        self.receipts = receipts
        self.label_id = label_id
        # Label *names* to ids. The API addresses labels by id, so every name
        # that reaches import or modify is translated first.
        self.labels = dict(labels or {})
        self.state_path = state_path

    def label_ids(self, names) -> set:
        """Manifest label names as the Gmail label ids the API takes."""
        return {self.labels.get(name, name) for name in names}

    def remember_created_label(self, label_id: str) -> None:
        """Record the id `create_label` just minted so its name still maps."""
        self.label_id = label_id
        self.labels[SYNTHETIC_LABEL] = label_id

    def eml_bytes(self, message: dict) -> bytes:
        path = os.path.join(self.directory, *message["path"].split("/"))
        with open(path, "rb") as handle:
            return handle.read()


def resolve(ctx: Context, message: dict, receipts, adopt: bool):
    """(status, gmail_id, metadata) for one manifest message.

    `status` is `missing`, `ambiguous` or `accepted`. Ambiguous messages are
    skipped and counted, never modified.
    """
    ids = lookup_by_message_id(ctx.http, ctx.token, message["message_id"])
    if not ids:
        return "missing", None, None
    if len(ids) > 1:
        return "ambiguous", None, None
    gmail_id = ids[0]
    metadata = message_metadata(ctx.http, ctx.token, gmail_id)
    if metadata_header(metadata, PROVENANCE_HEADER) != ctx.digest:
        return "ambiguous", None, None
    labels = set(metadata.get("labelIds") or [])
    if adopt:
        # Header trust, and the label. Documented, never hidden.
        if ctx.label_id not in labels:
            return "ambiguous", None, None
        return "accepted", gmail_id, metadata
    if receipts is None:
        # The workflow path: no receipts on an ephemeral runner, so the
        # `--adopt` check stands in. The spec accepts that weaker guarantee
        # instead of uploading receipts anywhere.
        if ctx.label_id not in labels:
            return "ambiguous", None, None
        return "accepted", gmail_id, metadata
    if receipts["messages"].get(message["message_id"]) != gmail_id:
        return "ambiguous", None, None
    return "accepted", gmail_id, metadata


def resolve_all(ctx: Context, receipts, adopt: bool):
    """Classify every manifest message once."""
    accepted, missing, ambiguous = [], [], []
    resolved_ids = set()
    for message in ctx.messages:
        status, gmail_id, metadata = resolve(ctx, message, receipts, adopt)
        if gmail_id:
            resolved_ids.add(gmail_id)
        if status == "accepted":
            accepted.append((message, gmail_id, metadata))
        elif status == "missing":
            missing.append(message)
        else:
            ambiguous.append(message)
    return accepted, missing, ambiguous, resolved_ids


def count_extra(ctx: Context, resolved_ids: set) -> int:
    """Messages carrying the label that the manifest does not describe. Counted
    only: never touched and never listed."""
    if not ctx.label_id:
        return 0
    labelled = set(
        list_pages(
            ctx.http,
            ctx.token,
            {"labelIds": ctx.label_id, "includeSpamTrash": "true", "maxResults": "500"},
        )
    )
    return len(labelled - resolved_ids)


# --------------------------------------------------------------------------- #
# Modes
# --------------------------------------------------------------------------- #


def report(out, account: str, outcome: str, counts: dict) -> None:
    """COUNTS, an OUTCOME CODE and the account NAME only."""
    print(f"account {account}", file=out)
    print("counts " + " ".join(f"{key}={value}" for key, value in sorted(counts.items())), file=out)
    print(f"outcome {outcome}", file=out)


def run_seed(ctx: Context, apply_changes: bool, out) -> int:
    """Make sure the label exists, then import every message not already there."""
    imported = 0
    present = 0
    label_created = False
    if ctx.label_id is None:
        if apply_changes:
            ctx.remember_created_label(create_label(ctx.http, ctx.token))
            label_created = True
        else:
            label_created = True  # planned
    receipts = dict(ctx.receipts["messages"]) if ctx.receipts else {}
    for message in ctx.messages:
        if lookup_by_message_id(ctx.http, ctx.token, message["message_id"]):
            present += 1
            continue
        imported += 1
        if not apply_changes:
            continue
        label_ids = sorted(ctx.label_ids(baseline_labels(message)))
        gmail_id = import_message(ctx.http, ctx.token, ctx.eml_bytes(message), label_ids)
        receipts[message["message_id"]] = gmail_id
        ctx.http.pause()
    if apply_changes and imported:
        write_receipts(receipts_path_for(ctx), ctx.account, ctx.digest, receipts)
    report(
        out,
        ctx.account,
        "SEED-OK" if apply_changes else "SEED-DRY",
        {"imported": imported, "present": present, "label_created": int(label_created)},
    )
    return EXIT_OK


def run_reset(ctx: Context, apply_changes: bool, adopt: bool, reimport: bool, out, confirm) -> int:
    """Restore the baseline: untrash, fix labels, re-import what is gone."""
    if apply_changes and ctx.receipts is None and not adopt:
        # The spec's guard: with no receipts, nothing is modified until the
        # operator accepts header trust once, on purpose.
        raise ToolError(
            "no receipts for this runner; reset refuses to modify anything without --adopt",
            EXIT_REFUSED,
        )
    receipts = ctx.receipts
    accepted, missing, ambiguous, resolved_ids = resolve_all(ctx, receipts, adopt)
    extra = count_extra(ctx, resolved_ids)

    adopted_ids = {}
    if adopt:
        print(
            "adopt is HEADER TRUST, not proof of origin: anyone who can write to "
            "this mailbox can plant a message that passes the check.",
            file=out,
        )
        print(f"account {ctx.account} counts would-adopt={len(accepted)}", file=out)
        typed = (confirm or input)(f"Type the account name ({ctx.account}) to adopt: ")
        if (typed or "").strip() != ctx.account:
            raise ToolError("adopt refused: the account name did not match", EXIT_REFUSED)
        adopted_ids = {message["message_id"]: gmail_id for message, gmail_id, _ in accepted}

    if ctx.label_id is None and apply_changes:
        ctx.remember_created_label(create_label(ctx.http, ctx.token))

    restored = 0
    untrashed = 0
    for message, gmail_id, metadata in accepted:
        target = ctx.label_ids(baseline_labels(message))
        current = set(metadata.get("labelIds") or [])
        if "TRASH" in current:
            untrashed += 1
            if apply_changes:
                untrash_message(ctx.http, ctx.token, gmail_id)
            current.discard("TRASH")
        add = target - current
        remove = current - target
        if add or remove:
            restored += 1
            if apply_changes:
                modify_labels(ctx.http, ctx.token, gmail_id, sorted(add), sorted(remove))

    reimported = 0
    if reimport and missing:
        if ctx.label_id is None and apply_changes:
            ctx.label_id = create_label(ctx.http, ctx.token)
        for message in missing:
            reimported += 1
            if not apply_changes:
                continue
            gmail_id = import_message(
                ctx.http, ctx.token, ctx.eml_bytes(message), sorted(ctx.label_ids(baseline_labels(message)))
            )
            adopted_ids[message["message_id"]] = gmail_id
            ctx.http.pause()

    if apply_changes and adopted_ids:
        merged = dict(receipts["messages"]) if receipts else {}
        merged.update(adopted_ids)
        write_receipts(receipts_path_for(ctx), ctx.account, ctx.digest, merged)

    report(
        out,
        ctx.account,
        "RESET-OK" if apply_changes else "RESET-DRY",
        {
            "accepted": len(accepted),
            "restored": restored,
            "untrashed": untrashed,
            "missing": len(missing),
            "ambiguous": len(ambiguous),
            "extra": extra,
            "reimported": reimported,
        },
    )
    return EXIT_OK


def run_verify(ctx: Context, out) -> int:
    """Compare the mailbox to the manifest; exit 1 on any drift."""
    accepted, missing, ambiguous, resolved_ids = resolve_all(ctx, ctx.receipts, adopt=False)
    drifted = 0
    present = 0
    for message, _gmail_id, metadata in accepted:
        current = set(metadata.get("labelIds") or [])
        if current == ctx.label_ids(baseline_labels(message)):
            present += 1
        else:
            drifted += 1
    extra = count_extra(ctx, resolved_ids)
    summary = {
        "present": present,
        "missing": len(missing),
        "drifted": drifted,
        "ambiguous": len(ambiguous),
        "extra": extra,
    }
    print(f"account {ctx.account}", file=out)
    print(json.dumps(summary, sort_keys=True), file=out)
    drift = summary["missing"] + summary["drifted"] + summary["ambiguous"]
    print(f"outcome {'VERIFY-DRIFT' if drift else 'VERIFY-OK'}", file=out)
    return EXIT_DRIFT if drift else EXIT_OK


def receipts_path_for(ctx: Context) -> str:
    return ctx.state_path


# --------------------------------------------------------------------------- #
# Command line
# --------------------------------------------------------------------------- #


def parse_args(argv):
    parser = argparse.ArgumentParser(
        prog="gmail_seed.py",
        description="Seed, reset and verify a synthetic Gmail test mailbox (T-1120b).",
    )
    parser.add_argument("mode", choices=("seed", "reset", "verify"))
    parser.add_argument("--account", required=True, help="registry account name")
    parser.add_argument("--manifest", required=True, help="DIR/manifest.jsonl from T-1120a")
    parser.add_argument("--apply", action="store_true", help="make the changes; default is a dry run")
    parser.add_argument("--adopt", action="store_true", help="accept existing messages on header trust (interactive)")
    parser.add_argument("--reimport-missing", action="store_true", dest="reimport_missing")
    parser.add_argument("--registry", default=DEFAULT_REGISTRY, help="account registry JSON")
    parser.add_argument("--state-dir", default=None, help="where the receipts file lives")
    return parser.parse_args(argv)


def run(args, out, err, confirm, opener, sleep, env) -> int:
    """The whole run, once the arguments are parsed."""
    account = args.account
    if not ACCOUNT_RE.match(account):
        raise ToolError("the account name is not a registry name", EXIT_REFUSED)
    accounts = load_registry(args.registry)
    entry = accounts.get(account)
    if entry is None:
        # Before the token, before any mailbox call.
        raise ToolError("unknown account", EXIT_REFUSED)

    header, messages, body = read_manifest(args.manifest)
    digest = validate_manifest(entry, header, messages, body, os.path.dirname(args.manifest) or ".")

    http = Http(base=API_BASE, opener=opener, sleep=sleep)
    client_id, client_secret, refresh = credentials_from_env(env)
    token = mint_access_token(client_id, client_secret, refresh, token_endpoint=TOKEN_ENDPOINT, opener=http.opener)

    # getProfile is the first and, on refusal, the only mailbox call.
    profile = http.call("GET", "/users/me/profile", token=token)
    address = profile.get("emailAddress") or ""
    if not address or address_digest(address) != entry.get("address_sha256"):
        raise ToolError("the profile address does not match the registry", EXIT_REFUSED)

    state_dir = args.state_dir or DEFAULT_STATE_DIR
    state_path = receipts_path(state_dir, account)
    receipts = read_receipts(state_path)
    labels = labels_by_name(http, token)
    label_id = labels.get(SYNTHETIC_LABEL)

    ctx = Context(
        http,
        token,
        account,
        digest,
        messages,
        os.path.dirname(args.manifest) or ".",
        receipts,
        label_id,
        labels,
        state_path,
    )

    if args.mode == "seed":
        return run_seed(ctx, args.apply, out)
    if args.mode == "reset":
        return run_reset(ctx, args.apply, args.adopt, args.reimport_missing, out, confirm)
    return run_verify(ctx, out)


def main(argv=None, *, out=None, err=None, confirm=None, opener=None, sleep=None, env=None) -> int:
    """CLI entry point. 0 ok, 1 drift, 2 refused, 3 needs re-authorising."""
    out = out if out is not None else sys.stdout
    err = err if err is not None else sys.stderr
    env = env if env is not None else os.environ
    args = parse_args(argv)
    try:
        return run(args, out, err, confirm, opener, sleep, env)
    except ToolError as error:
        print(f"error: {error.message}", file=err)
        return error.code


if __name__ == "__main__":
    sys.exit(main())
