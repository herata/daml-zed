# Releasing

Two repositories move together: the grammar and this one. The extension pins a
grammar revision, so the grammar is always released first.

## 1. Release the grammar

```sh
cd ../tree-sitter-daml
npx tree-sitter generate && npx tree-sitter test && ./script/parse-real-world.sh
git tag v0.x.y && git push --tags
git rev-parse HEAD
```

## 2. Point the extension at it

Update `[grammars.daml] commit` in `editors/zed/extension.toml` to that SHA, and
bump `version` in the same file.

```sh
cd editors/zed
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --target wasm32-wasip1
```

Install the dev extension and walk the manual checklist in the README. The
WebAssembly half has no automated coverage, so this step is not optional.

## 3. Tag this repository

```sh
git commit -am "release: v0.x.y"
git tag v0.x.y && git push --tags
```

## 4. Submit to the Zed registry

Only needed for the first release; afterwards a bot bumps the version, or you
update it by hand.

```sh
gh repo fork zed-industries/extensions --clone
cd extensions
git submodule add https://github.com/herata/daml-zed.git extensions/daml
git add extensions/daml
```

Add to `extensions.toml`. The `version` must match `editors/zed/extension.toml`,
and the submodule URL must be HTTPS, not SSH:

```toml
[daml]
submodule = "extensions/daml"
path = "editors/zed"
version = "0.x.y"
```

Sorting `extensions.toml` and `.gitmodules` is required before opening the PR,
and the submodule must point at a commit that is on a branch:

```sh
pnpm install
pnpm sort-extensions
git -C extensions/daml branch --contains HEAD
gh pr create --title "Add Daml extension"
```

## Merging upstream tree-sitter-haskell

```sh
cd ../tree-sitter-daml
git fetch upstream
git merge upstream/master
npx tree-sitter generate && npx tree-sitter test && ./script/parse-real-world.sh
```

Expect delete/modify conflicts for the language bindings, which this fork
dropped; resolve them with `git rm`. The Daml changes outside `grammar/daml.js`
are small and documented in that repository's README, so a conflict in
`src/scanner.c` or `grammar/module.js` is worth reading carefully rather than
resolving mechanically.
