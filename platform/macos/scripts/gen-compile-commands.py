#!/usr/bin/env python3
import json
import os
import shlex


def env(name):
    return os.environ[name]


cwd = env("CWD")
flags = [
    "-sdk", env("SDK"),
    "-target", env("TARGET"),
    "-Xcc", "-fmodule-map-file=" + env("MODMAP"),
    "-I", env("INCLUDE"),
] + shlex.split(env("DEFINES"))

targets = [
    ("ValetAutoFillExt", shlex.split(env("EXT_FILES"))),
    ("ValetAutoFill", shlex.split(env("APP_FILES"))),
]

entries = []
for module, files in targets:
    args = ["swiftc", "-module-name", module] + flags + files
    for f in files:
        entries.append({
            "directory": cwd,
            "file": f,
            "arguments": args,
        })

print(json.dumps(entries, indent=2))
