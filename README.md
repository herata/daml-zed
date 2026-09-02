# daml-zed

[Daml](https://www.digitalasset.com/developers) support for the
[Zed](https://zed.dev) editor.

## What you get

Syntax highlighting from a Daml-specific tree-sitter grammar, plus everything
`damlc multi-ide` provides: completion, type on hover, diagnostics, go to
definition, rename, and cross-package jump-to-definition in multi-package
projects.

The outline (`cmd-shift-o`) lists templates, choices, interfaces, interface
instances and exceptions, which is how a Daml file is usually navigated.

## What you don't get

**Script results.** In VS Code, Daml Studio renders them in a webview: the
server returns a code lens bound to a client-side command, the extension opens
a panel and receives the rendered HTML over `daml/virtualResource/didChange`.
A Zed extension runs in WebAssembly and can neither register client-side
commands nor open a panel, and Zed's LSP client does not advertise
`window/showDocument`. So this cannot be built inside the extension. A sidecar
that proxies the language server and serves the results to a browser is planned
as phase 2; see `docs/superpowers/specs/`.

## Requirements

Daml SDK 3.4 or newer with `dpm` on your `PATH`. Install it with:

```sh
curl https://get.digitalasset.com/install/install.sh | sh
```

The extension runs `dpm damlc multi-ide`. `multi-ide` is always used: it can
only be turned off with the legacy Daml Assistant, which this extension does
not target.

## Installation

Until the extension is in the Zed registry, install it from a checkout:

```sh
git clone https://github.com/herata/daml-zed
```

Then in Zed: `cmd-shift-p` → `zed: install dev extension` → pick
`daml-zed/editors/zed`.

Building a dev extension needs `rustup` with the `wasm32-wasip1` target:

```sh
rustup target add wasm32-wasip1
```

## Settings

```json
{
  "lsp": {
    "daml-language-server": {
      "settings": {
        "log_level": "Warning",
        "extra_arguments": ["--ghc-option", "-Wall"]
      }
    }
  }
}
```

| Setting | Values | Default | Meaning |
| --- | --- | --- | --- |
| `log_level` | `Debug`, `Info`, `Warning`, `Error` | `Warning` | Passed to `damlc multi-ide` as `--log-level` |
| `extra_arguments` | list of strings | `[]` | Appended to the `damlc multi-ide` command line |

If `dpm` is not on your `PATH`, or you need the legacy `daml` assistant or an
SDK older than 3.4, point the extension at a binary directly. This bypasses
everything above, so pass the arguments yourself:

```json
{
  "lsp": {
    "daml-language-server": {
      "binary": {
        "path": "/Users/me/.dpm/bin/dpm",
        "arguments": ["damlc", "multi-ide", "--telemetry-ignored"]
      }
    }
  }
}
```

## Telemetry

The extension always passes `--telemetry-ignored`, so the language server sends
nothing. The VS Code extension asks for consent through a dialog; a Zed
extension has no way to show one, and sending telemetry without asking is not
an option, so it is simply off.

## Repository layout

| Path | What it is |
| --- | --- |
| `editors/zed/` | The Zed extension, compiled to `wasm32-wasip1` |
| `editors/zed/src/server.rs` | Argument construction and `dpm` lookup. No Zed API calls, so it is unit tested |
| `editors/zed/src/lib.rs` | The only code that talks to Zed. Not automatically testable |
| `editors/zed/languages/daml/` | tree-sitter queries |
| `editors/zed/testdata/sample.daml` | Exercises every Daml construct; CI checks it parses and that every query compiles against it |
| `crates/daml-ide-bridge/` | Phase 2, the script-results sidecar. Not written yet |

The grammar lives in a separate repository,
[tree-sitter-daml](https://github.com/herata/tree-sitter-daml), because it is a
fork of [tree-sitter-haskell](https://github.com/tree-sitter/tree-sitter-haskell)
and needs to keep merging from upstream. `extension.toml` pins the revision.

## Development

```sh
cd editors/zed
cargo test                          # the pure logic in server.rs
cargo build --target wasm32-wasip1  # what Zed loads
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

The WebAssembly half cannot be tested automatically. After changing it, install
the dev extension and walk this list against `testdata/sample.daml`:

- [ ] The file is recognised as `Daml`
- [ ] `template`, `with`, `where`, `signatory`, `choice`, `controller`,
      `interface`, `viewtype`, `exception`, `try`, `catch` are keyword-coloured
- [ ] Template, interface and exception names are type-coloured; choice names
      are constructor-coloured
- [ ] `cmd-shift-o` lists the templates, their choices, the interface, the
      interface instance and the exception
- [ ] Breaking a type produces a diagnostic, and fixing it clears it
- [ ] Hover shows a type
- [ ] Go to definition reaches the standard library
- [ ] Completion renders as `name : Type`
- [ ] `cmd-/` comments with `-- `
- [ ] In a multi-package project, go to definition crosses package boundaries
- [ ] `debug: open language server logs` shows
      `dpm damlc multi-ide --telemetry-ignored --log-level=Warning`

## License

Apache-2.0.
