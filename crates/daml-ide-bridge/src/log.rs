//! An optional trace of everything that crosses the proxy.
//!
//! An editor captures the bridge's stderr into a log only the editor can show,
//! which makes diagnosing "nothing happened" needlessly hard. Pointing `--log`
//! (or `DAML_IDE_BRIDGE_LOG`) at a file gives a record anyone can read.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

pub fn open(path: Option<&str>) {
    let file = path.and_then(|path| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map(Mutex::new)
            .ok()
    });
    let _ = SINK.set(file);
}

pub fn line(text: &str) {
    let Some(Some(file)) = SINK.get() else { return };
    let Ok(mut file) = file.lock() else { return };
    let _ = writeln!(file, "{text}");
    let _ = file.flush();
}

/// One line per message: enough to follow the conversation without dumping
/// nine kilobytes of rendered HTML into the log.
pub fn message(direction: &str, msg: &Value) {
    if matches!(SINK.get(), None | Some(None)) {
        return;
    }
    let method = msg["method"].as_str().unwrap_or("");
    let id = match &msg["id"] {
        Value::Null => String::new(),
        other => format!(" id={other}"),
    };
    let detail = match method {
        "" => {
            // A response. Say something about its shape, since the method is
            // only known to the interceptor's pending table.
            match &msg["result"] {
                Value::Array(items) => format!(" result=[{} items]", items.len()),
                Value::Null if msg.get("error").is_some() => {
                    format!(" error={}", msg["error"]["message"])
                }
                Value::Null => " result=null".to_string(),
                _ => " result=object".to_string(),
            }
        }
        "textDocument/codeAction"
        | "textDocument/codeLens"
        | "textDocument/didOpen"
        | "textDocument/didSave" => {
            format!(" uri={}", msg["params"]["textDocument"]["uri"])
        }
        "workspace/executeCommand" => format!(" command={}", msg["params"]["command"]),
        _ => String::new(),
    };
    line(&format!("{direction} {method}{id}{detail}"));
}
