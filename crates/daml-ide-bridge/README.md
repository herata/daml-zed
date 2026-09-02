# daml-ide-bridge

Serves Daml script results to a browser by proxying the Daml language server.

## Why it exists

In VS Code, Daml Studio shows script results in a webview. The language server
returns a code lens bound to a command the extension registers on the client
side; the extension opens a panel, subscribes to a `daml://` virtual resource,
and renders the HTML the server pushes.

None of that is available to a Zed extension. Extensions run in WebAssembly, so
they can neither register a client-side command nor open a panel, and Zed's LSP
client does not advertise `window/showDocument`. The rendering has to happen
outside the editor, which is what this process is for.

It is editor-agnostic: anything that speaks LSP over stdio can put it in front
of `damlc multi-ide` and get the same behaviour.

## Running it

```sh
daml-ide-bridge -- dpm damlc multi-ide --telemetry-ignored
```

It prints the URL of its HTTP server on stderr and then relays LSP on stdio.
Invoke the "Show script results" code action on a Daml `Script`, and the page
opens in your browser and updates itself whenever the server re-renders.

`--no-open` suppresses the automatic browser launch; the URL is still printed.

## What it does to the protocol

| Direction | Message | Change |
| --- | --- | --- |
| server → editor | `initialize` result | adds `daml.showResource` to `executeCommandProvider.commands`, because Zed will not run a command the server does not claim |
| server → editor | `textDocument/codeLens` result | passed through, and remembered |
| server → editor | `textDocument/codeAction` result | a "Show script results" action is injected for any remembered lens on the requested lines |
| editor → server | `workspace/executeCommand` for `daml.showResource` | handled here, never forwarded |
| server → editor | `daml/virtualResource/*` | absorbed; the HTML goes to the browser instead |
| both | everything else | untouched |

The code action matters because Zed's code lens support is opt-in
(`"code_lens": "on"`), while code actions always work.

## Security

The HTTP server binds to `127.0.0.1` on a port the OS chooses. Loopback is
reachable by every other process on the machine and these pages contain project
source, so each request must carry a random per-process token. The links the
bridge prints and opens include it.

## Vendored files

`src/assets/webview.js` and `src/assets/webview.css` are copied verbatim from
[`digital-asset/daml`](https://github.com/digital-asset/daml), at
`sdk/compiler/daml-extension/src/`. They are Apache-2.0, and their copyright
headers are left intact. The HTML damlc renders references them through
`$webviewSrc` and `$webviewCss` placeholders that the client is expected to fill
in, and they implement the view toggles the rendered page's buttons call.

`src/assets/theme.css` is ours: it defines the VS Code theme variables the
rendered HTML refers to by name, for both colour schemes.

## Re-checking the protocol after a Daml upgrade

```sh
DAML_PROJECT=/path/to/built/project ./scripts/probe-protocol.py
```

It records the code lens, the virtual resource notification and the rendered
HTML, which is where every claim in the table above came from.

## Development

```sh
cargo test    # unit tests plus an end-to-end test against a scripted server
```

`tests/fixtures/server.py` replays the recorded protocol, so the end-to-end test
needs neither a Daml SDK nor a compile. `src/intercept.rs` holds every rewrite
and does no I/O; keep it that way.
