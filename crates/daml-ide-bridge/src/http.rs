//! A loopback HTTP server for the rendered script results.

use std::io::Write;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use tiny_http::{Header, Response, Server};

use crate::page;
use crate::resources::Registry;

pub struct Http {
    pub port: u16,
}

/// Binds to an OS-chosen port on the loopback interface and serves until the
/// process exits.
pub fn serve(registry: Arc<Registry>, token: String) -> std::io::Result<Http> {
    let server = Server::http("127.0.0.1:0").map_err(std::io::Error::other)?;
    let port = server
        .server_addr()
        .to_ip()
        .expect("an ip address for a tcp listener")
        .port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let registry = Arc::clone(&registry);
            let token = token.clone();
            std::thread::spawn(move || handle(request, &registry, &token));
        }
    });
    Ok(Http { port })
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a well formed header")
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

    let trimmed = path.trim_matches('/');
    let segments: Vec<&str> = if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    };

    match segments.as_slice() {
        [] => {
            let items: String = registry
                .list()
                .iter()
                .map(|r| {
                    format!(
                        r#"<li><a href="/r/{}?token={}">{}</a></li>"#,
                        r.id,
                        token,
                        escape(&r.title)
                    )
                })
                .collect();
            let body = if items.is_empty() {
                "<p>No script results yet. Run the “Show script results” code action in your editor.</p>".to_string()
            } else {
                format!("<ul>{items}</ul>")
            };
            let _ = request.respond(html(page::index(&body, token)));
        }
        ["assets", name] => {
            let (bytes, mime): (&[u8], &str) = match *name {
                "webview.js" => (include_bytes!("assets/webview.js"), "text/javascript"),
                "webview.css" => (include_bytes!("assets/webview.css"), "text/css"),
                "theme.css" => (include_bytes!("assets/theme.css"), "text/css"),
                _ => {
                    let _ =
                        request.respond(Response::from_string("not found").with_status_code(404));
                    return;
                }
            };
            let _ = request.respond(
                Response::from_data(bytes.to_vec()).with_header(header("Content-Type", mime)),
            );
        }
        ["r", id] => match registry.get(id) {
            Some(resource) => {
                let body = match resource.html.as_deref() {
                    Some(rendered) => page::render(rendered, id, token),
                    None => page::pending(&escape(&resource.title), id, token),
                };
                let _ = request.respond(html(body));
            }
            None => {
                let _ =
                    request.respond(Response::from_string("unknown result").with_status_code(404));
            }
        },
        ["r", id, "events"] => stream_events(request, registry, id),
        _ => {
            let _ = request.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// Titles come from the language server, so they are not attacker controlled,
/// but they do end up in HTML and can contain arbitrary Daml identifiers.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Server-sent events. tiny_http has no SSE helper, so the frames are written
/// straight to the socket.
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
        let frame = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(()) => {
                let running = registry.get(id).map(|r| r.running).unwrap_or(false);
                format!("data: {}\n\n", if running { "running" } else { "changed" })
            }
            // A comment frame keeps the connection alive and is how a closed
            // tab is noticed.
            Err(RecvTimeoutError::Timeout) => ":keepalive\n\n".to_string(),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if writer.write_all(frame.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_in_titles() {
        assert_eq!(
            escape("Script: <b>x</b> & y"),
            "Script: &lt;b&gt;x&lt;/b&gt; &amp; y"
        );
    }
}
