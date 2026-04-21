#!/usr/bin/env python3
"""Emit a swiftc -output-file-map JSON for incremental builds.

Usage: gen-output-file-map.py <objdir> <source.swift>...

Keys are the source paths as passed on the swiftc command line (relative to
CWD). Object and swiftdeps paths are placed under <objdir>, with slashes in
the source path flattened to underscores so Shared/Foo.swift and App/Foo.swift
don't collide.
"""
import json
import sys
from pathlib import PurePosixPath


def stem_for(src: str) -> str:
    return PurePosixPath(src).with_suffix("").as_posix().replace("/", "_")


def main() -> None:
    objdir = sys.argv[1]
    sources = sys.argv[2:]

    out = {"": {"swift-dependencies": f"{objdir}/master.swiftdeps"}}
    for src in sources:
        stem = stem_for(src)
        out[src] = {
            "object": f"{objdir}/{stem}.o",
            "swift-dependencies": f"{objdir}/{stem}.swiftdeps",
        }
    json.dump(out, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
