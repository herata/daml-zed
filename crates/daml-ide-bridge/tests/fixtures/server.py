#!/usr/bin/env python3
"""A stand-in for `damlc multi-ide`.

Replays the messages recorded in
docs/superpowers/specs/2026-09-02-daml-ide-bridge-design.md, so the bridge can
be tested end to end without a Daml SDK or a compile.
"""
import json
import pathlib
import sys

VR = "daml://compiler?file=%2Fa.daml&top-level-decl=setup"
LENS = {
    "range": {"start": {"line": 7, "character": 0}, "end": {"line": 7, "character": 5}},
    "command": {
        "command": "daml.showResource",
        "title": "Script results",
        "arguments": ["Script: setup", VR],
    },
}
# The real thing, captured by scripts/probe-protocol.py, so the test exercises
# the Markdown conversion on markup damlc actually produces.
HTML = (pathlib.Path(__file__).parent / "script-result.html").read_text()


def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))


while True:
    msg = read()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": msg["id"],
            "result": {
                "capabilities": {"executeCommandProvider": {"commands": ["typesignature.add"]}}
            },
        })
    elif method == "textDocument/codeLens":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [LENS]})
    elif method == "textDocument/codeAction":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": []})
    elif method == "textDocument/didOpen":
        if msg["params"]["textDocument"]["uri"].startswith("daml://"):
            send({
                "jsonrpc": "2.0",
                "method": "daml/virtualResource/didChange",
                "params": {"uri": VR, "contents": HTML},
            })
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        break
