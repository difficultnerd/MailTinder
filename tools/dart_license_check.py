#!/usr/bin/env python3
"""Check licences of resolved Dart/Flutter dependencies against an allowlist.

Dart has no cargo-deny equivalent, so this reads app/.dart_tool/package_config.json
(created by `flutter pub get`), finds each package's LICENSE file, classifies it by
text signature, and fails on anything outside the allowlist in
tools/dart_license_policy.json. Standard library only.
"""
import json
import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parent.parent
APP = ROOT / "app"
POLICY = json.loads((ROOT / "tools" / "dart_license_policy.json").read_text())

# Order matters: copyleft signatures are checked first so they always win.
SIGNATURES = [
    ("GPL", r"gnu (affero |lesser |library )?general public license"),
    ("MPL-2.0", r"mozilla public license"),
    ("Apache-2.0", r"apache license[\s,]+version 2\.0"),
    ("MIT", r"permission is hereby granted, free of charge"),
    ("BSD", r"redistribution and use in source and binary forms"),
    ("ISC", r"permission to use, copy, modify, and/or distribute this software"),
    ("Zlib", r"this software is provided 'as-is'"),
    ("Unlicense", r"this is free and unencumbered software"),
]


def classify(text: str) -> str:
    flat = re.sub(r"\s+", " ", text.lower())
    for name, pattern in SIGNATURES:
        if re.search(pattern, flat):
            return name
    return "UNKNOWN"


def package_dir(config_path: Path, root_uri: str) -> Path:
    parsed = urlparse(root_uri)
    if parsed.scheme == "file":
        return Path(unquote(parsed.path))
    return (config_path.parent / unquote(root_uri)).resolve()


def find_license(directory: Path, search_parents: bool = False) -> Path | None:
    """Return the package's licence file; optionally also look in parent directories."""
    candidates = [directory, *(directory.parents if search_parents else [])]
    for folder in candidates:
        for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "license*"):
            for candidate in sorted(folder.glob(pattern)):
                if candidate.is_file():
                    return candidate
    return None


def sdk_packages() -> set[str]:
    """Names of packages that pubspec.lock marks as coming from the Flutter SDK."""
    lock = APP / "pubspec.lock"
    if not lock.exists():
        return set()
    text = lock.read_text()
    return set(re.findall(r"^  (\S+):\n(?:    .*\n)*?    source: sdk\n", text, re.M))


def main() -> int:
    config_path = APP / ".dart_tool" / "package_config.json"
    if not config_path.exists():
        print("Missing app/.dart_tool/package_config.json; run `flutter pub get` in app/.")
        return 2

    allowed = set(POLICY["allow"])
    overrides = POLICY.get("overrides", {})
    packages = json.loads(config_path.read_text())["packages"]
    failures = []
    from_sdk = sdk_packages()

    for pkg in sorted(packages, key=lambda p: p["name"]):
        name = pkg["name"]
        directory = package_dir(config_path, pkg["rootUri"])
        if directory == APP.resolve():
            continue  # the app itself
        if name in overrides:
            licence = overrides[name]
        else:
            # Flutter SDK packages share one LICENSE at the SDK root, so search upwards for them.
            licence_file = find_license(directory, search_parents=name in from_sdk)
            if licence_file is None:
                licence = "MISSING"
            else:
                licence = classify(licence_file.read_text(errors="replace"))
        ok = licence in allowed
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {licence}")
        if not ok:
            failures.append((name, licence))

    if failures:
        print(f"\n{len(failures)} package(s) outside the allowlist {sorted(allowed)}.")
        print("Review each one. If acceptable, add it to 'overrides' in "
              "tools/dart_license_policy.json with the licence you verified.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
