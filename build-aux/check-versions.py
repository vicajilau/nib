#!/usr/bin/env python3
"""Verify that the version is declared identically everywhere it appears.

The version lives in three places that nothing keeps in step:

  Cargo.toml     drives config::VERSION, which is what the About dialog shows
  meson.build    the version the installed build reports
  metainfo.xml   the newest <release>, which is what AppStream and software
                 centres display

They drifted before: v0.1.1 and v0.1.2 were both tagged while Cargo.toml still said
0.1.0, so the released app reported the wrong version in its About dialog and the
metainfo listed no release for either tag.

The Flatpak manifest is deliberately not checked. Its "tag"/"commit" pin points at the
last released tag and is expected to lag main between releases.

Checking the three against each other catches a partial bump. It does not catch the
failure that actually happened, where nothing was bumped at all, so --expect takes the
version a release is claiming to be (the tag, without its leading v) and requires the
declarations to match it. CI passes that on tag pushes.

Usage:
    build-aux/check-versions.py
    build-aux/check-versions.py --expect 0.1.3
"""

import argparse
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"
MESON_BUILD = REPO_ROOT / "meson.build"
METAINFO = REPO_ROOT / "data" / "dev.victorcarreras.Nib.metainfo.xml"


def version_key(version: str) -> tuple[int, ...]:
    """Sorts dotted numeric versions; anything unparseable sorts lowest."""
    try:
        return tuple(int(part) for part in version.split("."))
    except ValueError:
        return (-1,)


def cargo_version() -> str:
    with CARGO_TOML.open("rb") as f:
        return tomllib.load(f)["package"]["version"]


def meson_version() -> str:
    # The project() call spans several lines, so match the version key rather than parse it.
    match = re.search(r"^\s*version:\s*'([^']+)'", MESON_BUILD.read_text(), re.M)
    if not match:
        raise SystemExit(f"No version: found in {MESON_BUILD.name}")
    return match.group(1)


def metainfo_version() -> str:
    releases = ET.parse(METAINFO).getroot().find("releases")
    entries = [] if releases is None else list(releases)
    versions = [r.get("version") for r in entries if r.get("version")]
    if not versions:
        raise SystemExit(f"No <release version=...> found in {METAINFO.name}")
    return max(versions, key=version_key)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--expect",
        metavar="VERSION",
        help="also require every declaration to equal this version (a release tag, sans 'v')",
    )
    args = parser.parse_args()

    found = {
        "Cargo.toml": cargo_version(),
        "meson.build": meson_version(),
        f"{METAINFO.name} (newest release)": metainfo_version(),
    }

    if args.expect:
        found["expected (release tag)"] = args.expect

    if len(set(found.values())) == 1:
        print(f"Version {next(iter(found.values()))} declared consistently")
        return 0

    print("Version declarations disagree:", file=sys.stderr)
    width = max(len(name) for name in found)
    for name, version in found.items():
        print(f"  {name:<{width}}  {version}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
