#!/usr/bin/env python3
"""Drive the bridge in front of a real damlc multi-ide, the way an editor would.

Checks the whole path that `cargo test` cannot: the real language server's code
lens, the injected code action, the rendered page, and that editing the source
updates it. Needs a built multi-package project:

    DAML_PROJECT=/path/to/project ./scripts/verify-against-damlc.py
"""
import json, os, re, subprocess, sys, threading, time, urllib.request

ROOT = os.environ.get("DAML_PROJECT") or sys.exit("set DAML_PROJECT to a built multi-package Daml project")
FILE = os.environ.get("DAML_SCRIPT_FILE") or ROOT + "/test/daml/Test.daml"
BRIDGE = os.environ.get("BRIDGE") or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "target/debug/daml-ide-bridge")
DPM = os.path.expanduser("~/.dpm/bin/dpm")

frame = lambda o: (lambda b: b"Content-Length: %d\r\n\r\n" % len(b) + b)(json.dumps(o).encode())

p = subprocess.Popen(
    [BRIDGE, "--no-open", "--", DPM, "damlc", "multi-ide", "--telemetry-ignored", "--log-level=Warning"],
    cwd=ROOT, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

announcement = p.stderr.readline().decode()
url = re.search(r"http://\S+", announcement).group(0)
print("bridge:", url.strip())
threading.Thread(target=lambda: [None for _ in p.stderr], daemon=True).start()

msgs = []
def reader():
    while True:
        line = p.stdout.readline()
        if not line:
            return
        if line.startswith(b"Content-Length:"):
            n = int(line.split(b":")[1]); p.stdout.readline()
            msgs.append(json.loads(p.stdout.read(n)))
threading.Thread(target=reader, daemon=True).start()

def send(o):
    p.stdin.write(frame(o)); p.stdin.flush()

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
    "capabilities": {"textDocument": {"codeLens": {}, "codeAction": {}}},
    "workspaceFolders": [{"uri": "file://" + ROOT, "name": "d"}]}})
init = wait(lambda m: m.get("id") == 1, 240)
if not init:
    print("FAIL: no initialize"); p.kill(); sys.exit(1)
commands = init["result"]["capabilities"]["executeCommandProvider"]["commands"]
print("advertised commands:", commands)
assert "daml.showResource" in commands, "the bridge must add its command"
assert "typesignature.add" in commands, "and keep the server's"

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
if not lens:
    print("FAIL: no code lens"); p.kill(); sys.exit(1)
line = lens[0]["range"]["start"]["line"]
print("code lens on line", line)

send({"jsonrpc": "2.0", "id": 500, "method": "textDocument/codeAction", "params": {
    "textDocument": {"uri": uri},
    "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 0}},
    "context": {"diagnostics": []}}})
actions = wait(lambda m: m.get("id") == 500, 120)
titles = [a.get("title") for a in (actions or {}).get("result", [])]
print("code actions:", titles)
assert any(t and t.startswith("Show script results") for t in titles), "injected action missing"

action = next(a for a in actions["result"] if a["title"].startswith("Show script results"))
send({"jsonrpc": "2.0", "id": 501, "method": "workspace/executeCommand", "params": action["command"]})
wait(lambda m: m.get("id") == 501, 60)

token = url.split("token=")[1].strip()
base = url.split("/?")[0]
page = ""
for _ in range(60):
    time.sleep(2)
    index = urllib.request.urlopen(f"{base}/?token={token}").read().decode()
    m = re.search(r"/r/([0-9a-f]+)", index)
    if not m:
        continue
    page = urllib.request.urlopen(f"{base}/r/{m.group(1)}?token={token}").read().decode()
    if "<table" in page:
        break

print("page bytes:", len(page))
print("has table:", "<table" in page)
print("placeholders substituted:", "$webviewSrc" not in page and "$webviewCss" not in page)
print("shim present:", "acquireVsCodeApi" in page)
print("theme linked:", "theme.css" in page)
print("body class kept:", re.search(r"<body class=\"[^\"]+\"", page).group(0) if "<body class=" in page else "MISSING")
# Live update: edit the source the way an editor would and re-fetch.
rid = re.search(r"/r/([0-9a-f]+)", urllib.request.urlopen(f"{base}/?token={token}").read().decode()).group(1)
before = urllib.request.urlopen(f"{base}/r/{rid}?token={token}").read().decode()
print("before edit mentions TV:", "TV" in before, "| Radio:", "Radio" in before)
send({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
    "textDocument": {"uri": uri, "version": 2},
    "contentChanges": [{"text": text.replace('"TV"', '"Radio"')}]}})
updated = False
for _ in range(90):
    time.sleep(2)
    after = urllib.request.urlopen(f"{base}/r/{rid}?token={token}").read().decode()
    if "Radio" in after:
        updated = True
        break
print("live update through the bridge:", "YES" if updated else "NO")

asset = urllib.request.urlopen(f"{base}/assets/webview.js?token={token}").read().decode()
print("webview.js served:", asset.startswith("// Copyright"))
try:
    urllib.request.urlopen(f"{base}/?token=wrong")
    print("token check: FAIL (wrong token accepted)")
except Exception as e:
    print("token check: OK", e.code if hasattr(e, "code") else e)
p.kill()
