# daml-ide-bridge

Phase 2. Not written yet.

An LSP proxy that sits between the editor and `dpm damlc multi-ide`, intercepts
the `daml/virtualResource/didChange` notifications carrying rendered script
results, and serves them over a local HTTP server so they can be watched in a
browser next to the editor.

It lives in this repository rather than its own because the extension will
download the matching binary from this repository's releases, so the two are
version-locked and a single tag has to cover both.

See `docs/superpowers/specs/2026-09-02-daml-zed-design.md`, section 9.
