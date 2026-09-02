//! Drives the bridge the way an editor would, against a scripted stand-in for
//! the language server, and checks that a script result lands in the pane.
//!
//! Using a stand-in rather than a real `damlc multi-ide` keeps this in `cargo
//! test`: the real server needs a Daml SDK and minutes of compilation, and the
//! part that can actually regress is the bridge.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    results: PathBuf,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = std::fs::remove_file(&self.results);
    }
}

fn start() -> Bridge {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/server.py");
    let results = std::env::temp_dir().join(format!(
        "daml-ide-bridge-proxy-{}-{:?}.md",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_daml-ide-bridge"))
        .args([
            "--results",
            results.to_str().unwrap(),
            "--",
            "python3",
            fixture,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the bridge starts");

    // The bridge announces the pane on stderr before doing anything else.
    let mut stderr = BufReader::new(child.stderr.take().expect("piped"));
    let mut line = String::new();
    stderr.read_line(&mut line).expect("an announcement");
    assert!(line.contains("script results in"), "unexpected: {line:?}");
    // Keep draining stderr so the bridge never blocks writing to it.
    std::thread::spawn(move || {
        let mut sink = String::new();
        let _ = stderr.read_to_string(&mut sink);
    });

    let stdin = child.stdin.take().expect("piped");
    let stdout = BufReader::new(child.stdout.take().expect("piped"));
    Bridge {
        child,
        stdin,
        stdout,
        results,
    }
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

    /// Reads until the response to this request arrives, collecting the
    /// notifications that pass by.
    fn response(&mut self, id: i64) -> (Value, Vec<Value>) {
        let mut notifications = Vec::new();
        for _ in 0..50 {
            let msg = self.recv();
            if msg["id"] == id {
                return (msg, notifications);
            }
            notifications.push(msg);
        }
        panic!("no response to request {id}");
    }

    fn pane(&self) -> String {
        std::fs::read_to_string(&self.results).unwrap_or_default()
    }
}

#[test]
fn a_script_result_reaches_the_pane() {
    let mut bridge = start();

    bridge.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let (init, _) = bridge.response(1);
    let commands = init["result"]["capabilities"]["executeCommandProvider"]["commands"]
        .as_array()
        .expect("the server's command list survives");
    assert!(
        commands.iter().any(|c| c == "daml.showResource"),
        "the bridge must advertise its own command, got {commands:?}"
    );
    assert!(
        commands.iter().any(|c| c == "typesignature.add"),
        "and must not drop the server's, got {commands:?}"
    );

    bridge.send(
        json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
               "params": {"textDocument": {"uri": "file:///a.daml"},
                          "range": {"start": {"line": 7, "character": 0},
                                    "end": {"line": 7, "character": 0}}}}),
    );
    // The bridge fetches the lenses itself, so the first attempt can race it.
    let mut action = Value::Null;
    for id in 3..12 {
        let (actions, _) = bridge.response(id);
        if let Some(found) = actions["result"].as_array().and_then(|a| {
            a.iter().find(|a| {
                a["title"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Show script results"))
            })
        }) {
            action = found.clone();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        bridge.send(
            json!({"jsonrpc": "2.0", "id": id + 1, "method": "textDocument/codeAction",
                   "params": {"textDocument": {"uri": "file:///a.daml"},
                              "range": {"start": {"line": 7, "character": 0},
                                        "end": {"line": 7, "character": 0}}}}),
        );
    }
    assert_eq!(
        action["title"], "Show script results: Script: setup",
        "the code action is the entry point the editor can always show"
    );

    bridge.send(
        json!({"jsonrpc": "2.0", "id": 90, "method": "workspace/executeCommand",
               "params": action["command"]}),
    );
    let (_, notifications) = bridge.response(90);
    assert!(
        notifications
            .iter()
            .any(|n| n["method"] == "window/showMessage"),
        "the editor is told where the pane is: {notifications:?}"
    );

    let mut pane = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        pane = bridge.pane();
        if pane.contains("## Transactions") {
            break;
        }
    }
    assert!(
        pane.contains("# Script: setup"),
        "the pane should be titled:\n{pane}"
    );
    assert!(
        pane.contains("| #2:1 | active |"),
        "the pane should carry the contract table:\n{pane}"
    );
    assert!(
        !pane.contains('<'),
        "no markup should survive into the pane:\n{pane}"
    );
}
