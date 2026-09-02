//! Every rewrite the bridge performs, with no I/O so it can be tested directly.

use std::collections::HashMap;

use serde_json::{json, Value};

/// The command the VS Code extension registers client-side. The bridge takes
/// the same name so a code lens the server emits works unchanged.
pub const SHOW_RESOURCE: &str = "daml.showResource";

#[derive(Debug)]
pub enum Outbound {
    ToServer(Value),
    ToClient(Value),
    /// The editor asked for a script result to be displayed.
    Show {
        title: String,
        uri: String,
    },
    /// The server rendered a virtual resource.
    ResourceChanged {
        uri: String,
        contents: String,
    },
    /// The server started recomputing a virtual resource.
    ResourceProgress {
        uri: String,
    },
}

/// What a request the client sent is waiting for, so the matching response can
/// be rewritten when it arrives.
#[derive(Debug, Clone)]
enum Pending {
    Initialize,
    CodeLens {
        document: String,
    },
    /// A lens request the bridge made on its own behalf. The editor never asked
    /// for it, so the response is cached and dropped rather than forwarded.
    InternalCodeLens {
        document: String,
    },
    CodeAction {
        document: String,
        range: Value,
    },
}

#[derive(Debug, Default)]
pub struct Interceptor {
    pending: HashMap<String, Pending>,
    /// Code lenses per document, kept so code actions can be synthesised from
    /// them. Zed's code lens support is opt-in; code actions always work.
    lenses: HashMap<String, Vec<Value>>,
    next_request: u64,
}

impl Interceptor {
    pub fn on_client_message(&mut self, msg: Value) -> Vec<Outbound> {
        let method = msg["method"].as_str().unwrap_or_default();

        if method == "workspace/executeCommand" && msg["params"]["command"] == SHOW_RESOURCE {
            let args = msg["params"]["arguments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let title = args
                .first()
                .and_then(Value::as_str)
                .unwrap_or("Script results");
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

        // Zed only sends textDocument/codeLens when the user turned code lenses
        // on, so without asking for them the bridge would never have anything
        // to build a code action from.
        let refresh = match method {
            "textDocument/didOpen" | "textDocument/didSave" | "textDocument/codeAction" => {
                let document = document_of(&msg);
                is_daml_source(&document).then_some(document)
            }
            _ => None,
        };

        if let Some(id) = id_key(&msg) {
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

        let mut out = vec![Outbound::ToServer(msg)];
        if let Some(document) = refresh {
            out.push(Outbound::ToServer(self.request_lenses(&document)));
        }
        out
    }

    /// Ask the server for this document's code lenses on the bridge's own
    /// behalf. Ranges go stale as the file is edited, and the first request can
    /// land before the package has compiled, so this is repeated rather than
    /// done once.
    fn request_lenses(&mut self, document: &str) -> Value {
        self.next_request += 1;
        let id = format!("daml-ide-bridge/{}", self.next_request);
        self.pending.insert(
            id.clone(),
            Pending::InternalCodeLens {
                document: document.to_string(),
            },
        );
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/codeLens",
            "params": {"textDocument": {"uri": document}},
        })
    }

    pub fn on_server_message(&mut self, msg: Value) -> Vec<Outbound> {
        match msg["method"].as_str() {
            Some("daml/virtualResource/didChange") => {
                return vec![Outbound::ResourceChanged {
                    uri: msg["params"]["uri"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    contents: msg["params"]["contents"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                }];
            }
            Some("daml/virtualResource/didProgress") | Some("daml/virtualResource/note") => {
                return vec![Outbound::ResourceProgress {
                    uri: msg["params"]["uri"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
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
                self.remember_lenses(document, &msg);
            }
            Pending::InternalCodeLens { document } => {
                self.remember_lenses(document, &msg);
                return Vec::new();
            }
            Pending::CodeAction { document, range } => {
                self.inject_actions(&mut msg, &document, &range)
            }
        }
        vec![Outbound::ToClient(msg)]
    }

    /// An empty result means the package has not compiled yet; keeping the
    /// previous lenses is better than forgetting them.
    fn remember_lenses(&mut self, document: String, msg: &Value) {
        match msg["result"].as_array() {
            Some(lenses) if !lenses.is_empty() => {
                self.lenses.insert(document, lenses.clone());
            }
            _ => {}
        }
    }

    fn inject_actions(&self, msg: &mut Value, document: &str, range: &Value) {
        let Some(lenses) = self.lenses.get(document) else {
            return;
        };
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

/// Request ids may be numbers or strings; normalise to a string key.
fn id_key(msg: &Value) -> Option<String> {
    match &msg["id"] {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Only real Daml sources have script results; the `daml://` virtual resources
/// the bridge itself opens must not trigger another round of requests.
fn is_daml_source(uri: &str) -> bool {
    uri.starts_with("file://") && uri.ends_with(".daml")
}

fn document_of(msg: &Value) -> String {
    msg["params"]["textDocument"]["uri"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Zed will not run a command the server does not claim to support.
fn advertise_command(msg: &mut Value) {
    let mut commands = msg["result"]["capabilities"]["executeCommandProvider"]["commands"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if commands.iter().any(|c| c == SHOW_RESOURCE) {
        return;
    }
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
    fn primes_the_lens_cache_when_a_daml_file_is_opened() {
        // Zed only asks for code lenses when the user opted into them, so the
        // bridge has to fetch them itself or it has nothing to build an action
        // from.
        let mut i = Interceptor::default();
        let out = i.on_client_message(json!({
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///a.daml", "text": ""}}
        }));
        let requests: Vec<&Value> = out
            .iter()
            .filter_map(|o| match o {
                Outbound::ToServer(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(requests.len(), 2, "{out:?}");
        assert_eq!(requests[1]["method"], "textDocument/codeLens");
        assert_eq!(
            requests[1]["params"]["textDocument"]["uri"],
            "file:///a.daml"
        );
    }

    #[test]
    fn does_not_prime_for_documents_that_are_not_daml_sources() {
        let mut i = Interceptor::default();
        for uri in ["file:///a.txt", "daml://compiler?x=1"] {
            let out = i.on_client_message(json!({
                "method": "textDocument/didOpen",
                "params": {"textDocument": {"uri": uri, "text": ""}}
            }));
            assert_eq!(out.len(), 1, "{uri} should just be forwarded: {out:?}");
        }
    }

    #[test]
    fn its_own_lens_response_is_cached_and_not_forwarded() {
        let mut i = Interceptor::default();
        let out = i.on_client_message(json!({
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///a.daml", "text": ""}}
        }));
        let Outbound::ToServer(request) = &out[1] else {
            panic!("{out:?}")
        };
        let id = request["id"].clone();

        let out = i.on_server_message(json!({"id": id, "result": [lens()]}));
        assert!(out.is_empty(), "the editor never asked for this: {out:?}");

        // And the cache is now good enough to answer a code action.
        i.on_client_message(json!({"id": 3, "method": "textDocument/codeAction",
                                   "params": {"textDocument": {"uri": "file:///a.daml"},
                                              "range": {"start": {"line": 7, "character": 0},
                                                        "end": {"line": 7, "character": 0}}}}));
        let out = i.on_server_message(json!({"id": 3, "result": []}));
        let Outbound::ToClient(msg) = out
            .iter()
            .find(|o| matches!(o, Outbound::ToClient(_)))
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            msg["result"][0]["title"],
            "Show script results: Script: setup"
        );
    }

    #[test]
    fn every_code_action_refreshes_the_lenses() {
        // Lens ranges go stale as the file is edited, and the first fetch can
        // land before the package compiles, so ask again each time.
        let mut i = Interceptor::default();
        let out = i.on_client_message(json!({"id": 3, "method": "textDocument/codeAction",
                                             "params": {"textDocument": {"uri": "file:///a.daml"},
                                                        "range": {"start": {"line": 1, "character": 0},
                                                                  "end": {"line": 1, "character": 0}}}}));
        let methods: Vec<&str> = out
            .iter()
            .filter_map(|o| match o {
                Outbound::ToServer(m) => m["method"].as_str(),
                _ => None,
            })
            .collect();
        assert_eq!(
            methods,
            vec!["textDocument/codeAction", "textDocument/codeLens"]
        );
    }

    #[test]
    fn saving_refreshes_the_lenses() {
        let mut i = Interceptor::default();
        let out = i.on_client_message(json!({
            "method": "textDocument/didSave",
            "params": {"textDocument": {"uri": "file:///a.daml"}}
        }));
        let methods: Vec<&str> = out
            .iter()
            .filter_map(|o| match o {
                Outbound::ToServer(m) => m["method"].as_str(),
                _ => None,
            })
            .collect();
        assert_eq!(
            methods,
            vec!["textDocument/didSave", "textDocument/codeLens"]
        );
    }

    #[test]
    fn advertises_the_show_resource_command() {
        let mut i = Interceptor::default();
        i.on_client_message(json!({"id": 1, "method": "initialize", "params": {}}));
        let out = i.on_server_message(json!({
            "id": 1,
            "result": {"capabilities": {"executeCommandProvider": {"commands": ["typesignature.add"]}}}
        }));
        let Outbound::ToClient(msg) = &out[0] else {
            panic!("{out:?}")
        };
        let commands = msg["result"]["capabilities"]["executeCommandProvider"]["commands"]
            .as_array()
            .unwrap();
        assert!(commands.iter().any(|c| c == "daml.showResource"));
        assert!(commands.iter().any(|c| c == "typesignature.add"));
    }

    #[test]
    fn adds_the_command_when_the_server_advertises_none() {
        let mut i = Interceptor::default();
        i.on_client_message(json!({"id": 1, "method": "initialize", "params": {}}));
        let out = i.on_server_message(json!({"id": 1, "result": {"capabilities": {}}}));
        let Outbound::ToClient(msg) = &out[0] else {
            panic!()
        };
        assert_eq!(
            msg["result"]["capabilities"]["executeCommandProvider"]["commands"][0],
            "daml.showResource"
        );
    }

    #[test]
    fn remembers_lenses_and_turns_them_into_code_actions() {
        let mut i = Interceptor::default();
        i.on_client_message(json!({"id": 2, "method": "textDocument/codeLens",
                             "params": {"textDocument": {"uri": "file:///a.daml"}}}));
        i.on_server_message(json!({"id": 2, "result": [lens()]}));

        i.on_client_message(json!({"id": 3, "method": "textDocument/codeAction",
                             "params": {"textDocument": {"uri": "file:///a.daml"},
                                        "range": {"start": {"line": 7, "character": 0},
                                                  "end": {"line": 7, "character": 0}}}}));
        let out = i.on_server_message(json!({"id": 3, "result": []}));
        let Outbound::ToClient(msg) = &out[0] else {
            panic!()
        };
        let action = &msg["result"][0];
        assert_eq!(action["title"], "Show script results: Script: setup");
        assert_eq!(action["command"]["command"], "daml.showResource");
    }

    #[test]
    fn does_not_offer_an_action_for_another_line() {
        let mut i = Interceptor::default();
        i.on_client_message(json!({"id": 2, "method": "textDocument/codeLens",
                             "params": {"textDocument": {"uri": "file:///a.daml"}}}));
        i.on_server_message(json!({"id": 2, "result": [lens()]}));
        i.on_client_message(json!({"id": 3, "method": "textDocument/codeAction",
                             "params": {"textDocument": {"uri": "file:///a.daml"},
                                        "range": {"start": {"line": 40, "character": 0},
                                                  "end": {"line": 40, "character": 0}}}}));
        let out = i.on_server_message(json!({"id": 3, "result": []}));
        let Outbound::ToClient(msg) = &out[0] else {
            panic!()
        };
        assert_eq!(msg["result"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn keeps_the_actions_the_server_already_returned() {
        let mut i = Interceptor::default();
        i.on_client_message(json!({"id": 2, "method": "textDocument/codeLens",
                             "params": {"textDocument": {"uri": "file:///a.daml"}}}));
        i.on_server_message(json!({"id": 2, "result": [lens()]}));
        i.on_client_message(json!({"id": 3, "method": "textDocument/codeAction",
                             "params": {"textDocument": {"uri": "file:///a.daml"},
                                        "range": {"start": {"line": 7, "character": 0},
                                                  "end": {"line": 7, "character": 0}}}}));
        let out =
            i.on_server_message(json!({"id": 3, "result": [{"title": "Add type signature"}]}));
        let Outbound::ToClient(msg) = &out[0] else {
            panic!()
        };
        assert_eq!(msg["result"][0]["title"], "Add type signature");
        assert_eq!(msg["result"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn handles_show_resource_without_forwarding_it() {
        let mut i = Interceptor::default();
        let out = i.on_client_message(json!({
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
        let out = i.on_client_message(json!({
            "id": 9, "method": "workspace/executeCommand",
            "params": {"command": "typesignature.add", "arguments": []}
        }));
        assert!(matches!(out[0], Outbound::ToServer(_)));
    }

    #[test]
    fn absorbs_virtual_resource_notifications() {
        let mut i = Interceptor::default();
        let out = i.on_server_message(json!({
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
    fn absorbs_progress_notifications() {
        let mut i = Interceptor::default();
        let out = i.on_server_message(json!({
            "method": "daml/virtualResource/didProgress",
            "params": {"uri": "daml://compiler?x=1"}
        }));
        assert!(matches!(out[0], Outbound::ResourceProgress { .. }));
    }

    #[test]
    fn passes_everything_else_through_untouched() {
        let mut i = Interceptor::default();
        let diag =
            json!({"method": "textDocument/publishDiagnostics", "params": {"uri": "file:///a"}});
        let out = i.on_server_message(diag.clone());
        let Outbound::ToClient(msg) = &out[0] else {
            panic!()
        };
        assert_eq!(msg, &diag);

        let hover = json!({"id": 5, "method": "textDocument/hover", "params": {}});
        let out = i.on_client_message(hover.clone());
        let Outbound::ToServer(msg) = &out[0] else {
            panic!()
        };
        assert_eq!(msg, &hover);
    }

    #[test]
    fn a_string_request_id_is_matched_too() {
        let mut i = Interceptor::default();
        i.on_client_message(json!({"id": "abc", "method": "initialize", "params": {}}));
        let out = i.on_server_message(json!({"id": "abc", "result": {"capabilities": {}}}));
        let Outbound::ToClient(msg) = &out[0] else {
            panic!()
        };
        assert_eq!(
            msg["result"]["capabilities"]["executeCommandProvider"]["commands"][0],
            "daml.showResource"
        );
    }
}
