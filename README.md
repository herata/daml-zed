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

## Failing scripts

Turn on `autorun_scripts` and every Daml `Script` is evaluated when you open the
file, so a failure becomes an ordinary red squiggle on the declaration:

```
Script execution failed on commit at Test:24:12:
  Attempt to fetch or exercise a contract not visible to the reading parties.
  Contract: #0:0 (Main:Asset@37a33fb2…)
  actAs: 'Bob'
  Disclosed to: 'Alice'

Committed transactions:
  TX 0 1970-01-01T00:00:00Z (Test:18:14)
  #0:0
  └─> 'Alice' creates Main:Asset@37a33fb2… with issuer = 'Alice'; …
```

The reason, the transactions committed before the failure and the disclosure
are all in the diagnostic. No sidecar involved. It is off by default because it
costs a full evaluation of every script in the file each time it is opened.

## Script results


Script results answer a different question from the one above: not *did it
pass* but *what did it actually do*. A passing script produces a transaction
tree and a table of who ends up seeing which contract, and neither
`dpm test` nor a diagnostic shows you that. If you only need pass or fail,
`autorun_scripts` is enough and you can skip the rest of this section.

They render in an editor pane rather than a webview.

Invoke the **Show script results** code action on a Daml `Script`. The result is
written to `.daml/ide/script-results.md`; open that file once, split beside your
source, and it updates on every click and on every edit. Zed's
`markdown: open preview` renders the contract tables.

```
# Script: setup

`test/daml/Test.daml`

## Transactions

    TX 0 1970-01-01T00:00:00Z (Test:18:14)
    #0:0
    │   consumed by: #1:0
    └─> 'Alice' creates Main:Asset@37a33fb2…
                with
                  issuer = 'Alice'; owner = 'Alice'; name = "TV"

## Contracts

| id   | status   | issuer  | owner   | name | Alice | Bob |
|------|----------|---------|---------|------|-------|-----|
| #2:1 | active   | 'Alice' | 'Alice' | "TV" | S     | W   |
```

This runs outside the editor because it has to. In VS Code the extension opens
a webview and subscribes to a `daml://` virtual resource; a Zed extension runs
in WebAssembly, so it can neither register the client-side command the lens
refers to nor open a panel, and Zed's LSP client does not advertise
`window/showDocument`. So a sidecar, [`daml-ide-bridge`](crates/daml-ide-bridge),
proxies the language server and writes the pane.

The bridge is optional. Without it everything else still works and only script
results are missing. Build and install it with:

```sh
cargo install --path crates/daml-ide-bridge
```

Set `script_results` to `false` to run the language server directly even when a
bridge is on `PATH`.

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

Building a dev extension needs `rustup`, a Rust of 1.82 or newer as the
**default** toolchain, and the `wasm32-wasip2` target. Zed installs the target
itself, but only into the default toolchain — a directory override does not
apply, and an older default fails with `no prebuilt artifacts available for
target 'wasm32-wasip2'`.

```sh
rustup default stable
rustup target add wasm32-wasip2
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
| `autorun_scripts` | bool | `false` | Evaluate every script on open, so failures appear as diagnostics |
| `script_results` | bool | `true` | Run the language server behind `daml-ide-bridge` when one is available |
| `bridge_path` | string | unset | Where to find `daml-ide-bridge`, if not on `PATH` |
| `bridge_args` | list of strings | `[]` | Passed to the bridge, e.g. `["--log", "/tmp/bridge.log"]` |

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
| `editors/zed/` | The Zed extension, compiled to `wasm32-wasip2` |
| `editors/zed/src/server.rs` | Argument construction and `dpm` lookup. No Zed API calls, so it is unit tested |
| `editors/zed/src/lib.rs` | The only code that talks to Zed. Not automatically testable |
| `editors/zed/languages/daml/` | tree-sitter queries |
| `editors/zed/testdata/sample.daml` | Exercises every Daml construct; CI checks it parses and that every query compiles against it |
| `crates/daml-ide-bridge/` | The script-results sidecar: an LSP proxy that writes the pane |

The grammar lives in a separate repository,
[tree-sitter-daml](https://github.com/herata/tree-sitter-daml), because it is a
fork of [tree-sitter-haskell](https://github.com/tree-sitter/tree-sitter-haskell)
and needs to keep merging from upstream. `extension.toml` pins the revision.

## Development

```sh
cd editors/zed
cargo test                          # the pure logic in server.rs
cargo build --target wasm32-wasip2  # what Zed loads
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
- [ ] `debug: open language server logs` shows the bridge in front of
      `dpm damlc multi-ide --telemetry-ignored --log-level=Warning`, or just
      the latter if no bridge is installed
- [ ] With the bridge installed, the "Show script results" code action on a
      `Script` writes `.daml/ide/script-results.md`, and Zed shows a
      notification with the path
- [ ] With that file open, editing the script updates it without any manual
      reload, and clicking a different script switches the pane

## License

Apache-2.0.
