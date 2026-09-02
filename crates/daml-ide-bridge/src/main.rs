//! Proxies the Daml language server so script results can be read in the editor.
//!
//! Daml Studio shows them in a VS Code webview: the server returns a code lens
//! bound to a command the extension registers on the client side, the extension
//! opens a panel and receives the rendered HTML over a `daml/virtualResource`
//! notification. A Zed extension runs in WebAssembly and can do neither.
//!
//! So this process sits between the editor and `damlc multi-ide`, handles that
//! command itself, and writes the result to a Markdown file the editor keeps
//! open. The editor reloads an unmodified buffer when the file changes on disk,
//! which is what makes the pane live.
//!
//! Usage: `daml-ide-bridge [--results PATH] [--log PATH] -- <server command...>`

mod framing;
mod intercept;
mod log;
mod markdown;
mod results;

use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use crate::intercept::{Interceptor, Outbound};
use crate::results::Pane;

fn main() -> std::io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let results = take_option(&mut args, "--results");
    // An editor swallows the bridge's stderr into a log only it can show, so a
    // file is the only way to see what actually crossed the wire.
    let log_path =
        take_option(&mut args, "--log").or_else(|| std::env::var("DAML_IDE_BRIDGE_LOG").ok());

    let server_command: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => args[i + 1..].to_vec(),
        None => args,
    };
    if server_command.is_empty() {
        eprintln!("usage: daml-ide-bridge [--results PATH] [--log PATH] -- <server command...>");
        std::process::exit(2);
    }

    log::open(log_path.as_deref());
    log::line(&format!("start: {}", server_command.join(" ")));
    run(server_command, results.as_deref())
}

/// Removes `--name VALUE` from the arguments and returns the value.
fn take_option(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    let value = args.get(i + 1).cloned();
    args.drain(i..=(i + 1).min(args.len() - 1));
    value
}

fn run(server_command: Vec<String>, results: Option<&str>) -> std::io::Result<()> {
    let mut child: Child = Command::new(&server_command[0])
        .args(&server_command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let root = std::env::current_dir()?;
    let pane = Arc::new(Pane::new(root, results));
    eprintln!(
        "daml-ide-bridge: script results in {}",
        pane.path().display()
    );
    log::line(&format!("results file: {}", pane.path().display()));

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
        pane,
        to_server,
        to_client,
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

#[derive(Clone)]
struct Context {
    pane: Arc<Pane>,
    to_server: Sender<Value>,
    to_client: Sender<Value>,
}

impl Context {
    /// The editor is the one place the reader is certainly looking.
    fn tell_the_editor(&self, message: String) {
        let _ = self.to_client.send(json!({
            "jsonrpc": "2.0",
            "method": "window/showMessage",
            "params": {"type": 3, "message": message}
        }));
    }
}

/// The only place that acts on an `Outbound`. What to react to was decided in
/// `intercept.rs`; this carries it out.
fn dispatch(outbound: Vec<Outbound>, ctx: &Context) {
    for item in outbound {
        match item {
            Outbound::ToServer(msg) => {
                let _ = ctx.to_server.send(msg);
            }
            Outbound::ToClient(msg) => {
                log::message("bridge->editor", &msg);
                let _ = ctx.to_client.send(msg);
            }
            Outbound::Show { title, uri } => {
                log::line(&format!("show: {title} {uri}"));
                // Claim the pane before asking for the render. The server can
                // answer faster than this thread continues, and a render for a
                // script the pane has not been told about is dropped.
                let shown = ctx.pane.show(&uri, &title);
                // Opening the virtual resource is what makes the server start
                // rendering it, exactly as the VS Code extension does. It is
                // never closed: closing it would stop the updates.
                let _ = ctx.to_server.send(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {"textDocument": {
                        "uri": uri, "languageId": "daml", "version": 0, "text": ""}}
                }));
                match shown {
                    Ok(()) => {
                        ctx.tell_the_editor(format!("{title} → {}", ctx.pane.path().display()))
                    }
                    Err(e) => {
                        log::line(&format!("could not write the results file: {e}"));
                        ctx.tell_the_editor(format!(
                            "Daml: could not write {}: {e}",
                            ctx.pane.path().display()
                        ));
                    }
                }
            }
            Outbound::ResourceChanged { uri, contents } => match ctx.pane.update(&uri, &contents) {
                Ok(true) => log::line(&format!("wrote markdown from {} bytes", contents.len())),
                Ok(false) => log::line("render for a script the pane is not showing"),
                Err(e) => log::line(&format!("could not write the results file: {e}")),
            },
            Outbound::ResourceProgress { uri } => log::line(&format!("running: {uri}")),
        }
    }
}
