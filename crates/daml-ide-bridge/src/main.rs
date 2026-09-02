//! Proxies the Daml language server and serves its script results to a browser.
//!
//! Zed cannot render them itself: an extension runs in WebAssembly, so it can
//! neither register the client-side command the server's code lens refers to
//! nor open a panel to draw the result in. This process sits between the editor
//! and `damlc multi-ide`, handles that command itself, and puts the HTML the
//! server produces on a loopback HTTP server instead.
//!
//! Usage: `daml-ide-bridge [--no-open] [--log PATH] -- <language server command...>`

mod framing;
mod http;
mod ids;
mod intercept;
mod log;
mod open;
mod page;
mod resources;

use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use crate::intercept::{Interceptor, Outbound};
use crate::resources::Registry;

fn main() -> std::io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let auto_open = !args.iter().any(|a| a == "--no-open");
    args.retain(|a| a != "--no-open");

    // An editor swallows the bridge's stderr into a log only it can show, so a
    // file is the only way to see what actually crossed the wire.
    let log_path = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var("DAML_IDE_BRIDGE_LOG").ok());
    if let Some(i) = args.iter().position(|a| a == "--log") {
        args.drain(i..=(i + 1).min(args.len() - 1));
    }

    let server_command: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => args[i + 1..].to_vec(),
        None => args,
    };
    if server_command.is_empty() {
        eprintln!(
            "usage: daml-ide-bridge [--no-open] [--log PATH] -- <language server command...>"
        );
        std::process::exit(2);
    }
    log::open(log_path.as_deref());
    log::line(&format!("start: {}", server_command.join(" ")));
    run(server_command, auto_open)
}

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
    log::line(&format!("http: {base}/?token={token}"));

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

    let interceptor = Arc::new(Mutex::new(Interceptor::default()));
    let context = Context {
        registry: Arc::clone(&registry),
        to_server,
        to_client,
        base,
        token,
        auto_open,
    };

    // server -> editor
    let mut child_stdout = BufReader::new(child.stdout.take().expect("piped"));
    let upstream = {
        let interceptor = Arc::clone(&interceptor);
        let context = context.clone();
        thread::spawn(move || {
            while let Ok(Some(msg)) = framing::read_message(&mut child_stdout) {
                log::message("server->editor", &msg);
                let outbound = interceptor.lock().unwrap().on_server_message(msg);
                dispatch(outbound, &context);
            }
        })
    };

    // editor -> server
    let mut stdin = BufReader::new(std::io::stdin());
    while let Ok(Some(msg)) = framing::read_message(&mut stdin) {
        log::message("editor->server", &msg);
        let outbound = interceptor.lock().unwrap().on_client_message(msg);
        dispatch(outbound, &context);
    }
    log::line("editor closed the connection");

    // The editor closed its end, so the server has no reason to stay up.
    let _ = child.kill();
    let _ = upstream.join();
    Ok(())
}

/// Everything `dispatch` needs, bundled so the signature stays readable.
#[derive(Clone)]
struct Context {
    registry: Arc<Registry>,
    to_server: Sender<Value>,
    to_client: Sender<Value>,
    base: String,
    token: String,
    auto_open: bool,
}

/// The only place that acts on an `Outbound`. What to react to was decided in
/// `intercept.rs`; this just carries it out.
fn dispatch(outbound: Vec<Outbound>, ctx: &Context) {
    for item in outbound {
        match item {
            Outbound::ToServer(msg) => {
                let _ = ctx.to_server.send(msg);
            }
            Outbound::ToClient(msg) => {
                // Logged again on the way out, so a rewrite shows up as a
                // before/after pair in the trace.
                log::message("bridge->editor", &msg);
                let _ = ctx.to_client.send(msg);
            }
            Outbound::Show { title, uri } => {
                log::line(&format!("show: {title} {uri}"));
                let first = !ctx.registry.is_known(&uri);
                let id = ctx.registry.register(&title, &uri);
                if first {
                    // Opening the virtual resource is what makes the server
                    // start rendering it, exactly as the VS Code extension
                    // does. It is never closed: closing it stops the updates.
                    let _ = ctx.to_server.send(json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didOpen",
                        "params": {"textDocument": {
                            "uri": uri, "languageId": "daml", "version": 0, "text": ""}}
                    }));
                }
                let url = format!("{}/r/{id}?token={}", ctx.base, ctx.token);
                eprintln!("daml-ide-bridge: {title} -> {url}");
                if ctx.auto_open && first {
                    open::url(&url);
                }
            }
            Outbound::ResourceChanged { uri, contents } => {
                log::line(&format!("rendered: {} bytes for {uri}", contents.len()));
                ctx.registry.update(&uri, &contents)
            }
            Outbound::ResourceProgress { uri } => ctx.registry.set_running(&uri),
        }
    }
}
