//! Turning the server's rendered HTML into a page a browser can load.
//!
//! The rendered document is served as-is rather than embedded in a wrapper.
//! Its `<body class="hide_transaction …">` is what `webview.css` keys off, so
//! moving the content into a `<div>` would silently show both views at once.
//! Everything the bridge adds goes into `<head>` instead.

/// A shim for the one VS Code API `webview.js` uses, plus live reload.
///
/// `webview.js` starts with `const vscode = acquireVsCodeApi()` and posts the
/// user's view choices back to the extension host. In a browser there is no
/// host, so the shim keeps those choices in `localStorage` and restores them
/// after a reload - otherwise every re-render would throw the reader back to
/// the default view.
fn bridge_script(id: &str, token: &str) -> String {
    format!(
        r#"
(function () {{
  var KEY = 'daml-bridge-view';
  window.acquireVsCodeApi = function () {{
    return {{ postMessage: save, getState: function () {{ return null; }}, setState: function () {{}} }};
  }};
  function save() {{
    try {{ localStorage.setItem(KEY, document.body.className); }} catch (e) {{}}
  }}
  function restore() {{
    try {{
      var saved = localStorage.getItem(KEY);
      if (saved) document.body.className = saved;
    }} catch (e) {{}}
    check('show_archived', 'hide_archived');
    check('show_detailed_disclosure', 'hidden_disclosure');
  }}
  function check(boxId, hiddenClass) {{
    var box = document.getElementById(boxId);
    if (box) box.checked = !document.body.classList.contains(hiddenClass);
  }}
  document.addEventListener('DOMContentLoaded', function () {{
    restore();
    var bar = document.createElement('div');
    bar.id = 'daml-bridge-status';
    bar.textContent = 'connecting';
    document.body.appendChild(bar);
    var es = new EventSource('/r/{id}/events?token={token}');
    es.onopen = function () {{ bar.textContent = 'live'; }};
    es.onerror = function () {{ bar.textContent = 'disconnected'; }};
    es.onmessage = function (e) {{
      if (e.data === 'running') {{ bar.textContent = 'running'; return; }}
      save();
      location.reload();
    }};
  }});
}})();
"#
    )
}

/// Fill in the placeholders damlc leaves for the client's own copies of the
/// view script and stylesheet, and add what the browser needs.
pub fn render(html: &str, id: &str, token: &str) -> String {
    let js = format!("/assets/webview.js?token={token}");
    let css = format!("/assets/webview.css?token={token}");
    let shim = format!("<script>{}</script>", bridge_script(id, token));
    let theme = format!(r#"<link rel="stylesheet" href="/assets/theme.css?token={token}">"#);

    // The shim has to run before webview.js, which calls acquireVsCodeApi at
    // the top level, so it is spliced in ahead of that script tag.
    let mut out = html.replace(
        r#"<script src="$webviewSrc""#,
        &format!(r#"{shim}<script src="{js}""#),
    );
    // Whatever the surrounding markup looks like, the shim has to end up in the
    // document. If the tag was not shaped as expected, put it as early as
    // possible instead.
    if !out.contains("acquireVsCodeApi") {
        out = match out.find("<head>") {
            Some(i) => {
                let (before, after) = out.split_at(i + "<head>".len());
                format!("{before}{shim}{after}")
            }
            None => format!("{shim}{out}"),
        };
    }
    out = out.replace("$webviewSrc", &js).replace("$webviewCss", &css);
    if out.contains("</head>") {
        out.replacen("</head>", &format!("{theme}</head>"), 1)
    } else {
        format!("{theme}{out}")
    }
}

/// The listing at `/`.
pub fn index(body: &str, token: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Daml script results</title>
<link rel="stylesheet" href="/assets/theme.css?token={token}"></head>
<body><h1>Daml script results</h1>{body}</body></html>"#
    )
}

/// Shown for a result the server has not rendered yet.
pub fn pending(title: &str, id: &str, token: &str) -> String {
    let shim = format!("<script>{}</script>", bridge_script(id, token));
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>{title}</title>
<link rel="stylesheet" href="/assets/theme.css?token={token}">{shim}</head>
<body><p>Running {title}…</p></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real render captured by scripts/probe-protocol.py.
    const RENDERED: &str = r#"<!DOCTYPE HTML>
<html><head><style>.da-code {}</style><script src="$webviewSrc"></script><link rel="stylesheet" href="$webviewCss"></head><body class="hide_archived hide_transaction"><div>hi</div></body></html>"#;

    #[test]
    fn substitutes_the_asset_placeholders() {
        let out = render(RENDERED, "abc", "tok");
        assert!(!out.contains("$webviewSrc"));
        assert!(!out.contains("$webviewCss"));
        assert!(out.contains("/assets/webview.js?token=tok"));
        assert!(out.contains("/assets/webview.css?token=tok"));
    }

    #[test]
    fn adds_the_theme_stylesheet() {
        assert!(render(RENDERED, "abc", "tok").contains("/assets/theme.css?token=tok"));
    }

    #[test]
    fn keeps_the_body_class_the_server_chose() {
        // The class decides which of the two views is visible; losing it shows both.
        assert!(
            render(RENDERED, "abc", "tok").contains(r#"class="hide_archived hide_transaction""#)
        );
    }

    #[test]
    fn defines_the_vscode_shim_before_webview_js_runs() {
        let out = render(RENDERED, "abc", "tok");
        let shim = out.find("acquireVsCodeApi").expect("shim present");
        let script = out.find("/assets/webview.js").expect("script present");
        assert!(
            shim < script,
            "the shim must be parsed first, or webview.js throws"
        );
    }

    #[test]
    fn subscribes_to_this_resource() {
        let out = render(RENDERED, "abc", "tok");
        assert!(out.contains("/r/abc/events?token=tok"));
    }

    #[test]
    fn survives_html_without_the_expected_tags() {
        let out = render("<html><head></head><body>x</body></html>", "abc", "tok");
        assert!(out.contains("theme.css"));
        assert!(out.contains("acquireVsCodeApi"));
    }
}
