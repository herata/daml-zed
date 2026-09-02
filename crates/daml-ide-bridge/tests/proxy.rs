//! Drives the bridge the way an editor would, against a scripted stand-in for
//! the language server, and checks that a script result reaches HTTP.
//!
//! Using a stand-in rather than a real `damlc multi-ide` keeps this in `cargo
//! test`: the real server needs a Daml SDK and minutes of compilation, and the
//! part that can actually regress is the bridge.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    authority: String,
    token: String,
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
        .expect("the bridge starts");

    // The bridge announces its URL on stderr before doing anything else.
    let mut stderr = BufReader::new(child.stderr.take().expect("piped"));
    let mut line = String::new();
    stderr.read_line(&mut line).expect("an announcement");
    let url = line
        .split_whitespace()
        .find(|w| w.starts_with("http://"))
        .unwrap_or_else(|| panic!("no url in {line:?}"))
        .to_string();
    let authority = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .expect("an authority")
        .to_string();
    let token = url.split("token=").nth(1).expect("a token").to_string();

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
        authority,
        token,
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

    /// Reads until the response to this request arrives.
    fn response(&mut self, id: i64) -> Value {
        for _ in 0..50 {
            let msg = self.recv();
            if msg["id"] == id {
                return msg;
            }
        }
        panic!("no response to request {id}");
    }

    /// A dependency-free HTTP/1.0 GET; enough for a loopback test server.
    fn get(&self, path: &str) -> String {
        let mut stream = TcpStream::connect(&self.authority).expect("the http server is up");
        let request = format!(
            "GET {path} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.authority
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
}

#[test]
fn a_script_result_reaches_the_browser() {
    let mut bridge = start();

    bridge.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let init = bridge.response(1);
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
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/codeLens",
                       "params": {"textDocument": {"uri": "file:///a.daml"}}}),
    );
    bridge.response(2);

    bridge.send(
        json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
                       "params": {"textDocument": {"uri": "file:///a.daml"},
                                  "range": {"start": {"line": 7, "character": 0},
                                            "end": {"line": 7, "character": 0}}}}),
    );
    let actions = bridge.response(3);
    let action = actions["result"][0].clone();
    assert_eq!(
        action["title"], "Show script results: Script: setup",
        "the code action is the entry point Zed can always show"
    );

    bridge.send(
        json!({"jsonrpc": "2.0", "id": 4, "method": "workspace/executeCommand",
                       "params": action["command"]}),
    );
    bridge.response(4);

    // The stand-in answers the resulting didOpen with the rendered HTML.
    let mut page = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let index = bridge.get(&format!("/?token={}", bridge.token));
        let Some(start) = index.find("/r/") else {
            continue;
        };
        let id: String = index[start + 3..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        page = bridge.get(&format!("/r/{id}?token={}", bridge.token));
        if page.contains("<td>Iou</td>") {
            break;
        }
    }

    assert!(
        page.contains("<td>Iou</td>"),
        "the page should carry the render:\n{page}"
    );
    assert!(
        !page.contains("$webviewSrc") && !page.contains("$webviewCss"),
        "the asset placeholders must be substituted:\n{page}"
    );
    assert!(
        page.contains(r#"class="hide_archived hide_transaction""#),
        "the body class decides which view is shown:\n{page}"
    );
}

#[test]
fn the_token_is_required() {
    let bridge = start();
    assert!(bridge.get("/").contains("403"));
    assert!(bridge.get("/?token=wrong").contains("403"));
    assert!(bridge
        .get(&format!("/?token={}", bridge.token))
        .contains("200"));
}
