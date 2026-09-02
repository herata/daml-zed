#!/usr/bin/env python3
"""Record how damlc multi-ide exposes script results over LSP.

Saves the rendered HTML and checks that editing the source pushes a fresh copy.
Run it against a built multi-package project to re-confirm the protocol after a
Daml upgrade:

    DAML_PROJECT=/path/to/project ./scripts/probe-protocol.py
"""
import json, os, re, subprocess, threading, time, pathlib, sys

ROOT = os.environ.get("DAML_PROJECT") or sys.exit("set DAML_PROJECT to a built multi-package Daml project")
FILE = os.environ.get("DAML_SCRIPT_FILE") or ROOT + "/test/daml/Test.daml"
CMD = [os.path.expanduser("~/.dpm/bin/dpm"), "damlc", "multi-ide",
       "--telemetry-ignored", "--log-level=Warning"]


def frame(o):
    b = json.dumps(o).encode()
    return b"Content-Length: %d\r\n\r\n" % len(b) + b


p = subprocess.Popen(CMD, cwd=ROOT, stdin=subprocess.PIPE,
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
msgs = []


def reader():
    while True:
        line = p.stdout.readline()
        if not line:
            return
        if line.startswith(b"Content-Length:"):
            n = int(line.split(b":")[1])
            p.stdout.readline()
            msgs.append(json.loads(p.stdout.read(n)))


threading.Thread(target=reader, daemon=True).start()


def send(o):
    p.stdin.write(frame(o))
    p.stdin.flush()


def wait(pred, t):
    end = time.time() + t
    while time.time() < end:
        hit = next((m for m in msgs if pred(m)), None)
        if hit:
            return hit
        time.sleep(0.5)
    return None


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "processId": os.getpid(), "rootUri": "file://" + ROOT,
    "capabilities": {"textDocument": {"codeLens": {}}},
    "workspaceFolders": [{"uri": "file://" + ROOT, "name": "d"}]}})
if not wait(lambda m: m.get("id") == 1, 240):
    print("no initialize"); p.kill(); sys.exit(1)
send({"jsonrpc": "2.0", "method": "initialized", "params": {}})

text = open(FILE).read()
uri = "file://" + FILE
send({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
    "textDocument": {"uri": uri, "languageId": "daml", "version": 1, "text": text}}})

lens = None
for a in range(24):
    time.sleep(10)
    send({"jsonrpc": "2.0", "id": 100 + a, "method": "textDocument/codeLens",
          "params": {"textDocument": {"uri": uri}}})
    r = wait(lambda m, a=a: m.get("id") == 100 + a, 60)
    if r and r.get("result"):
        lens = r["result"]
        break
    print(f"  codeLens attempt {a + 1}: empty")

if not lens:
    print("FAIL: no code lens")
    p.kill(); sys.exit(1)

vr = lens[0]["command"]["arguments"][1]
print("lens command:", lens[0]["command"]["command"], "| title:", lens[0]["command"]["title"])
send({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
    "textDocument": {"uri": vr, "languageId": "daml", "version": 0, "text": ""}}})
if not wait(lambda m: m.get("method", "").startswith("daml/virtualResource"), 240):
    print("FAIL: no virtualResource notification"); p.kill(); sys.exit(1)

changes = lambda: [m for m in msgs if m.get("method") == "daml/virtualResource/didChange"]
html = changes()[-1]["params"]["contents"]
pathlib.Path("script-result.html").write_text(html)
print("saved script-result.html, %d bytes" % len(html))
print("css vars:", sorted(set(re.findall(r"var\((--[a-zA-Z-]+)\)", html))))
print("script tags:", html.count("<script"))
print("body starts:", re.search(r"<body[^>]*>(.{0,200})", html, re.S).group(1).strip()[:200] if re.search(r"<body", html) else "no body tag")

before = len(changes())
send({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
    "textDocument": {"uri": uri, "version": 2},
    "contentChanges": [{"text": text.replace('"TV"', '"Radio"')}]}})
end = time.time() + 240
got = 0
while time.time() < end:
    if len(changes()) > before:
        got = len(changes())
        break
    time.sleep(1)
print("live update after edit:", f"YES ({got} total notifications)" if got else "NO")
if got:
    print("  new html mentions Radio:", "Radio" in changes()[-1]["params"]["contents"])
print("all methods:", sorted({m["method"] for m in msgs if "method" in m}))
p.kill()
