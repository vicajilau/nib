#!/usr/bin/env python3
"""Generate or verify cargo-sources.json from Cargo.lock.

A Flatpak build has no network, so every crate has to be listed in the manifest as an
explicit source. cargo-sources.json is that list, and it is a pure function of Cargo.lock:
per crate, one archive entry pointing at static.crates.io plus the .cargo-checksum.json that
a vendor directory requires, and finally the .cargo config redirecting crates-io at that
directory.

Because it is checked in, it goes stale the moment Cargo.lock moves without it, which is
what every Dependabot bump does. The Flatpak build then fails minutes in, with cargo saying
"failed to select a version for the requirement ... perhaps a crate was updated and forgotten
to be re-vendored?". --check catches that same drift in about a second, and says what to run.

Only registry (crates.io) dependencies are handled. A git dependency is reported as an error
rather than quietly skipped: that case needs the upstream flatpak-cargo-generator, which can
resolve git revisions.

Usage:
    build-aux/cargo-sources.py            # rewrite cargo-sources.json
    build-aux/cargo-sources.py --check    # exit 1 if it is out of sync
"""

import argparse
import json
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_LOCK = REPO_ROOT / "Cargo.lock"
CARGO_SOURCES = REPO_ROOT / "cargo-sources.json"

VENDOR_DIR = "cargo/vendor"
CARGO_CONFIG = (
    '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
    f'[source.vendored-sources]\ndirectory = "{VENDOR_DIR}"\n'
)


def build_sources(lock_path: Path) -> list[dict]:
    """Turns a Cargo.lock into the flatpak source list its vendored build needs."""
    with lock_path.open("rb") as f:
        lock = tomllib.load(f)

    git_packages = [
        p["name"]
        for p in lock.get("package", [])
        if str(p.get("source", "")).startswith("git+")
    ]
    if git_packages:
        raise SystemExit(
            f"{lock_path.name} has git dependencies ({', '.join(git_packages)}), which this "
            "script does not handle. Use the upstream flatpak-cargo-generator instead."
        )

    # Local path dependencies (the crate itself, workspace members) carry no checksum and are
    # not vendored.
    packages = sorted(
        (p for p in lock.get("package", []) if "checksum" in p),
        key=lambda p: (p["name"], p["version"]),
    )

    sources: list[dict] = []
    for pkg in packages:
        name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
        dest = f"{VENDOR_DIR}/{name}-{version}"
        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": dest,
            }
        )
        # cargo refuses to use a vendored crate that has no checksum file next to it.
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}),
                "dest": dest,
                "dest-filename": ".cargo-checksum.json",
            }
        )

    sources.append(
        {
            "type": "inline",
            "contents": CARGO_CONFIG,
            "dest": "cargo",
            "dest-filename": "config",
        }
    )
    return sources


def render(sources: list[dict]) -> str:
    return json.dumps(sources, indent=4) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify cargo-sources.json matches Cargo.lock instead of rewriting it",
    )
    args = parser.parse_args()

    generated = render(build_sources(CARGO_LOCK))

    if not args.check:
        CARGO_SOURCES.write_text(generated)
        print(f"Wrote {CARGO_SOURCES.relative_to(REPO_ROOT)} from Cargo.lock")
        return 0

    current = CARGO_SOURCES.read_text() if CARGO_SOURCES.exists() else ""
    if current == generated:
        print("cargo-sources.json is in sync with Cargo.lock")
        return 0

    # Report the drift in terms of crates, which is what a reader can act on, rather than as
    # a diff of a 300-entry JSON file.
    def crates(text: str) -> set[str]:
        if not text:
            return set()
        return {
            e["dest"].removeprefix(f"{VENDOR_DIR}/")
            for e in json.loads(text)
            if e["type"] == "archive"
        }

    have, want = crates(current), crates(generated)
    print("cargo-sources.json is out of sync with Cargo.lock.", file=sys.stderr)
    for crate in sorted(want - have):
        print(f"  missing: {crate}", file=sys.stderr)
    for crate in sorted(have - want):
        print(f"  stale:   {crate}", file=sys.stderr)
    if have == want:
        print("  (same crates, formatting differs)", file=sys.stderr)
    print("\nRegenerate it with: build-aux/cargo-sources.py", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
