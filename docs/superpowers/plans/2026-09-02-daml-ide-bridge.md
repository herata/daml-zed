# daml-ide-bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native binary that proxies the Daml language server and serves script results to a browser, so Zed users get the Daml Studio experience Zed itself cannot render.

**Architecture:** `daml-ide-bridge` spawns `dpm damlc multi-ide` and relays LSP over stdio. It injects a "Show script results" code action, handles the resulting `workspace/executeCommand` itself, opens the virtual resource against the server, and serves the HTML the server pushes over a localhost HTTP server with SSE live-reload. All message rewriting lives in one I/O-free module so it can be unit tested; the plumbing around it stays thin.

**Tech Stack:** Rust (`serde_json`, `tiny_http`, `getrandom`), the LSP protocol measured in the design spec, vendored `webview.js`/`webview.css` from `digital-asset/daml` (Apache-2.0)

**Design spec:** `docs/superpowers/specs/2026-09-02-daml-ide-bridge-design.md`

---

## File structure

```
crates/daml-ide-bridge/
├── Cargo.toml
├── scripts/probe-protocol.py      already committed
├── src/
│   ├── main.rs                    argument parsing, wiring, nothing else
│   ├── framing.rs                 LSP Content-Length framing
│   ├── intercept.rs               every message rewrite, no I/O
│   ├── ids.rs                     stable ids and the access token
│   ├── resources.rs               virtual resource state and subscribers
│   ├── http.rs                    HTTP and SSE
│   ├── open.rs                    launching a browser
│   └── assets/
│       ├── webview.js             vendored, Apache-2.0
│       ├── webview.css            vendored, Apache-2.0
│       └── theme.css              our CSS variables
└── tests/
    └── proxy.rs                   integration test, skipped without dpm
```

`intercept.rs` is the only module with interesting logic. Everything else is
either plumbing or data. Keep it that way: if a rewrite rule starts leaking into
`main.rs`, move it back.

---

## Task 1: Crate skeleton and framing

**Files:**
- Create: `crates/daml-ide-bridge/Cargo.toml`
- Create: `crates/daml-ide-bridge/src/main.rs`
- Create: `crates/daml-ide-bridge/src/framing.rs`

- [ ] **Step 1: Write the failing framing test**

`src/framing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_one_message() {
        let mut input = &b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}"[..];
        let msg = read_message(&mut input).unwrap().unwrap();
        assert_eq!(msg["jsonrpc"], "2.0");
    }

    #[test]
    fn reads_two_messages_back_to_back() {
        let mut input =
            &b"Content-Length: 7\r\n\r\n{\"a\":1}Content-Length: 7\r\n\r\n{\"b\":2}"[..];
        assert_eq!(read_message(&mut input).unwrap().unwrap()["a"], 1);
        assert_eq!(read_message(&mut input).unwrap().unwrap()["b"], 2);
        assert!(read_message(&mut input).unwrap().is_none());
    }

    #[test]
    fn ignores_unknown_headers() {
        let mut input =
            &b"Content-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}"[..];
        assert_eq!(read_message(&mut input).unwrap().unwrap()["a"], 1);
    }

    #[test]
    fn round_trips() {
        let mut buf = Vec::new();
        write_message(&mut buf, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(read_message(&mut &buf[..]).unwrap().unwrap()["a"], 1);
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cd crates/daml-ide-bridge && cargo test
```

Expected: `read_message` and `write_message` undefined.

- [ ] **Step 3: Implement the framing**

```rust
//! LSP messages are `Content-Length: N\r\n\r\n` followed by N bytes of JSON.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Returns `Ok(None)` at a clean end of stream.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            len = rest.trim().parse().ok();
        }
    }
    let len = len.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "message without Content-Length")
    })?;
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(io::Error::other)
}

pub fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()
}
```

- [ ] **Step 4: Verify**

```bash
cargo test
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/daml-ide-bridge
git commit -m "feat(bridge): LSP message framing"
```

---

## Task 2: Interception rules

**Files:**
- Create: `crates/daml-ide-bridge/src/intercept.rs`

This is the heart of the bridge. Every rewrite goes here and nothing here does I/O.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lens() -> Value {
        json!({
            "range": {"start": {"line": 7, "character": 0},
                      "end": {"line": 7, "character": 5}},
            "command": {
                "command": "daml.showResource",
                "title": "Script results",
                "arguments": ["Script: setup", "daml://compiler?file=%2Fa.daml&top-level-decl=setup"]
            }
        })
    }

    #[test]
    fn advertises_the_show_resource_command() {
        let mut i = Interceptor::default();
        i.from_client(json!({"id": 1, "method": "initialize", "params": {}}));
        let out = i.from_server(json!({
            "id": 1,
            "result": {"capabilities": {"executeCommandProvider": {"commands": ["typesignature.add"]}}}
        }));
        let Outbound::ToClient(msg) = &out[0] else { panic!("{out:?}") };
        let commands = &msg["result"]["capabilities"]["executeCommandProvider"]["commands"];
        assert!(commands.as_array().unwrap().iter().any(|c| c == "daml.showResource"));
        assert!(commands.as_array().unwrap().iter().any(|c| c == "typesignature.add"));
    }

    #[test]
    fn adds_the_command_when_the_server_advertises_none() {
        let mut i = Interceptor::default();
        i.from_client(json!({"id": 1, "method": "initialize", "params": {}}));
        let out = i.from_server(json!({"id": 1, "result": {"capabilities": {}}}));
        let Outbound::ToClient(msg) = &out[0] else { panic!() };
        assert_eq!(
            msg["result"]["capabilities"]["executeCommandProvider"]["commands"][0],
            "daml.showResource"
        );
    }

    #[test]
    fn remembers_lenses_and_turns_them_into_code_actions() {
        let mut i = Interceptor::default();
        i.from_client(json!({"id": 2, "method": "textDocument/codeLens",
                             "params": {"textDocument": {"uri": "file:///a.daml"}}}));
        i.from_server(json!({"id": 2, "result": [lens()]}));

        i.from_client(json!({"id": 3, "method": "textDocument/codeAction",
                             "params": {"textDocument": {"uri": "file:///a.daml"},
                                        "range": {"start": {"line": 7, "character": 0},
                                                  "end": {"line": 7, "character": 0}}}}));
        let out = i.from_server(json!({"id": 3, "result": []}));
        let Outbound::ToClient(msg) = &out[0] else { panic!() };
        let action = &msg["result"][0];
        assert_eq!(action["title"], "Show script results: Script: setup");
        assert_eq!(action["command"]["command"], "daml.showResource");
    }

    #[test]
    fn does_not_offer_an_action_for_another_line() {
        let mut i = Interceptor::default();
        i.from_client(json!({"id": 2, "method": "textDocument/codeLens",
                             "params": {"textDocument": {"uri": "file:///a.daml"}}}));
        i.from_server(json!({"id": 2, "result": [lens()]}));
        i.from_client(json!({"id": 3, "method": "textDocument/codeAction",
                             "params": {"textDocument": {"uri": "file:///a.daml"},
                                        "range": {"start": {"line": 40, "character": 0},
                                                  "end": {"line": 40, "character": 0}}}}));
        let out = i.from_server(json!({"id": 3, "result": []}));
        let Outbound::ToClient(msg) = &out[0] else { panic!() };
        assert_eq!(msg["result"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn handles_show_resource_without_forwarding_it() {
        let mut i = Interceptor::default();
        let out = i.from_client(json!({
            "id": 9, "method": "workspace/executeCommand",
            "params": {"command": "daml.showResource",
                       "arguments": ["Script: setup", "daml://compiler?x=1"]}
        }));
        assert!(matches!(out[0], Outbound::Show { .. }));
        // The editor still needs a response, and the server must not see it.
        assert!(out.iter().any(|o| matches!(o, Outbound::ToClient(_))));
        assert!(!out.iter().any(|o| matches!(o, Outbound::ToServer(_))));
    }

    #[test]
    fn forwards_other_commands_to_the_server() {
        let mut i = Interceptor::default();
        let out = i.from_client(json!({
            "id": 9, "method": "workspace/executeCommand",
            "params": {"command": "typesignature.add", "arguments": []}
        }));
        assert!(matches!(out[0], Outbound::ToServer(_)));
    }

    #[test]
    fn absorbs_virtual_resource_notifications() {
        let mut i = Interceptor::default();
        let out = i.from_server(json!({
            "method": "daml/virtualResource/didChange",
            "params": {"uri": "daml://compiler?x=1", "contents": "<html></html>"}
        }));
        match &out[0] {
            Outbound::ResourceChanged { uri, contents } => {
                assert_eq!(uri, "daml://compiler?x=1");
                assert_eq!(contents, "<html></html>");
            }
            other => panic!("{other:?}"),
        }
        assert!(!out.iter().any(|o| matches!(o, Outbound::ToClient(_))));
    }

    #[test]
    fn passes_everything_else_through_untouched() {
        let mut i = Interceptor::default();
        let diag = json!({"method": "textDocument/publishDiagnostics", "params": {"uri": "file:///a"}});
        let out = i.from_server(diag.clone());
        let Outbound::ToClient(msg) = &out[0] else { panic!() };
        assert_eq!(msg, &diag);

        let hover = json!({"id": 5, "method": "textDocument/hover", "params": {}});
        let out = i.from_client(hover.clone());
        let Outbound::ToServer(msg) = &out[0] else { panic!() };
        assert_eq!(msg, &hover);
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test
```

Expected: `Interceptor` and `Outbound` undefined.

- [ ] **Step 3: Implement**

Write `Outbound`, `Interceptor` and the rules above `mod tests`:

```rust
//! Every rewrite the bridge performs, with no I/O so it can be tested directly.

use std::collections::HashMap;

use serde_json::{json, Value};

pub const SHOW_RESOURCE: &str = "daml.showResource";

#[derive(Debug)]
pub enum Outbound {
    ToServer(Value),
    ToClient(Value),
    /// The editor asked for a script result to be displayed.
    Show { title: String, uri: String },
    /// The server rendered a virtual resource.
    ResourceChanged { uri: String, contents: String },
    /// The server started recomputing a virtual resource.
    ResourceProgress { uri: String },
}

/// What a request the client sent is waiting for, so the matching response can
/// be rewritten when it comes back.
#[derive(Debug, Clone)]
enum Pending {
    Initialize,
    CodeLens { document: String },
    CodeAction { document: String, range: Value },
}

#[derive(Debug, Default)]
pub struct Interceptor {
    pending: HashMap<String, Pending>,
    /// Code lenses per document, kept so code actions can be synthesised from
    /// them. Zed's code lens support is opt-in, code actions always work.
    lenses: HashMap<String, Vec<Value>>,
}
```

The methods:

```rust
impl Interceptor {
    pub fn from_client(&mut self, msg: Value) -> Vec<Outbound> {
        let method = msg["method"].as_str().unwrap_or_default();
        let id = id_key(&msg);

        if method == "workspace/executeCommand"
            && msg["params"]["command"] == SHOW_RESOURCE
        {
            let args = msg["params"]["arguments"].as_array().cloned().unwrap_or_default();
            let title = args.first().and_then(Value::as_str).unwrap_or("Script results");
            let uri = args.get(1).and_then(Value::as_str).unwrap_or_default();
            let mut out = vec![Outbound::Show {
                title: title.to_string(),
                uri: uri.to_string(),
            }];
            if let Some(id) = msg.get("id") {
                out.push(Outbound::ToClient(
                    json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}),
                ));
            }
            return out;
        }

        if let Some(id) = id {
            let pending = match method {
                "initialize" => Some(Pending::Initialize),
                "textDocument/codeLens" => Some(Pending::CodeLens {
                    document: document_of(&msg),
                }),
                "textDocument/codeAction" => Some(Pending::CodeAction {
                    document: document_of(&msg),
                    range: msg["params"]["range"].clone(),
                }),
                _ => None,
            };
            if let Some(pending) = pending {
                self.pending.insert(id, pending);
            }
        }

        vec![Outbound::ToServer(msg)]
    }

    pub fn from_server(&mut self, msg: Value) -> Vec<Outbound> {
        match msg["method"].as_str() {
            Some("daml/virtualResource/didChange") => {
                return vec![Outbound::ResourceChanged {
                    uri: msg["params"]["uri"].as_str().unwrap_or_default().to_string(),
                    contents: msg["params"]["contents"].as_str().unwrap_or_default().to_string(),
                }];
            }
            Some("daml/virtualResource/didProgress") | Some("daml/virtualResource/note") => {
                return vec![Outbound::ResourceProgress {
                    uri: msg["params"]["uri"].as_str().unwrap_or_default().to_string(),
                }];
            }
            _ => {}
        }

        let Some(id) = id_key(&msg) else {
            return vec![Outbound::ToClient(msg)];
        };
        let Some(pending) = self.pending.remove(&id) else {
            return vec![Outbound::ToClient(msg)];
        };

        let mut msg = msg;
        match pending {
            Pending::Initialize => advertise_command(&mut msg),
            Pending::CodeLens { document } => {
                if let Some(lenses) = msg["result"].as_array() {
                    self.lenses.insert(document, lenses.clone());
                }
            }
            Pending::CodeAction { document, range } => {
                self.inject_actions(&mut msg, &document, &range)
            }
        }
        vec![Outbound::ToClient(msg)]
    }
}
```

Plus the free functions `id_key`, `document_of`, `advertise_command`,
`lens_intersects`, and `Interceptor::inject_actions`:

```rust
/// Request ids may be numbers or strings; normalise to a string key.
fn id_key(msg: &Value) -> Option<String> {
    match &msg["id"] {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn document_of(msg: &Value) -> String {
    msg["params"]["textDocument"]["uri"].as_str().unwrap_or_default().to_string()
}

/// Zed will not run a command the server does not claim to support.
fn advertise_command(msg: &mut Value) {
    let commands = msg["result"]["capabilities"]["executeCommandProvider"]["commands"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if commands.iter().any(|c| c == SHOW_RESOURCE) {
        return;
    }
    let mut commands = commands;
    commands.push(json!(SHOW_RESOURCE));
    msg["result"]["capabilities"]["executeCommandProvider"] = json!({"commands": commands});
}

fn line_of(range: &Value, edge: &str) -> i64 {
    range[edge]["line"].as_i64().unwrap_or(-1)
}

/// A lens applies if the requested range touches any of its lines.
fn lens_intersects(lens: &Value, range: &Value) -> bool {
    let lens_start = line_of(&lens["range"], "start");
    let lens_end = line_of(&lens["range"], "end");
    let want_start = line_of(range, "start");
    let want_end = line_of(range, "end");
    lens_start <= want_end && want_start <= lens_end
}

impl Interceptor {
    fn inject_actions(&self, msg: &mut Value, document: &str, range: &Value) {
        let Some(lenses) = self.lenses.get(document) else { return };
        let mut actions = msg["result"].as_array().cloned().unwrap_or_default();
        for lens in lenses {
            if lens["command"]["command"] != SHOW_RESOURCE || !lens_intersects(lens, range) {
                continue;
            }
            let title = lens["command"]["arguments"][0]
                .as_str()
                .unwrap_or("Script results");
            actions.push(json!({
                "title": format!("Show script results: {title}"),
                "kind": "source.daml.showResource",
                "command": lens["command"],
            }));
        }
        msg["result"] = Value::Array(actions);
    }
}
```

- [ ] **Step 4: Verify**

```bash
cargo test
```

Expected: 8 interception tests plus the 4 framing tests pass.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(bridge): LSP interception rules"
```

---

## Task 3: Resource registry

**Files:**
- Create: `crates/daml-ide-bridge/src/ids.rs`
- Create: `crates/daml-ide-bridge/src/resources.rs`

- [ ] **Step 1: Write the failing tests**

`src/resources.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_updates() {
        let reg = Registry::default();
        let id = reg.register("Script: setup", "daml://x");
        assert!(reg.get(&id).unwrap().html.is_none());

        reg.update("daml://x", "<html>1</html>");
        assert_eq!(reg.get(&id).unwrap().html.as_deref(), Some("<html>1</html>"));

        reg.update("daml://x", "<html>2</html>");
        assert_eq!(reg.get(&id).unwrap().html.as_deref(), Some("<html>2</html>"));
    }

    #[test]
    fn the_same_uri_keeps_the_same_id() {
        let reg = Registry::default();
        assert_eq!(reg.register("a", "daml://x"), reg.register("a", "daml://x"));
    }

    #[test]
    fn an_update_for_an_unknown_uri_is_ignored() {
        let reg = Registry::default();
        reg.update("daml://never-seen", "<html/>");
        assert!(reg.list().is_empty());
    }

    #[test]
    fn subscribers_are_woken_on_update() {
        let reg = Registry::default();
        let id = reg.register("a", "daml://x");
        let rx = reg.subscribe(&id).unwrap();
        reg.update("daml://x", "<html/>");
        assert!(rx.recv_timeout(std::time::Duration::from_secs(1)).is_ok());
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test
```

- [ ] **Step 3: Implement `ids.rs`**

```rust
//! Short, stable identifiers for virtual resources, and the access token.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A resource id is derived from its `daml://` URI so the same script always
/// gets the same page. It only has to be stable within one bridge process.
pub fn resource_id(uri: &str) -> String {
    let mut hasher = DefaultHasher::new();
    uri.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The HTTP server binds to localhost, but every other process on the machine
/// can reach localhost too, and these pages contain project source. Require a
/// token that only the bridge and the links it opens know.
pub fn access_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("no source of randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
```

- [ ] **Step 4: Implement `resources.rs`**

```rust
//! The set of script results the bridge is currently showing.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use crate::ids::resource_id;

#[derive(Debug, Clone)]
pub struct Resource {
    pub id: String,
    pub title: String,
    pub uri: String,
    /// `None` until the server has rendered it once.
    pub html: Option<String>,
    pub running: bool,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    by_id: HashMap<String, Resource>,
    id_by_uri: HashMap<String, String>,
    subscribers: HashMap<String, Vec<Sender<()>>>,
}

impl Registry {
    /// Returns the id, and whether this is the first time the URI was seen.
    pub fn register(&self, title: &str, uri: &str) -> String {
        let id = resource_id(uri);
        let mut inner = self.inner.lock().unwrap();
        inner.id_by_uri.insert(uri.to_string(), id.clone());
        inner.by_id.entry(id.clone()).or_insert_with(|| Resource {
            id: id.clone(),
            title: title.to_string(),
            uri: uri.to_string(),
            html: None,
            running: true,
        });
        id
    }

    pub fn is_known(&self, uri: &str) -> bool {
        self.inner.lock().unwrap().id_by_uri.contains_key(uri)
    }

    pub fn update(&self, uri: &str, html: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(id) = inner.id_by_uri.get(uri).cloned() else { return };
        if let Some(resource) = inner.by_id.get_mut(&id) {
            resource.html = Some(html.to_string());
            resource.running = false;
        }
        notify(&mut inner, &id);
    }

    pub fn set_running(&self, uri: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(id) = inner.id_by_uri.get(uri).cloned() else { return };
        if let Some(resource) = inner.by_id.get_mut(&id) {
            resource.running = true;
        }
        notify(&mut inner, &id);
    }

    pub fn get(&self, id: &str) -> Option<Resource> {
        self.inner.lock().unwrap().by_id.get(id).cloned()
    }

    pub fn list(&self) -> Vec<Resource> {
        let mut all: Vec<_> = self.inner.lock().unwrap().by_id.values().cloned().collect();
        all.sort_by(|a, b| a.title.cmp(&b.title));
        all
    }

    pub fn subscribe(&self, id: &str) -> Option<Receiver<()>> {
        let mut inner = self.inner.lock().unwrap();
        inner.by_id.get(id)?;
        let (tx, rx) = channel();
        inner.subscribers.entry(id.to_string()).or_default().push(tx);
        Some(rx)
    }
}

/// Dropping the receiver is how a closed browser tab unsubscribes.
fn notify(inner: &mut Inner, id: &str) {
    if let Some(subscribers) = inner.subscribers.get_mut(id) {
        subscribers.retain(|tx| tx.send(()).is_ok());
    }
}
```

- [ ] **Step 5: Verify and commit**

```bash
cargo test
git commit -am "feat(bridge): virtual resource registry"
```

---

## Task 4: Vendored assets and the page shell

**Files:**
- Create: `crates/daml-ide-bridge/src/assets/webview.js`
- Create: `crates/daml-ide-bridge/src/assets/webview.css`
- Create: `crates/daml-ide-bridge/src/assets/theme.css`
- Create: `crates/daml-ide-bridge/src/page.rs`

- [ ] **Step 1: Vendor the two files**

```bash
cd crates/daml-ide-bridge/src/assets
gh api repos/digital-asset/daml/contents/sdk/compiler/daml-extension/src/webview.js \
  --jq '.content' | base64 -d > webview.js
gh api repos/digital-asset/daml/contents/sdk/compiler/daml-extension/src/webview.css \
  --jq '.content' | base64 -d > webview.css
head -3 webview.js webview.css
```

Both already carry the Digital Asset copyright header and the Apache-2.0
identifier; leave them untouched so the attribution travels with the file.
Record the provenance in `crates/daml-ide-bridge/README.md`.

- [ ] **Step 2: Write `theme.css`**

The server's HTML references VS Code's terminal colour variables. Define them
for both colour schemes rather than renaming anything, so a change on the
server side keeps working.

```css
/* The HTML damlc renders references VS Code's theme variables by name. Define
   them here instead of rewriting the HTML, so a change on the server side does
   not silently lose colour. */
:root {
  color-scheme: light dark;
  --link-color: #0969da;
  --vscode-terminal-ansiRed: #cf222e;
  --vscode-terminal-ansiBrightRed: #a40e26;
  --vscode-terminal-ansiYellow: #9a6700;
  --vscode-terminal-ansiBrightYellow: #7d4e00;
  --vscode-terminal-ansiGreen: #1a7f37;
  --vscode-terminal-ansiBlue: #0969da;
  --vscode-terminal-ansiBrightBlue: #218bff;
  --vscode-terminal-ansiMagenta: #8250df;
  --vscode-terminal-ansiWhite: #6e7781;
  --page-bg: #ffffff;
  --page-fg: #1f2328;
}

@media (prefers-color-scheme: dark) {
  :root {
    --link-color: #4493f8;
    --vscode-terminal-ansiRed: #ff7b72;
    --vscode-terminal-ansiBrightRed: #ffa198;
    --vscode-terminal-ansiYellow: #d29922;
    --vscode-terminal-ansiBrightYellow: #e3b341;
    --vscode-terminal-ansiGreen: #3fb950;
    --vscode-terminal-ansiBlue: #58a6ff;
    --vscode-terminal-ansiBrightBlue: #79c0ff;
    --vscode-terminal-ansiMagenta: #bc8cff;
    --vscode-terminal-ansiWhite: #8b949e;
    --page-bg: #0d1117;
    --page-fg: #e6edf3;
  }
}

body {
  background: var(--page-bg);
  color: var(--page-fg);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
  margin: 0;
  padding: 1rem;
}

table, th, td { border-color: color-mix(in srgb, var(--page-fg) 25%, transparent); }
#bridge-status { font: 12px ui-monospace, monospace; opacity: 0.7; padding-bottom: 0.5rem; }
```

- [ ] **Step 3: Write the failing page test**

`src/page.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const RENDERED: &str = r#"<!DOCTYPE HTML>
<html><head><style>.da-code {}</style><script src="$webviewSrc"></script><link rel="stylesheet" href="$webviewCss"></head><body class="hide_archived"><div>hi</div></body></html>"#;

    #[test]
    fn substitutes_the_asset_placeholders() {
        let out = fill_placeholders(RENDERED, "tok");
        assert!(!out.contains("$webviewSrc"));
        assert!(!out.contains("$webviewCss"));
        assert!(out.contains("/assets/webview.js?token=tok"));
        assert!(out.contains("/assets/webview.css?token=tok"));
    }

    #[test]
    fn adds_the_theme_stylesheet() {
        assert!(fill_placeholders(RENDERED, "tok").contains("/assets/theme.css?token=tok"));
    }

    #[test]
    fn keeps_the_body_class_the_server_chose() {
        // The class drives which of the two views is visible; losing it shows both.
        assert!(fill_placeholders(RENDERED, "tok").contains(r#"class="hide_archived""#));
    }
}
```

- [ ] **Step 4: Implement `page.rs`**

```rust
//! Turning the server's rendered HTML into a page a browser can load.

/// damlc emits `$webviewSrc` and `$webviewCss` placeholders for the client to
/// fill in with its own copies of the view's script and stylesheet. Point them
/// at the assets this bridge serves, and add the theme variables the HTML
/// references.
pub fn fill_placeholders(html: &str, token: &str) -> String {
    let js = format!("/assets/webview.js?token={token}");
    let css = format!("/assets/webview.css?token={token}");
    let theme = format!(r#"<link rel="stylesheet" href="/assets/theme.css?token={token}">"#);
    html.replace("$webviewSrc", &js)
        .replace("$webviewCss", &css)
        .replace("</head>", &format!("{theme}</head>"))
}

/// The wrapper served at `/r/{id}`: a status line, the rendered result, and a
/// tiny SSE client that refetches the body when the server re-renders.
pub fn shell(title: &str, id: &str, token: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>{title}</title>
<link rel="stylesheet" href="/assets/theme.css?token={token}">
</head><body>
<div id="bridge-status">connecting…</div>
<div id="bridge-body">{body}</div>
<script>
const status = document.getElementById('bridge-status');
const target = document.getElementById('bridge-body');
async function refresh() {{
  const r = await fetch('/r/{id}/body?token={token}');
  target.innerHTML = await r.text();
  if (window.setup_view) window.setup_view();
}}
const es = new EventSource('/r/{id}/events?token={token}');
es.onopen = () => status.textContent = 'live';
es.onerror = () => status.textContent = 'disconnected — the bridge stopped';
es.onmessage = (e) => {{ status.textContent = e.data === 'running' ? 'running…' : 'live'; refresh(); }};
</script>
</body></html>"#
    )
}
```

- [ ] **Step 5: Verify and commit**

```bash
cargo test
git commit -am "feat(bridge): page shell and vendored webview assets"
```

---

## Task 5: HTTP server

**Files:**
- Create: `crates/daml-ide-bridge/src/http.rs`

- [ ] **Step 1: Add the dependency**

`Cargo.toml`:

```toml
[dependencies]
getrandom = "0.3"
serde_json = "1"
tiny_http = "0.12"
```

- [ ] **Step 2: Implement**

```rust
//! A localhost HTTP server for the rendered script results.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use tiny_http::{Header, Response, Server};

use crate::page;
use crate::resources::Registry;

pub struct Http {
    pub port: u16,
    pub token: String,
}

/// Binds to an OS-chosen port on the loopback interface and serves until the
/// process exits.
pub fn serve(registry: Arc<Registry>, token: String) -> std::io::Result<Http> {
    let server = Server::http("127.0.0.1:0").map_err(std::io::Error::other)?;
    let port = server.server_addr().to_ip().unwrap().port();
    let served_token = token.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let registry = Arc::clone(&registry);
            let token = served_token.clone();
            std::thread::spawn(move || handle(request, &registry, &token));
        }
    });
    Ok(Http { port, token })
}
```

Then `handle`, routing on the path and rejecting a wrong token with 403:

```rust
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

fn html(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_header(header("Content-Type", "text/html; charset=utf-8"))
}

fn handle(request: tiny_http::Request, registry: &Registry, token: &str) {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let supplied = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .unwrap_or_default();
    if supplied != token {
        let _ = request.respond(Response::from_string("forbidden").with_status_code(403));
        return;
    }

    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        [""] => {
            let items: String = registry
                .list()
                .iter()
                .map(|r| {
                    format!(
                        r#"<li><a href="/r/{}?token={}">{}</a></li>"#,
                        r.id, token, r.title
                    )
                })
                .collect();
            let body = if items.is_empty() {
                "<p>No script results open yet. Run the “Show script results” code action in your editor.</p>".to_string()
            } else {
                format!("<ul>{items}</ul>")
            };
            let _ = request.respond(html(page::shell_index(&body, token)));
        }
        ["assets", name] => {
            let (bytes, mime): (&[u8], &str) = match *name {
                "webview.js" => (include_bytes!("assets/webview.js"), "text/javascript"),
                "webview.css" => (include_bytes!("assets/webview.css"), "text/css"),
                "theme.css" => (include_bytes!("assets/theme.css"), "text/css"),
                _ => {
                    let _ = request.respond(Response::from_string("not found").with_status_code(404));
                    return;
                }
            };
            let _ = request.respond(
                Response::from_data(bytes.to_vec()).with_header(header("Content-Type", mime)),
            );
        }
        ["r", id] => match registry.get(id) {
            Some(resource) => {
                let body = resource
                    .html
                    .as_deref()
                    .map(|h| page::fill_placeholders(h, token))
                    .unwrap_or_else(|| "<p>running…</p>".to_string());
                let _ = request.respond(html(page::shell(&resource.title, id, token, &body)));
            }
            None => {
                let _ = request.respond(Response::from_string("unknown result").with_status_code(404));
            }
        },
        ["r", id, "body"] => match registry.get(id).and_then(|r| r.html) {
            Some(h) => {
                let _ = request.respond(html(page::fill_placeholders(&h, token)));
            }
            None => {
                let _ = request.respond(html("<p>running…</p>".to_string()));
            }
        },
        ["r", id, "events"] => stream_events(request, registry, id),
        _ => {
            let _ = request.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// Server-sent events. tiny_http has no SSE helper, so write the frames by hand
/// over the raw socket.
fn stream_events(request: tiny_http::Request, registry: &Registry, id: &str) {
    let Some(rx) = registry.subscribe(id) else {
        let _ = request.respond(Response::from_string("unknown result").with_status_code(404));
        return;
    };
    let mut writer = request.into_writer();
    let preamble = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Cache-Control: no-cache\r\n\
                    Connection: keep-alive\r\n\r\n";
    if writer.write_all(preamble.as_bytes()).is_err() {
        return;
    }
    loop {
        let event = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(()) => {
                let running = registry.get(id).map(|r| r.running).unwrap_or(false);
                if running { "running" } else { "changed" }
            }
            // A comment frame keeps the connection open through proxies and
            // tells us when the tab is gone.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => ":keepalive",
            Err(_) => return,
        };
        let frame = if event.starts_with(':') {
            format!("{event}\n\n")
        } else {
            format!("data: {event}\n\n")
        };
        if writer.write_all(frame.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
    }
}
```

Add `page::shell_index`:

```rust
pub fn shell_index(body: &str, token: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Daml script results</title>
<link rel="stylesheet" href="/assets/theme.css?token={token}"></head>
<body><h1>Daml script results</h1>{body}</body></html>"#
    )
}
```

- [ ] **Step 3: Verify it builds and commit**

```bash
cargo test
git commit -am "feat(bridge): localhost HTTP server with SSE live reload"
```

---

## Task 6: Browser launch and wiring

**Files:**
- Create: `crates/daml-ide-bridge/src/open.rs`
- Modify: `crates/daml-ide-bridge/src/main.rs`

- [ ] **Step 1: Implement `open.rs`**

```rust
//! Opening a URL in the user's browser.

use std::process::{Command, Stdio};

pub fn url(url: &str) {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    // Failing to open a browser is not fatal: the URL is on stderr anyway.
    let _ = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}
```

- [ ] **Step 2: Implement `main.rs`**

```rust
//! Proxies the Daml language server and serves its script results to a browser.
//!
//! Usage: daml-ide-bridge [--no-open] -- <language server command...>

mod framing;
mod http;
mod ids;
mod intercept;
mod open;
mod page;
mod resources;

use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

use crate::intercept::{Interceptor, Outbound};
use crate::resources::Registry;

fn main() -> std::io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let auto_open = !args.iter().any(|a| a == "--no-open");
    args.retain(|a| a != "--no-open");
    let server_command: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => args[i + 1..].to_vec(),
        None => args,
    };
    if server_command.is_empty() {
        eprintln!("usage: daml-ide-bridge [--no-open] -- <language server command...>");
        std::process::exit(2);
    }
    run(server_command, auto_open)
}
```

`run` spawns the child, starts the HTTP server, and creates the two writer
threads plus the two pumps. Keep the pumps free of logic: they read a message,
hand it to the `Interceptor`, and dispatch the `Outbound`s.

```rust
fn run(server_command: Vec<String>, auto_open: bool) -> std::io::Result<()> {
    let mut child: Child = Command::new(&server_command[0])
        .args(&server_command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let registry = Arc::new(Registry::default());
    let token = ids::access_token();
    let server = http::serve(Arc::clone(&registry), token.clone())?;
    let base = format!("http://127.0.0.1:{}", server.port);
    eprintln!("daml-ide-bridge: script results at {base}/?token={token}");

    let (to_server, server_rx) = channel::<Value>();
    let (to_client, client_rx) = channel::<Value>();

    let mut child_stdin = BufWriter::new(child.stdin.take().expect("piped"));
    thread::spawn(move || {
        for msg in server_rx {
            if framing::write_message(&mut child_stdin, &msg).is_err() {
                return;
            }
        }
    });

    thread::spawn(move || {
        let mut out = BufWriter::new(std::io::stdout());
        for msg in client_rx {
            if framing::write_message(&mut out, &msg).is_err() {
                return;
            }
        }
    });

    let interceptor = Arc::new(std::sync::Mutex::new(Interceptor::default()));

    // server -> editor
    let mut child_stdout = BufReader::new(child.stdout.take().expect("piped"));
    let up = {
        let interceptor = Arc::clone(&interceptor);
        let registry = Arc::clone(&registry);
        let to_client = to_client.clone();
        let to_server = to_server.clone();
        let base = base.clone();
        let token = token.clone();
        thread::spawn(move || {
            while let Ok(Some(msg)) = framing::read_message(&mut child_stdout) {
                let outbound = interceptor.lock().unwrap().from_server(msg);
                dispatch(outbound, &registry, &to_server, &to_client, &base, &token, auto_open);
            }
        })
    };

    // editor -> server
    let mut stdin = BufReader::new(std::io::stdin());
    while let Ok(Some(msg)) = framing::read_message(&mut stdin) {
        let outbound = interceptor.lock().unwrap().from_client(msg);
        dispatch(outbound, &registry, &to_server, &to_client, &base, &token, auto_open);
    }

    let _ = child.kill();
    let _ = up.join();
    Ok(())
}
```

`dispatch` is the only place that reacts to an `Outbound`:

```rust
#[allow(clippy::too_many_arguments)]
fn dispatch(
    outbound: Vec<Outbound>,
    registry: &Registry,
    to_server: &Sender<Value>,
    to_client: &Sender<Value>,
    base: &str,
    token: &str,
    auto_open: bool,
) {
    for item in outbound {
        match item {
            Outbound::ToServer(msg) => {
                let _ = to_server.send(msg);
            }
            Outbound::ToClient(msg) => {
                let _ = to_client.send(msg);
            }
            Outbound::Show { title, uri } => {
                let first = !registry.is_known(&uri);
                let id = registry.register(&title, &uri);
                if first {
                    // Opening the virtual resource is what makes the server
                    // start rendering it, exactly as the VS Code extension does.
                    let _ = to_server.send(json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didOpen",
                        "params": {"textDocument": {
                            "uri": uri, "languageId": "daml", "version": 0, "text": ""}}
                    }));
                }
                let url = format!("{base}/r/{id}?token={token}");
                eprintln!("daml-ide-bridge: {title} -> {url}");
                if auto_open {
                    open::url(&url);
                }
            }
            Outbound::ResourceChanged { uri, contents } => registry.update(&uri, &contents),
            Outbound::ResourceProgress { uri } => registry.set_running(&uri),
        }
    }
}
```

- [ ] **Step 3: Build**

```bash
cargo build
```

- [ ] **Step 4: Verify**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(bridge): proxy the language server and serve script results"
```

---

## Task 7: Integration test against a scripted server

**Files:**
- Create: `crates/daml-ide-bridge/tests/fixtures/server.py`
- Create: `crates/daml-ide-bridge/tests/proxy.rs`

Testing against a real `dpm damlc multi-ide` needs a built Daml project and
minutes of compilation, which does not belong in `cargo test`. Instead, replay
the exact messages recorded in the design spec from a scripted stand-in. That
tests the bridge - framing, interception, registry, HTTP - deterministically and
in under a second, and it is the part that can actually regress. The real server
is covered by the manual check in the README.

- [ ] **Step 1: Write the stand-in server**

`tests/fixtures/server.py` speaks LSP on stdio and replays the recorded
responses:

```python
#!/usr/bin/env python3
"""A stand-in for `damlc multi-ide` that replays the messages recorded in
docs/superpowers/specs/2026-09-02-daml-ide-bridge-design.md."""
import json, sys

VR = "daml://compiler?file=%2Fa.daml&top-level-decl=setup"
LENS = {
    "range": {"start": {"line": 7, "character": 0}, "end": {"line": 7, "character": 5}},
    "command": {
        "command": "daml.showResource",
        "title": "Script results",
        "arguments": ["Script: setup", VR],
    },
}
HTML = ('<!DOCTYPE HTML><html><head><style>.da-code {}</style>'
        '<script src="$webviewSrc"></script>'
        '<link rel="stylesheet" href="$webviewCss"></head>'
        '<body class="hide_archived"><table><tr><td>Iou</td></tr></table></body></html>')


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
    return json.loads(sys.stdin.buffer.read(length))


while (msg := read()) is not None:
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "executeCommandProvider": {"commands": ["typesignature.add"]}}}})
    elif method == "textDocument/codeLens":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [LENS]})
    elif method == "textDocument/codeAction":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": []})
    elif method == "textDocument/didOpen":
        if msg["params"]["textDocument"]["uri"].startswith("daml://"):
            send({"jsonrpc": "2.0", "method": "daml/virtualResource/didChange",
                  "params": {"uri": VR, "contents": HTML}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        break
```

- [ ] **Step 2: Write the failing test**

`tests/proxy.rs`:

```rust
//! Drives the bridge the way an editor would, against a scripted stand-in for
//! the language server, and checks that a script result reaches HTTP.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    base: String,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn start() -> Bridge {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/server.py");
    let mut child = Command::new(env!("CARGO_BIN_EXE_daml-ide-bridge"))
        .args(["--no-open", "--", "python3", fixture])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bridge starts");

    // The bridge announces its URL on stderr before doing anything else.
    let mut stderr = BufReader::new(child.stderr.take().expect("piped"));
    let mut line = String::new();
    stderr.read_line(&mut line).expect("announcement");
    let base = line
        .split_whitespace()
        .find(|w| w.starts_with("http://"))
        .expect("url in announcement")
        .to_string();
    // Keep draining stderr so the bridge never blocks writing to it.
    std::thread::spawn(move || {
        let mut sink = String::new();
        let _ = stderr.read_to_string(&mut sink);
    });

    let stdin = child.stdin.take().expect("piped");
    let stdout = BufReader::new(child.stdout.take().expect("piped"));
    Bridge { child, stdin, stdout, base }
}

impl Bridge {
    fn send(&mut self, msg: Value) {
        let body = serde_json::to_vec(&msg).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.stdin.write_all(&body).unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("Content-Length:") {
                length = rest.trim().parse().unwrap();
            }
        }
        let mut body = vec![0u8; length];
        self.stdout.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Reads responses until the one with this id arrives.
    fn response(&mut self, id: i64) -> Value {
        loop {
            let msg = self.recv();
            if msg["id"] == id {
                return msg;
            }
        }
    }
}

fn get(url: &str) -> String {
    // A dependency-free HTTP/1.0 GET is enough for a loopback test server.
    use std::io::BufWriter;
    use std::net::TcpStream;
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let mut stream = TcpStream::connect(authority).unwrap();
    {
        let mut w = BufWriter::new(&mut stream);
        write!(w, "GET /{path} HTTP/1.0\r\nHost: {authority}\r\n\r\n").unwrap();
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn a_script_result_reaches_the_browser() {
    let mut bridge = start();

    bridge.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let init = bridge.response(1);
    let commands = init["result"]["capabilities"]["executeCommandProvider"]["commands"]
        .as_array()
        .unwrap();
    assert!(
        commands.iter().any(|c| c == "daml.showResource"),
        "the bridge must advertise its own command: {commands:?}"
    );

    bridge.send(json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/codeLens",
                       "params": {"textDocument": {"uri": "file:///a.daml"}}}));
    bridge.response(2);

    bridge.send(json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
                       "params": {"textDocument": {"uri": "file:///a.daml"},
                                  "range": {"start": {"line": 7, "character": 0},
                                            "end": {"line": 7, "character": 0}}}}));
    let actions = bridge.response(3);
    let action = &actions["result"][0];
    assert_eq!(action["title"], "Show script results: Script: setup");

    bridge.send(json!({"jsonrpc": "2.0", "id": 4, "method": "workspace/executeCommand",
                       "params": action["command"]}));
    bridge.response(4);

    // The stand-in answers didOpen with the rendered HTML; give it a moment.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let index = get(&bridge.base.replace("http://", "http://"));
    assert!(index.contains("Script: setup"), "index should list the result:\n{index}");

    let id_start = index.find("/r/").unwrap() + 3;
    let id: String = index[id_start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    let token = bridge.base.split("token=").nth(1).unwrap().trim().to_string();
    let page = get(&format!("http://{}/r/{id}/body?token={token}",
                            bridge.base.split('/').nth(2).unwrap()));
    assert!(page.contains("<td>Iou</td>"), "page should carry the render:\n{page}");
    assert!(!page.contains("$webviewSrc"), "placeholders must be substituted");
}
```

- [ ] **Step 3: Run it**

```bash
cargo test --test proxy
```

Expected: pass. If the announcement line format changes, the test's URL
extraction is the first thing to fix.

- [ ] **Step 4: Commit**

```bash
git commit -am "test(bridge): end-to-end proxy test against a scripted server"
```

---

## Task 8: Zed extension integration

**Files:**
- Modify: `editors/zed/src/server.rs`
- Modify: `editors/zed/src/lib.rs`
- Modify: `editors/zed/extension.toml`

- [ ] **Step 1: Extend the settings and tests**

Add to `ServerSettings`:

```rust
    /// Proxy the language server through daml-ide-bridge so script results can
    /// be viewed in a browser.
    pub script_results: bool,
    pub bridge_path: Option<String>,
    pub bridge_args: Vec<String>,
```

`script_results` defaults to `true`, which `#[derive(Default)]` will not do for
a bool, so implement `Default` for `ServerSettings` by hand.

New tests in `server.rs`:

```rust
    #[test]
    fn wraps_the_server_in_the_bridge_when_one_is_available() {
        let settings = ServerSettings::default();
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            Some("/opt/bin/daml-ide-bridge".into()),
            &settings,
        )
        .unwrap();
        assert_eq!(cmd.program, "/opt/bin/daml-ide-bridge");
        assert_eq!(cmd.args[0], "--");
        assert_eq!(cmd.args[1], "/opt/dpm/bin/dpm");
        assert_eq!(cmd.args[2], "damlc");
    }

    #[test]
    fn runs_dpm_directly_when_no_bridge_is_available() {
        let cmd =
            resolve_command(Some("/opt/dpm/bin/dpm".into()), None, &ServerSettings::default())
                .unwrap();
        assert_eq!(cmd.program, "/opt/dpm/bin/dpm");
        assert_eq!(cmd.args[0], "damlc");
    }

    #[test]
    fn script_results_can_be_turned_off() {
        let settings = ServerSettings {
            script_results: false,
            ..Default::default()
        };
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            Some("/opt/bin/daml-ide-bridge".into()),
            &settings,
        )
        .unwrap();
        assert_eq!(cmd.program, "/opt/dpm/bin/dpm");
    }

    #[test]
    fn bridge_args_come_before_the_separator() {
        let settings = ServerSettings {
            bridge_args: vec!["--no-open".into()],
            ..Default::default()
        };
        let cmd = resolve_command(
            Some("/opt/dpm/bin/dpm".into()),
            Some("/opt/bin/daml-ide-bridge".into()),
            &settings,
        )
        .unwrap();
        assert_eq!(cmd.args[0], "--no-open");
        assert_eq!(cmd.args[1], "--");
    }
```

- [ ] **Step 2: Watch them fail, then change the signature**

`resolve_command` gains a `bridge_path: Option<String>` parameter and, when
`script_results` is on and a bridge is available, returns the bridge with
`bridge_args`, `--`, then the dpm command.

- [ ] **Step 3: Find the bridge in `lib.rs`**

In order: `bridge_path` setting, `worktree.which("daml-ide-bridge")`, then a
previously downloaded copy in the extension's work directory. Downloading a
release asset is deliberately left for a follow-up: until releases exist there
is nothing to download, and the fallback to plain `dpm` keeps the extension
fully usable.

- [ ] **Step 4: Verify**

```bash
cd editors/zed
cargo test && cargo clippy --all-targets -- -D warnings && cargo build --target wasm32-wasip1
```

- [ ] **Step 5: Commit**

```bash
git commit -am "feat: run the language server through daml-ide-bridge when present"
```

---

## Task 9: Workspace, CI and docs

**Files:**
- Create: `Cargo.toml` (workspace root)
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Create: `crates/daml-ide-bridge/README.md`

- [ ] **Step 1: Do not add a workspace root**

`editors/zed` builds to `wasm32-wasip1` and the bridge builds natively. A shared
workspace would force one target and one lock file on both, and Zed builds the
extension directory on its own. Keep the two crates independent, and give the
bridge its own CI job.

- [ ] **Step 2: Add the bridge job to CI**

```yaml
  bridge:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: crates/daml-ide-bridge
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
```

- [ ] **Step 3: Write `crates/daml-ide-bridge/README.md`**

Cover what it is, why it exists outside the editor, how to run it by hand
(`daml-ide-bridge -- dpm damlc multi-ide`), the `--no-open` flag, that it binds
only to loopback and requires a token, and the provenance of the vendored
`webview.js`/`webview.css` with their Apache-2.0 attribution.

- [ ] **Step 4: Update the top-level README**

Replace the "What you don't get" section: script results now work through the
bridge, in a browser rather than an editor panel, and explain why.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --manifest-path crates/daml-ide-bridge/Cargo.toml
git commit -am "ci: build and test the bridge; docs: describe script results"
```

---

## Completion criteria

- [ ] `cargo test` in `crates/daml-ide-bridge` covers framing, interception, the registry and the page shell
- [ ] Running `daml-ide-bridge -- dpm damlc multi-ide` in a Daml project and driving it like an editor produces a code action, and executing it serves a page with the rendered script result
- [ ] Editing the source updates the open page without a manual reload
- [ ] With the bridge absent, the extension behaves exactly as it did in phase 1
- [ ] Both CI jobs green
