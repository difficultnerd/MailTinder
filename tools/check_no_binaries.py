#!/usr/bin/env python3
"""check_no_binaries.py [--staged | --tree] [--repo DIR] -- keep unreviewed binaries out of the repository.

Why: on 2026-10-09 a 9.5 MB compiled `cargo-mutants` executable (plus its install metadata) was committed by `git add -A` and merged to
main; a review finding about it was lost between incremental reviews. A built executable is unreviewable code in a public repo, so this
check is deterministic and cheap, and it runs in three places: the pre-commit hook (`--staged`), the local gate and CI's
`language-policy` job (`--tree`, via tools/check_language_policy.sh).

Rules (a path listed in tools/binary_allowlist.txt is exempt from all of them; that file changes only through a reviewed PR):
  1. An executable by magic number (ELF, PE/MZ, Mach-O, Java class/fat binary) is never allowed.
  2. Any other file containing a NUL byte must be an allowed asset type (images, fonts, pdf).
  3. No tracked file over 1 MB (lock files excepted).
  4. No path inside a hidden tool/install directory (tools/.cargo-mutants/, tools/.bin/, .venv*, node_modules/, target/).
Exit 0 ok, 1 violations (printed as FAILED lines), 2 the check could not run (git failed / empty tree): CI treats that as a failure, the local hook fails open.
Never modifies anything.
"""
import fnmatch
import os
import subprocess
import sys

MAX_BYTES = 1024 * 1024
ASSET_EXT = {".png", ".jpg", ".jpeg", ".gif", ".ico", ".webp", ".ttf", ".otf", ".woff", ".woff2", ".pdf"}
LOCKS = {"Cargo.lock", "pubspec.lock", "package-lock.json"}
EXEC_MAGIC = (b"\x7fELF", b"MZ", b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xce", b"\xca\xfe\xba\xbe")
BAD_DIRS = (".cargo-mutants", ".bin", ".venv", ".venv-semgrep", "node_modules", "target")


class GitError(Exception):
    pass


def git(repo, *args):
    """stdout of a git command; a failing git is an ERROR (exit 2), never an empty file list that reads as 'OK, 0 files'."""
    r = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, check=False)
    if r.returncode != 0:
        raise GitError(f"git {' '.join(args[:2])} failed in {repo}: {r.stderr.strip()[:200]}")
    return r.stdout


def allowlist(repo):
    p = os.path.join(repo, "tools", "binary_allowlist.txt")
    out = []
    try:
        for line in open(p, encoding="utf-8"):
            line = line.split("#", 1)[0].strip()
            if line:
                out.append(line)
    except OSError:
        pass
    return out


def allowed(path, allow):
    return any(fnmatch.fnmatch(path, g) or path == g for g in allow)


def read_head(repo, path, staged):
    if staged:
        r = subprocess.run(["git", "-C", repo, "show", f":{path}"], capture_output=True, check=False)
        return r.stdout[:8192], len(r.stdout)
    full = os.path.join(repo, path)
    try:
        with open(full, "rb") as f:
            head = f.read(8192)
        return head, os.path.getsize(full)
    except OSError:
        return b"", 0


def main():
    args = sys.argv[1:]
    staged = "--staged" in args
    repo = os.getcwd()
    if "--repo" in args:
        repo = args[args.index("--repo") + 1]
    if staged:
        # --no-renames: a rename is reported as delete + add, so the NEW path is always scanned (a `git mv` into tools/.bin/ used to slip by)
        paths = [p for p in git(repo, "diff", "--cached", "--name-only", "--no-renames", "--diff-filter=ACMT", "-z").split("\0") if p]
    else:
        paths = [p for p in git(repo, "ls-files", "-z").split("\0") if p]
    if not staged and not paths:
        raise GitError("git ls-files returned no files; refusing to report a clean tree")
    allow = allowlist(repo)
    bad = []
    for p in paths:
        if allowed(p, allow):
            continue
        parts = p.split("/")
        hidden = [c for c in parts[:-1] if c in BAD_DIRS or c.startswith(".venv")]
        head, size = read_head(repo, p, staged)
        ext = os.path.splitext(p)[1].lower()
        if hidden:
            bad.append(f"{p}: inside a tool/install/cache directory ({hidden[0]}); these are build outputs and must never be committed")
        elif head.startswith(EXEC_MAGIC):
            bad.append(f"{p}: compiled executable ({size} bytes); built binaries are unreviewable and are never allowed")
        elif b"\0" in head and ext not in ASSET_EXT:
            bad.append(f"{p}: binary file ({size} bytes, type {ext or 'none'}) that is not an allowed asset type")
        elif size > MAX_BYTES and os.path.basename(p) not in LOCKS:
            bad.append(f"{p}: {size} bytes (over 1 MB)")
    for b in bad:
        print(f"FAILED binary-policy: {b}")
    if bad:
        print("error: remove these from the commit, or (only if truly needed and reviewed) list the path in tools/binary_allowlist.txt")
        return 1
    print(f"binary policy OK ({len(paths)} files checked{', staged' if staged else ''}).")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except GitError as e:
        print(f"ERROR binary-policy: {e}")
        sys.exit(2)
