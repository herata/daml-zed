# Daml support for Zed — フェーズ1 実装プラン

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Zed で `.daml` を開くと Daml 固有構文が正しくハイライトされ、`dpm damlc multi-ide` による補完・型ホバー・診断・定義ジャンプが動作する状態を作り、Zed 公式レジストリに公開する。

**Architecture:** 2リポジトリ。`tree-sitter-daml` は `tree-sitter/tree-sitter-haskell` (MIT) の GitHub fork で、Daml 固有規則を `grammar/daml.js` に隔離して上流 merge を安く保つ。`daml-zed` は monorepo で、`editors/zed/` に Zed 拡張（wasm32-wasip1）、`crates/daml-ide-bridge/` はフェーズ2用に空のまま。拡張は「正しい引数で LSP を起動する」だけの薄い層に保つ。

**Tech Stack:** tree-sitter (JS DSL + C scanner), Rust (`zed_extension_api` 0.7), Daml SDK 3.4+ / `dpm`, GitHub Actions

**設計 spec:** `docs/superpowers/specs/2026-09-02-daml-zed-design.md`

---

## ディレクトリ構成（完成形）

```
~/dev/tree-sitter-daml/          ← 別リポジトリ (fork)
├── grammar/daml.js              ← 新規。Daml 固有規則をすべてここに隔離
├── grammar/module.js            ← 変更。declaration の choice に2行追加
├── grammar.js                   ← 変更。daml.js の require と展開、name を 'daml' に
├── test/corpus/daml/            ← 新規。Daml 固有構文のコーパステスト
├── script/parse-real-world.sh   ← 新規。SDK の .daml を全パースする追従テスト
└── .github/workflows/ci.yml     ← 変更。上記2つを回す

~/dev/daml-zed/                  ← 本リポジトリ
├── editors/zed/
│   ├── extension.toml
│   ├── Cargo.toml
│   ├── src/lib.rs               ← Zed API との接続のみ。ロジックを持たない
│   ├── src/server.rs            ← 純ロジック（引数組み立て・dpm 探索）。cargo test 対象
│   └── languages/daml/
│       ├── config.toml
│       ├── highlights.scm
│       ├── brackets.scm
│       ├── indents.scm
│       ├── outline.scm
│       ├── textobjects.scm
│       └── injections.scm
├── crates/daml-ide-bridge/      ← フェーズ2。本プランでは作らない
├── .github/workflows/ci.yml
├── README.md
├── LICENSE
└── RELEASING.md
```

`src/lib.rs` と `src/server.rs` を分けるのは、`lib.rs` が WASM 専用で自動テストできないのに対し、
`server.rs` はネイティブでテストできるため。ロジックは全部 `server.rs` に置く。

---

## Task 0: ツールチェーンの用意

**Files:** なし（環境構築のみ）

- [ ] **Step 1: 現状を確認**

```bash
rustc --version; cargo --version; rustup --version; tree-sitter --version; dpm --version
```

期待: `rustc`/`cargo` は Homebrew 版 1.89 が存在。`rustup` / `tree-sitter` / `dpm` は not found。

- [ ] **Step 2: rustup を入れる**

Zed の dev extension ビルドは rustup と `wasm32-wasip1` ターゲットを要求する。
Homebrew の rust では target を追加できないので rustup を入れる。

```bash
brew install rustup
rustup-init -y --no-modify-path --default-toolchain stable
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

- [ ] **Step 3: wasm ターゲットを追加**

```bash
"$(brew --prefix rustup)/bin/rustup" target add wasm32-wasip1
"$(brew --prefix rustup)/bin/rustup" target list --installed
```

期待: 出力に `wasm32-wasip1` が含まれる。

- [ ] **Step 4: tree-sitter CLI を入れる**

```bash
npm install -g tree-sitter-cli
tree-sitter --version
```

期待: `tree-sitter 0.2x.x` が表示される。

- [ ] **Step 5: Daml SDK (dpm) を入れる**

```bash
curl -sSL https://get.digitalasset.com/dpm | sh
export PATH="$HOME/.dpm/bin:$PATH"
dpm --version
```

期待: 3.4 以上のバージョンが表示される。インストーラ URL が変わっていた場合は
`https://docs.digitalasset.com/build/3.4/dpm/` を参照して手順を差し替える。

`dpm` が入らない場合でも Task 1〜8（文法）は続行できる。Task 9 以降の LSP 実機確認は
`dpm` 必須なので、そこで止まる。

---

## Task 1: tree-sitter-daml を fork してベースラインを確認

**Files:**
- Create: `~/dev/tree-sitter-daml/`（fork のクローン）

- [ ] **Step 1: GitHub アカウントを確認**

```bash
gh api user --jq .login
```

- [ ] **Step 2: fork を作る**

```bash
gh repo fork tree-sitter/tree-sitter-haskell --fork-name tree-sitter-daml --clone=false
```

- [ ] **Step 3: クローンして upstream を設定**

```bash
cd ~/dev
gh repo clone "$(gh api user --jq .login)/tree-sitter-daml"
cd tree-sitter-daml
git remote add upstream https://github.com/tree-sitter/tree-sitter-haskell.git
git remote -v
```

期待: `origin` が自分の fork、`upstream` が tree-sitter/tree-sitter-haskell。

- [ ] **Step 4: ベースラインのテストが通ることを確認**

```bash
cd ~/dev/tree-sitter-daml
npm install
tree-sitter generate
tree-sitter test 2>&1 | tail -20
```

期待: 全テスト PASS。ここが赤い場合は上流の問題なので、先に原因を切り分ける。

- [ ] **Step 5: コミットしない（変更なし）**

この時点で作業ツリーはクリーンなはず。`git status --short` が空であることを確認。

---

## Task 2: 文法の名前を daml にする

**Files:**
- Modify: `~/dev/tree-sitter-daml/grammar.js`
- Modify: `~/dev/tree-sitter-daml/package.json`
- Modify: `~/dev/tree-sitter-daml/tree-sitter.json`

- [ ] **Step 1: grammar.js の name を変更**

`grammar.js` の

```js
module.exports = grammar({
  name: 'haskell',
```

を

```js
module.exports = grammar({
  name: 'daml',
```

に変更する。

- [ ] **Step 2: package.json と tree-sitter.json を更新**

`package.json` の `name` を `tree-sitter-daml`、`description` を
`Daml grammar for tree-sitter (fork of tree-sitter-haskell)` に変更。

`tree-sitter.json` の `grammars[0].name` を `daml`、`grammars[0].file-types` を
`["daml"]`、`grammars[0].scope` を `source.daml` に変更。`metadata.version` を `0.1.0` に。

- [ ] **Step 3: 再生成してテストが通ることを確認**

```bash
cd ~/dev/tree-sitter-daml
tree-sitter generate
tree-sitter test 2>&1 | tail -20
```

期待: 全テスト PASS（Haskell のコーパスがそのまま通る。Daml は Haskell の上位互換なので正しい）。

- [ ] **Step 4: LICENSE に追記**

`LICENSE` の末尾に以下を追記する（MIT の原著作権表示は消さない）。

```
---

Modifications for Daml (c) 2026 <your name>
Licensed under the same MIT terms as the original work.
```

- [ ] **Step 5: コミット**

```bash
cd ~/dev/tree-sitter-daml
git add -A
git commit -m "chore: rename grammar from haskell to daml"
```

---

## Task 3: Daml のフィールドブロックと template の骨格

**Files:**
- Create: `~/dev/tree-sitter-daml/grammar/daml.js`
- Modify: `~/dev/tree-sitter-daml/grammar.js`
- Modify: `~/dev/tree-sitter-daml/grammar/module.js`
- Test: `~/dev/tree-sitter-daml/test/corpus/daml/template.txt`

- [ ] **Step 1: 失敗するテストを書く**

`test/corpus/daml/template.txt` を新規作成:

```
================================================================================
empty template
================================================================================

module M where

template T
  with
  where

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields)
      (template_body))))

================================================================================
template with fields
================================================================================

module M where

template Iou
  with
    issuer : Party
    owner : Party
  where

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields
        field: (daml_field name: (variable) type: (name))
        field: (daml_field name: (variable) type: (name)))
      (template_body))))
```

- [ ] **Step 2: テストが失敗することを確認**

```bash
cd ~/dev/tree-sitter-daml
tree-sitter test -f "empty template" 2>&1 | tail -20
```

期待: FAIL。`template` が未知の構文なのでパースエラーになる。

- [ ] **Step 3: grammar/daml.js を作る**

```js
const {
  sep1,
  layout,
} = require('./util.js')

/**
 * Daml-specific declarations.
 *
 * Everything Daml adds on top of Haskell lives in this file, so that merges from
 * upstream tree-sitter-haskell stay cheap. The only changes outside this file are
 * the `require`/spread in `grammar.js` and two entries in `module.js`'s
 * `declaration` supertype.
 */
module.exports = {

  // ------------------------------------------------------------------------
  // field blocks (`with`)
  // ------------------------------------------------------------------------

  /**
   * Daml's `with` blocks are layout-sensitive, one field per line:
   *
   * > template Iou
   * >   with
   * >     issuer : Party
   * >     owner : Party
   *
   * but may also be written inline, comma separated.
   */
  daml_fields: $ => layout($, sep1(',', field('field', $.daml_field))),

  daml_field: $ => seq(
    field('name', $.variable),
    $._colon2,
    field('type', $.quantified_type),
  ),

  _daml_with: $ => seq('with', field('fields', $.daml_fields)),

  // ------------------------------------------------------------------------
  // template
  // ------------------------------------------------------------------------

  template: $ => seq(
    'template',
    $._type_head,
    optional($._daml_with),
    optional(seq($._where, optional(field('body', $.template_body)))),
  ),

  template_body: $ => layout($, field('item', $._template_item)),

  _template_item: $ => choice(
    $.decl,
  ),

}
```

`daml_field` が `::` ではなく `:` を使う点に注意。Daml は型注釈に単一コロンを使う。
`$._colon2` は `::`/`∷` なので**そのままでは使えない**。Step 4 で直す。

- [ ] **Step 4: `:` による型注釈にする**

`daml_field` を次に置き換える。

```js
  daml_field: $ => seq(
    field('name', $.variable),
    ':',
    field('type', $.quantified_type),
  ),
```

- [ ] **Step 5: grammar.js に組み込む**

`grammar.js` の require 群に追加:

```js
  daml = require('./grammar/daml.js'),
```

`rules` の `...decl,` の直後に追加:

```js
    ...daml,
```

- [ ] **Step 6: module.js の declaration に登録**

`grammar/module.js` の `declaration: $ => choice(` の中、`$.class,` の直前に追加:

```js
    $.template,
```

- [ ] **Step 7: 生成してテストする**

```bash
cd ~/dev/tree-sitter-daml
tree-sitter generate && tree-sitter test -f "template" 2>&1 | tail -40
```

期待: PASS。conflict エラーが出た場合は `grammar/conflicts.js` に
`[$.daml_field, $.decl]` などの必要な conflict を追加する。

- [ ] **Step 8: Haskell の既存コーパスが壊れていないことを確認**

```bash
tree-sitter test 2>&1 | tail -20
```

期待: 全 PASS。

- [ ] **Step 9: コミット**

```bash
git add -A
git commit -m "feat: parse template declarations with field blocks"
```

---

## Task 4: template body の各項目

**Files:**
- Modify: `~/dev/tree-sitter-daml/grammar/daml.js`
- Test: `~/dev/tree-sitter-daml/test/corpus/daml/template_body.txt`

- [ ] **Step 1: 失敗するテストを書く**

`test/corpus/daml/template_body.txt`:

```
================================================================================
signatory observer ensure
================================================================================

module M where

template Iou
  with
    issuer : Party
    owner : Party
    amount : Decimal
  where
    signatory issuer
    observer owner
    ensure amount > 0.0

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields
        field: (daml_field name: (variable) type: (name))
        field: (daml_field name: (variable) type: (name))
        field: (daml_field name: (variable) type: (name)))
      (template_body
        item: (signatory (variable))
        item: (observer (variable))
        item: (ensure (infix
          left_operand: (variable)
          operator: (operator)
          right_operand: (float)))))))

================================================================================
contract key
================================================================================

module M where

template T
  with
    p : Party
    k : Text
  where
    signatory p
    key (p, k) : (Party, Text)
    maintainer key._1

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields
        field: (daml_field name: (variable) type: (name))
        field: (daml_field name: (variable) type: (name)))
      (template_body
        item: (signatory (variable))
        item: (key
          expression: (tuple (variable) (variable))
          type: (tuple (name) (name)))
        item: (maintainer (field_path field: (field_name) subfield: (field_name)))))))
```

- [ ] **Step 2: テストが失敗することを確認**

```bash
tree-sitter test -f "signatory observer ensure" 2>&1 | tail -20
```

期待: FAIL。

- [ ] **Step 3: template_item の規則を実装**

`grammar/daml.js` の `_template_item` を置き換え、規則を追加する。

```js
  _template_item: $ => choice(
    $.signatory,
    $.observer,
    $.ensure,
    $.agreement,
    $.key,
    $.maintainer,
    $.choice,
    $.interface_instance,
    $.decl,
  ),

  signatory: $ => seq('signatory', sep1(',', field('party', $._exp))),

  observer: $ => seq('observer', sep1(',', field('party', $._exp))),

  ensure: $ => seq('ensure', field('condition', $._exp)),

  agreement: $ => seq('agreement', field('text', $._exp)),

  key: $ => seq(
    'key',
    field('expression', $._exp),
    optional(seq(':', field('type', $.quantified_type))),
  ),

  maintainer: $ => seq('maintainer', sep1(',', field('party', $._exp))),
```

`agreement` は Daml 2.x で非推奨だが、既存コードのパースのために残す。

- [ ] **Step 4: 生成してテストする**

```bash
tree-sitter generate && tree-sitter test -f "signatory observer ensure" 2>&1 | tail -40
tree-sitter test -f "contract key" 2>&1 | tail -40
```

期待: 両方 PASS。`$.choice` と `$.interface_instance` は未定義なので generate が
失敗する。Task 5・6 で定義するまで、この2行は一旦コメントアウトしておく。

- [ ] **Step 5: 全コーパスが通ることを確認**

```bash
tree-sitter test 2>&1 | tail -20
```

- [ ] **Step 6: コミット**

```bash
git add -A
git commit -m "feat: parse signatory, observer, ensure, agreement, key, maintainer"
```

---

## Task 5: choice 宣言

**Files:**
- Modify: `~/dev/tree-sitter-daml/grammar/daml.js`
- Test: `~/dev/tree-sitter-daml/test/corpus/daml/choice.txt`

- [ ] **Step 1: 失敗するテストを書く**

`test/corpus/daml/choice.txt`:

```
================================================================================
consuming choice with arguments
================================================================================

module M where

template T
  with
    owner : Party
  where
    signatory owner
    choice Transfer : ContractId T
      with
        newOwner : Party
      controller owner
      do
        create this with owner = newOwner

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields
        field: (daml_field name: (variable) type: (name)))
      (template_body
        item: (signatory (variable))
        item: (choice
          name: (constructor)
          return_type: (apply function: (name) argument: (name))
          (daml_fields
            field: (daml_field name: (variable) type: (name)))
          (controller (variable))
          body: (do
            (bind
              (apply
                function: (variable)
                argument: (record
                  (variable)
                  field: (field_update
                    field: (field_name)
                    (variable))))))))))))

================================================================================
nonconsuming choice with observer before controller
================================================================================

module M where

template T
  with
    owner : Party
  where
    signatory owner
    nonconsuming choice Peek : Int
      observer owner
      controller owner
      do
        pure 1

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields
        field: (daml_field name: (variable) type: (name)))
      (template_body
        item: (signatory (variable))
        item: (choice
          (choice_modifier)
          name: (constructor)
          return_type: (name)
          (observer (variable))
          (controller (variable))
          body: (do
            (bind (apply function: (variable) argument: (integer)))))))))
```

- [ ] **Step 2: テストが失敗することを確認**

```bash
tree-sitter test -f "consuming choice" 2>&1 | tail -20
```

期待: FAIL。

- [ ] **Step 3: choice を実装**

`grammar/daml.js` に追加し、Task 4 でコメントアウトした `$.choice` を復活させる。

```js
  choice_modifier: _ => choice('nonconsuming', 'preconsuming', 'postconsuming'),

  controller: $ => seq('controller', sep1(',', field('party', $._exp))),

  /**
   * The `observer` clause may appear before or after `controller`.
   */
  choice: $ => seq(
    optional(field('modifier', $.choice_modifier)),
    'choice',
    field('name', $._con),
    ':',
    field('return_type', $.quantified_type),
    optional($._daml_with),
    optional($.observer),
    $.controller,
    optional($.observer),
    field('body', alias($._exp_do, $.do)),
  ),
```

- [ ] **Step 4: 生成してテストする**

```bash
tree-sitter generate && tree-sitter test -f "choice" 2>&1 | tail -40
```

期待: PASS。

- [ ] **Step 5: 全コーパスが通ることを確認**

```bash
tree-sitter test 2>&1 | tail -20
```

- [ ] **Step 6: コミット**

```bash
git add -A
git commit -m "feat: parse choice declarations with modifiers and observer clauses"
```

---

## Task 6: interface / viewtype / interface instance

**Files:**
- Modify: `~/dev/tree-sitter-daml/grammar/daml.js`
- Modify: `~/dev/tree-sitter-daml/grammar/module.js`
- Test: `~/dev/tree-sitter-daml/test/corpus/daml/interface.txt`

- [ ] **Step 1: 失敗するテストを書く**

`test/corpus/daml/interface.txt`:

```
================================================================================
interface with viewtype
================================================================================

module M where

interface Token where
  viewtype TokenView
  getOwner : Party
  nonconsuming choice GetAmount : Decimal
    controller getOwner this
    do
      pure 1.0

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (interface
      name: (name)
      (interface_body
        item: (viewtype type: (name))
        item: (signature name: (variable) type: (name))
        item: (choice
          (choice_modifier)
          name: (constructor)
          return_type: (name)
          (controller (apply function: (variable) argument: (variable)))
          body: (do
            (bind (apply function: (variable) argument: (float)))))))))

================================================================================
interface instance in template
================================================================================

module M where

template T
  with
    owner : Party
  where
    signatory owner
    interface instance Token for T where
      view = TokenView owner

--------------------------------------------------------------------------------

(daml
  (header (module))
  (declarations
    (template
      name: (name)
      (daml_fields
        field: (daml_field name: (variable) type: (name)))
      (template_body
        item: (signatory (variable))
        item: (interface_instance
          interface: (name)
          template: (name)
          (interface_instance_body
            item: (bind
              name: (variable)
              (match
                expression: (apply function: (constructor) argument: (variable)))))))))))
```

- [ ] **Step 2: テストが失敗することを確認**

```bash
tree-sitter test -f "interface with viewtype" 2>&1 | tail -20
```

期待: FAIL。

- [ ] **Step 3: interface を実装**

`grammar/daml.js` に追加し、Task 4 でコメントアウトした `$.interface_instance` を復活させる。

```js
  // ------------------------------------------------------------------------
  // interface
  // ------------------------------------------------------------------------

  interface: $ => seq(
    'interface',
    $._type_head,
    optional(seq($._where, optional(field('body', $.interface_body)))),
  ),

  interface_body: $ => layout($, field('item', $._interface_item)),

  _interface_item: $ => choice(
    $.viewtype,
    $.choice,
    $.decl,
  ),

  viewtype: $ => seq('viewtype', field('type', $.quantified_type)),

  interface_instance: $ => seq(
    'interface',
    'instance',
    field('interface', $.quantified_type),
    'for',
    field('template', $.quantified_type),
    optional(seq($._where, optional(field('body', $.interface_instance_body)))),
  ),

  interface_instance_body: $ => layout($, field('item', $.decl)),
```

- [ ] **Step 4: module.js の declaration に interface を登録**

`grammar/module.js` の `declaration` の choice、`$.template,` の直後に追加:

```js
    $.interface,
```

- [ ] **Step 5: 生成してテストする**

```bash
tree-sitter generate && tree-sitter test -f "interface" 2>&1 | tail -40
```

期待: PASS。`interface` と `interface instance` の先読み衝突が出た場合は
`grammar/conflicts.js` に `[$.interface, $.interface_instance]` を追加する。

- [ ] **Step 6: 全コーパスが通ることを確認**

```bash
tree-sitter test 2>&1 | tail -20
```

- [ ] **Step 7: コミット**

```bash
git add -A
git commit -m "feat: parse interface, viewtype and interface instance"
```

---

## Task 7: 実世界コーパスによる追従テスト

**Files:**
- Create: `~/dev/tree-sitter-daml/script/parse-real-world.sh`

これが「Daml のバージョンに追従できていない」を構造的に防ぐ主要な仕掛け。

- [ ] **Step 1: スクリプトを書く**

`script/parse-real-world.sh`:

```bash
#!/usr/bin/env bash
# Parse every .daml file shipped by the Daml SDK and daml-finance, and fail if
# any of them produces an ERROR or MISSING node. This is the regression guard
# that tells us the grammar has fallen behind a new Daml release.
set -euo pipefail

WORK="${TMPDIR:-/tmp}/tree-sitter-daml-corpus"
mkdir -p "$WORK"

clone() {
  local url="$1" dir="$2" ref="$3"
  if [ ! -d "$WORK/$dir" ]; then
    git clone --depth 1 --branch "$ref" "$url" "$WORK/$dir"
  fi
}

clone https://github.com/digital-asset/daml.git daml "${DAML_REF:-main}"
clone https://github.com/digital-asset/daml-finance.git daml-finance "${DAML_FINANCE_REF:-main}"

mapfile -t files < <(find "$WORK" -name '*.daml' -not -path '*/.git/*' | sort)
echo "parsing ${#files[@]} .daml files"

# tree-sitter parse exits non-zero when any file fails to parse cleanly.
if tree-sitter parse --quiet --stat "${files[@]}"; then
  echo "OK: all files parsed without errors"
else
  echo "FAIL: some files did not parse cleanly" >&2
  echo "re-run without --quiet to see the failures:" >&2
  echo "  tree-sitter parse \$(find $WORK -name '*.daml' -not -path '*/.git/*')" >&2
  exit 1
fi
```

```bash
chmod +x script/parse-real-world.sh
```

- [ ] **Step 2: 実行して現状の失敗数を把握**

```bash
cd ~/dev/tree-sitter-daml
./script/parse-real-world.sh 2>&1 | tail -30
```

期待: 最初は失敗する。`--stat` が出す成功／失敗件数を記録する。

- [ ] **Step 3: 失敗ファイルを1つずつ潰す**

失敗の多い順に構文を特定し、`grammar/daml.js` に規則を足す。
規則を足すたびに `test/corpus/daml/` に最小再現のコーパステストを追加してから直すこと
（テストなしで直すと再発を検出できない）。

```bash
tree-sitter parse <失敗したファイル> 2>&1 | grep -n "ERROR\|MISSING" | head
```

- [ ] **Step 4: 成功率 100% を確認**

```bash
./script/parse-real-world.sh 2>&1 | tail -5
```

期待: `OK: all files parsed without errors`

- [ ] **Step 5: コミット**

```bash
git add -A
git commit -m "test: parse the Daml SDK and daml-finance corpora"
```

---

## Task 8: 文法リポジトリの CI

**Files:**
- Modify: `~/dev/tree-sitter-daml/.github/workflows/ci.yml`

- [ ] **Step 1: 既存の CI を確認**

```bash
cd ~/dev/tree-sitter-daml
cat .github/workflows/ci.yml
```

上流の CI は複数言語のバインディングをビルドする。Daml fork では
「generate + test + 実世界コーパス」に絞る。

- [ ] **Step 2: ci.yml を差し替える**

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main, master]
  pull_request:
  schedule:
    # Weekly, so a new Daml release that breaks the grammar shows up on its own.
    - cron: "0 3 * * 1"

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: npm install -g tree-sitter-cli
      - run: tree-sitter generate
      - name: Fail if the generated parser is out of date
        run: git diff --exit-code -- src/
      - run: tree-sitter test

  real-world:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: npm install -g tree-sitter-cli
      - run: tree-sitter generate
      - run: ./script/parse-real-world.sh
```

`git diff --exit-code -- src/` は、`src/parser.c` をコミットし忘れた状態を検出する。
tree-sitter では生成物をコミットするのが慣習で、Zed もコミット済みの `src/` を使う。

- [ ] **Step 3: 生成物がコミットされていることを確認**

```bash
tree-sitter generate
git status --short src/
```

期待: 空（生成物が既にコミット済み）。差分がある場合はコミットする。

- [ ] **Step 4: push して CI が緑になることを確認**

```bash
git add -A
git commit -m "ci: run generate, corpus tests and real-world parsing"
git push origin HEAD
gh run watch
```

期待: 両ジョブが success。

---

## Task 9: Zed 拡張の骨格

**Files:**
- Create: `~/dev/daml-zed/editors/zed/extension.toml`
- Create: `~/dev/daml-zed/editors/zed/Cargo.toml`
- Create: `~/dev/daml-zed/editors/zed/src/lib.rs`
- Create: `~/dev/daml-zed/editors/zed/languages/daml/config.toml`

- [ ] **Step 1: extension.toml を作る**

`<owner>` は Task 1 Step 1 で確認した GitHub のログイン名、`<sha>` は
`cd ~/dev/tree-sitter-daml && git rev-parse HEAD` の値に置き換える。

```toml
id = "daml"
name = "Daml"
description = "Daml smart contract language support"
version = "0.1.0"
schema_version = 1
authors = ["<your name> <your email>"]
repository = "https://github.com/<owner>/daml-zed"

[grammars.daml]
repository = "https://github.com/<owner>/tree-sitter-daml"
commit = "<sha>"

[language_servers.daml-language-server]
name = "Daml Language Server"
languages = ["Daml"]
language_ids = { "Daml" = "daml" }
```

- [ ] **Step 2: Cargo.toml を作る**

```toml
[package]
name = "zed_daml"
version = "0.1.0"
edition = "2021"
publish = false
license = "Apache-2.0"

[lib]
path = "src/lib.rs"
crate-type = ["cdylib", "rlib"]

[dependencies]
zed_extension_api = "0.7"
```

`crate-type` に `rlib` を含めるのは、Task 12 で `cargo test` をネイティブで走らせるため。

- [ ] **Step 3: config.toml を作る**

`languages/daml/config.toml`:

```toml
name = "Daml"
grammar = "daml"
path_suffixes = ["daml"]
line_comments = ["-- "]
block_comment = ["{- ", " -}"]
tab_size = 2
hard_tabs = false
word_characters = ["'"]
autoclose_before = ",;:.=}])> \n\t"

[[brackets]]
start = "("
end = ")"
close = true
newline = false

[[brackets]]
start = "["
end = "]"
close = true
newline = false

[[brackets]]
start = "{"
end = "}"
close = true
newline = true

[[brackets]]
start = "\""
end = "\""
close = true
newline = false
not_in = ["string", "comment"]
```

- [ ] **Step 4: 最小の lib.rs を作る**

```rust
use zed_extension_api as zed;

struct DamlExtension;

impl zed::Extension for DamlExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        _worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        Err("not implemented yet".into())
    }
}

zed::register_extension!(DamlExtension);
```

- [ ] **Step 5: WASM ビルドが通ることを確認**

```bash
cd ~/dev/daml-zed/editors/zed
cargo build --target wasm32-wasip1
```

期待: `Finished` で終わる。

- [ ] **Step 6: コミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "feat: scaffold the Zed extension"
```

---

## Task 10: LSP 起動ロジック（純関数 + テスト）

**Files:**
- Create: `~/dev/daml-zed/editors/zed/src/server.rs`
- Modify: `~/dev/daml-zed/editors/zed/src/lib.rs`

- [ ] **Step 1: 失敗するテストを書く**

`src/server.rs` を新規作成し、テストだけ先に書く。

```rust
//! Pure logic for locating and launching the Daml language server.
//!
//! This module deliberately contains no `zed_extension_api` calls, so it can be
//! unit tested on a native target. `lib.rs` is the only place that talks to Zed.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_default_arguments() {
        let settings = ServerSettings::default();
        assert_eq!(
            build_args(&settings),
            vec![
                "damlc".to_string(),
                "multi-ide".to_string(),
                "--telemetry-ignored".to_string(),
                "--log-level=Warning".to_string(),
            ]
        );
    }

    #[test]
    fn honours_log_level() {
        let settings = ServerSettings {
            log_level: LogLevel::Debug,
            ..Default::default()
        };
        assert!(build_args(&settings).contains(&"--log-level=Debug".to_string()));
    }

    #[test]
    fn appends_extra_arguments() {
        let settings = ServerSettings {
            extra_arguments: vec!["--ghc-option".into(), "-W".into()],
            ..Default::default()
        };
        let args = build_args(&settings);
        assert_eq!(&args[args.len() - 2..], &["--ghc-option", "-W"]);
    }

    #[test]
    fn never_enables_telemetry() {
        // Zed extensions cannot show a consent dialog, so telemetry is always off.
        let args = build_args(&ServerSettings::default());
        assert!(args.contains(&"--telemetry-ignored".to_string()));
        assert!(!args.contains(&"--telemetry".to_string()));
    }

    #[test]
    fn missing_dpm_produces_an_actionable_error() {
        let err = resolve_command(None, &ServerSettings::default()).unwrap_err();
        assert!(err.contains("dpm"));
        assert!(err.contains("https://docs.digitalasset.com"));
    }

    #[test]
    fn uses_dpm_when_found() {
        let cmd = resolve_command(Some("/usr/local/bin/dpm".into()), &ServerSettings::default())
            .unwrap();
        assert_eq!(cmd.program, "/usr/local/bin/dpm");
        assert_eq!(cmd.args[0], "damlc");
    }
}
```

- [ ] **Step 2: テストが失敗することを確認**

```bash
cd ~/dev/daml-zed/editors/zed
cargo test 2>&1 | tail -20
```

期待: コンパイルエラー。`ServerSettings` / `LogLevel` / `build_args` / `resolve_command` が未定義。

- [ ] **Step 3: 実装を書く**

`src/server.rs` のテストモジュールの上に追記する。

```rust
use serde::Deserialize;

/// The log levels `damlc multi-ide` accepts. `Telemetry` is deliberately not
/// offered: see `never_enables_telemetry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        // Same default as the official VS Code extension.
        LogLevel::Warning
    }
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Warning => "Warning",
            LogLevel::Error => "Error",
        }
    }
}

/// User settings read from `lsp."daml-language-server".settings`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    pub log_level: LogLevel,
    pub extra_arguments: Vec<String>,
}

/// A resolved command line, independent of Zed's own `Command` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub program: String,
    pub args: Vec<String>,
}

pub const INSTALL_HINT: &str =
    "Daml SDK not found: no `dpm` on PATH. Install it and reopen the project. \
     See https://docs.digitalasset.com/build/3.4/dpm/ — or set \
     lsp.\"daml-language-server\".binary.path in your Zed settings.";

/// `multi-ide` is always used: it can only be disabled with the legacy Daml
/// Assistant, and this extension targets Daml 3.4+ / dpm.
pub fn build_args(settings: &ServerSettings) -> Vec<String> {
    let mut args = vec![
        "damlc".to_string(),
        "multi-ide".to_string(),
        "--telemetry-ignored".to_string(),
        format!("--log-level={}", settings.log_level.as_str()),
    ];
    args.extend(settings.extra_arguments.iter().cloned());
    args
}

/// `dpm_path` is whatever `worktree.which("dpm")` returned.
pub fn resolve_command(
    dpm_path: Option<String>,
    settings: &ServerSettings,
) -> Result<ResolvedCommand, String> {
    let program = dpm_path.ok_or_else(|| INSTALL_HINT.to_string())?;
    Ok(ResolvedCommand {
        program,
        args: build_args(settings),
    })
}
```

`Cargo.toml` の `[dependencies]` に `serde = { version = "1", features = ["derive"] }` を追加する。

- [ ] **Step 4: テストが通ることを確認**

```bash
cargo test 2>&1 | tail -20
```

期待: 6 tests passed。

- [ ] **Step 5: lib.rs から呼ぶ**

`src/lib.rs` を置き換える。

```rust
mod server;

use zed_extension_api::{self as zed, settings::LspSettings};

use crate::server::{resolve_command, ServerSettings};

struct DamlExtension;

impl zed::Extension for DamlExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let lsp_settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;

        // An explicit binary in the user's settings always wins. This is the
        // escape hatch for legacy `daml` assistants and for SDKs older than 3.4.
        if let Some(binary) = lsp_settings.binary {
            if let Some(path) = binary.path {
                return Ok(zed::Command {
                    command: path,
                    args: binary.arguments.unwrap_or_default(),
                    env: worktree.shell_env(),
                });
            }
        }

        let settings: ServerSettings = lsp_settings
            .settings
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();

        let resolved = resolve_command(worktree.which("dpm"), &settings)?;

        Ok(zed::Command {
            command: resolved.program,
            args: resolved.args,
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(DamlExtension);
```

`Cargo.toml` の `[dependencies]` に `serde_json = "1"` を追加する。

- [ ] **Step 6: 両方のターゲットでビルドを確認**

```bash
cargo test 2>&1 | tail -5
cargo build --target wasm32-wasip1 2>&1 | tail -5
```

期待: 両方成功。

- [ ] **Step 7: コミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "feat: launch dpm damlc multi-ide as the language server"
```

---

## Task 11: 補完・シンボルのラベル整形

**Files:**
- Modify: `~/dev/daml-zed/editors/zed/src/lib.rs`

- [ ] **Step 1: label_for_completion を実装**

`impl zed::Extension for DamlExtension` の中に追加する。

```rust
    fn label_for_completion(
        &self,
        _language_server_id: &zed::LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<zed::CodeLabel> {
        let detail = completion.detail.as_deref()?.trim();
        if detail.is_empty() {
            return None;
        }
        // damlc sometimes includes the `::` itself; normalise it away so we
        // always render exactly one separator.
        let detail = detail.strip_prefix("::").unwrap_or(detail).trim();

        let separator = " :: ";
        let label = &completion.label;
        let code = format!("{label}{separator}{detail}");
        let detail_start = label.len() + separator.len();

        Some(zed::CodeLabel {
            spans: vec![
                zed::CodeLabelSpan::code_range(0..label.len()),
                zed::CodeLabelSpan::literal(separator, Some("operator".to_string())),
                zed::CodeLabelSpan::code_range(detail_start..code.len()),
            ],
            filter_range: (0..label.len()).into(),
            code,
        })
    }
```

- [ ] **Step 2: label_for_symbol を実装**

```rust
    fn label_for_symbol(
        &self,
        _language_server_id: &zed::LanguageServerId,
        symbol: zed::lsp::Symbol,
    ) -> Option<zed::CodeLabel> {
        use zed::lsp::SymbolKind;

        let name = &symbol.name;
        let (code, display_range, filter_range) = match symbol.kind {
            SymbolKind::Struct => {
                let prefix = "data ";
                let code = format!("{prefix}{name} = A");
                (code, 0..prefix.len() + name.len(), prefix.len()..prefix.len() + name.len())
            }
            SymbolKind::Constructor => {
                let prefix = "data A = ";
                let code = format!("{prefix}{name}");
                (code, prefix.len()..prefix.len() + name.len(), 0..name.len())
            }
            SymbolKind::Variable => {
                let code = format!("{name} :: T");
                (code, 0..name.len(), 0..name.len())
            }
            _ => return None,
        };

        Some(zed::CodeLabel {
            spans: vec![zed::CodeLabelSpan::code_range(display_range)],
            filter_range: filter_range.into(),
            code,
        })
    }
```

- [ ] **Step 3: ビルドを確認**

```bash
cd ~/dev/daml-zed/editors/zed
cargo build --target wasm32-wasip1 2>&1 | tail -5
```

- [ ] **Step 4: コミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "feat: render completions and symbols as Haskell type signatures"
```

---

## Task 12: tree-sitter クエリ

**Files:**
- Create: `~/dev/daml-zed/editors/zed/languages/daml/highlights.scm`
- Create: `~/dev/daml-zed/editors/zed/languages/daml/outline.scm`
- Create: `~/dev/daml-zed/editors/zed/languages/daml/brackets.scm`
- Create: `~/dev/daml-zed/editors/zed/languages/daml/indents.scm`
- Create: `~/dev/daml-zed/editors/zed/languages/daml/textobjects.scm`
- Create: `~/dev/daml-zed/editors/zed/languages/daml/injections.scm`

- [ ] **Step 1: 上流の Haskell 用クエリを出発点にする**

```bash
cp ~/dev/tree-sitter-daml/queries/highlights.scm \
   ~/dev/daml-zed/editors/zed/languages/daml/highlights.scm
ls ~/dev/tree-sitter-daml/queries/
```

上流に無いファイルは Zed 本体の Haskell 拡張（`zed-industries/zed` の
`extensions/haskell/languages/haskell/`）を参照して作る。

- [ ] **Step 2: Daml 固有のハイライトを追記**

`highlights.scm` の末尾に追記する。

```scheme
; Daml-specific declarations
[
  "template"
  "interface"
  "instance"
  "with"
  "signatory"
  "observer"
  "controller"
  "ensure"
  "agreement"
  "key"
  "maintainer"
  "choice"
  "viewtype"
  "for"
] @keyword

(choice_modifier) @keyword

(template name: (name) @type)
(interface name: (name) @type)
(choice name: (constructor) @constructor)
(daml_field name: (variable) @property)
(interface_instance interface: (quantified_type) @type)
(interface_instance template: (quantified_type) @type)
```

- [ ] **Step 3: outline.scm を書く**

テンプレートと choice がアウトラインに出ることが Daml 開発で最も効く。

```scheme
(template
  "template" @context
  name: (name) @name) @item

(interface
  "interface" @context
  name: (name) @name) @item

(choice
  (choice_modifier)? @context
  "choice" @context
  name: (constructor) @name) @item

(interface_instance
  "interface" @context
  "instance" @context
  interface: (quantified_type) @name) @item

(data_type
  "data" @context
  name: (name) @name) @item

(signature
  name: (variable) @name) @item

(function
  name: (variable) @name) @item
```

- [ ] **Step 4: 残りのクエリを用意**

`brackets.scm`:

```scheme
("(" @open ")" @close)
("[" @open "]" @close)
("{" @open "}" @close)
("\"" @open "\"" @close)
```

`indents.scm`:

```scheme
[
  (template_body)
  (interface_body)
  (interface_instance_body)
  (daml_fields)
  (local_binds)
  (class_declarations)
  (instance_declarations)
] @indent
```

`textobjects.scm`:

```scheme
(template) @class.around
(template body: (template_body) @class.inside)

(interface) @class.around
(interface body: (interface_body) @class.inside)

(choice) @function.around
(choice body: (do) @function.inside)

(function) @function.around
(comment) @comment.around
```

`injections.scm`:

```scheme
((comment) @injection.content
 (#set! injection.language "comment"))
```

- [ ] **Step 5: クエリが文法に対して有効であることを検証**

Zed は不正なクエリを黙って無視することがあるので、tree-sitter CLI で検証する。

```bash
cd ~/dev/tree-sitter-daml
cat > /tmp/sample.daml <<'EOF'
module Sample where

template Iou
  with
    issuer : Party
    owner : Party
    amount : Decimal
  where
    signatory issuer
    observer owner
    ensure amount > 0.0

    choice Transfer : ContractId Iou
      with
        newOwner : Party
      controller owner
      do
        create this with owner = newOwner
EOF

for q in highlights outline brackets indents textobjects injections; do
  echo "== $q"
  tree-sitter query "$HOME/dev/daml-zed/editors/zed/languages/daml/$q.scm" /tmp/sample.daml \
    | head -5
done
```

期待: いずれもエラーなく、キャプチャが出力される。
`Query error` が出たら、そのノード名は文法に存在しないので直す。

- [ ] **Step 6: コミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "feat: add tree-sitter queries for Daml"
```

---

## Task 13: 拡張リポジトリの CI

**Files:**
- Create: `~/dev/daml-zed/.github/workflows/ci.yml`

- [ ] **Step 1: ワークフローを書く**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  extension:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: editors/zed
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-wasip1
          components: rustfmt, clippy
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
      - run: cargo build --target wasm32-wasip1

  queries:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: npm install -g tree-sitter-cli
      - name: Check out the pinned grammar revision
        run: |
          commit=$(grep -A2 '^\[grammars.daml\]' editors/zed/extension.toml \
            | grep '^commit' | cut -d'"' -f2)
          git clone https://github.com/${{ github.repository_owner }}/tree-sitter-daml grammar
          git -C grammar checkout "$commit"
      - run: tree-sitter generate
        working-directory: grammar
      - name: Validate every query against the grammar
        working-directory: grammar
        run: |
          cat > /tmp/sample.daml <<'EOF'
          module Sample where

          template Iou
            with
              issuer : Party
              owner : Party
            where
              signatory issuer
              observer owner

              choice Transfer : ContractId Iou
                with
                  newOwner : Party
                controller owner
                do
                  create this with owner = newOwner
          EOF
          for q in highlights outline brackets indents textobjects injections; do
            echo "== $q"
            tree-sitter query "$GITHUB_WORKSPACE/editors/zed/languages/daml/$q.scm" \
              /tmp/sample.daml > /dev/null
          done
```

`queries` ジョブは、文法の変更でクエリが壊れたことを検出する。
拡張の `commit` ピン留めと実際の文法がずれた状態を防ぐのが目的。

- [ ] **Step 2: ローカルで同じことを実行して通ることを確認**

```bash
cd ~/dev/daml-zed/editors/zed
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

期待: すべて成功。`cargo fmt --check` が落ちたら `cargo fmt` を実行してから再確認。

- [ ] **Step 3: コミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "ci: build, lint and test the extension and validate queries"
```

---

## Task 14: ドキュメント

**Files:**
- Create: `~/dev/daml-zed/README.md`
- Create: `~/dev/daml-zed/LICENSE`
- Create: `~/dev/daml-zed/RELEASING.md`

- [ ] **Step 1: LICENSE を置く**

Apache-2.0 の全文を置く。

```bash
cd ~/dev/daml-zed
curl -sSL https://www.apache.org/licenses/LICENSE-2.0.txt -o LICENSE
```

- [ ] **Step 2: README.md を書く**

以下の内容を含める。

- 何ができるか（ハイライト、補完、型ホバー、診断、定義ジャンプ、multi-package 横断ジャンプ）
- 何ができないか（Script results はフェーズ2、Zed は Webview を持たないため）
- 前提: Daml SDK 3.4+ と `dpm` が PATH にあること。インストール手順へのリンク
- 設定例:

````markdown
```json
{
  "lsp": {
    "daml-language-server": {
      "settings": {
        "log_level": "Warning",
        "extra_arguments": ["--ghc-option", "-W"]
      }
    }
  }
}
```

`dpm` が PATH に無い場合や、古い SDK で `damlc ide` を使いたい場合は
バイナリを直接指定します。

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
````

- テレメトリを一切送らないこと（常に `--telemetry-ignored`）とその理由
- 開発手順（dev extension としての install、Task 15 の手動確認チェックリスト）

- [ ] **Step 3: RELEASING.md を書く**

```markdown
# リリース手順

1. `tree-sitter-daml` で変更をマージし、タグを切る

   ```bash
   cd ~/dev/tree-sitter-daml
   git tag v0.x.y && git push --tags
   git rev-parse HEAD
   ```

2. `editors/zed/extension.toml` の `[grammars.daml] commit` を 1 の SHA に更新する
3. `editors/zed/extension.toml` の `version` を上げる
4. `daml-zed` をコミットしてタグを切る

   ```bash
   git commit -am "release: v0.x.y" && git tag v0.x.y && git push --tags
   ```

5. `zed-industries/extensions` に PR を出す

   `extensions.toml` に以下を追加（初回のみ）、以降は `version` のみ更新する。

   ```toml
   [daml]
   submodule = "extensions/daml"
   path = "editors/zed"
   version = "0.x.y"
   ```

## 上流の tree-sitter-haskell を取り込む

```bash
cd ~/dev/tree-sitter-daml
git fetch upstream
git merge upstream/master
tree-sitter generate && tree-sitter test && ./script/parse-real-world.sh
```
```

- [ ] **Step 4: コミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "docs: add README, LICENSE and release instructions"
```

---

## Task 15: Zed 実機での手動確認

**Files:** なし（検証のみ）

WASM 部分は自動テストできないため、ここで実際に動かして確認する。

- [ ] **Step 1: テスト用の Daml プロジェクトを作る**

```bash
cd /tmp && rm -rf daml-zed-smoke
dpm new daml-zed-smoke --template skeleton
cd daml-zed-smoke && dpm build
```

期待: `.dar` が生成される。ここが失敗する場合は拡張以前に SDK の問題。

- [ ] **Step 2: dev extension として install**

Zed で `cmd-shift-p` → `zed: install dev extension` → `~/dev/daml-zed/editors/zed`
を選択する。

期待: ビルドが成功し、拡張が Installed に出る。

- [ ] **Step 3: 確認項目を1つずつ潰す**

`/tmp/daml-zed-smoke` を Zed で開き、`daml/Main.daml` に対して確認する。

- [ ] ファイルが `Daml` 言語として認識される（右下の言語表示）
- [ ] `template` / `with` / `where` / `signatory` / `choice` / `controller` が
      キーワード色になる
- [ ] テンプレート名と choice 名が識別子とは別の色になる
- [ ] `cmd-shift-o` のアウトラインにテンプレートと choice が出る
- [ ] わざと型エラーを入れると診断が出る（例: `signatory 1`）
- [ ] 型エラーを直すと診断が消える
- [ ] 識別子の上で型のホバーが出る
- [ ] `create` などの標準ライブラリ関数で定義ジャンプができる
- [ ] 補完が `name :: Type` の形で出る
- [ ] `--` でコメントアウト、`cmd-/` が `-- ` を使う

- [ ] **Step 4: LSP のログを確認**

`cmd-shift-p` → `debug: open language server logs` で
`Daml Language Server` を選び、起動コマンドが
`dpm damlc multi-ide --telemetry-ignored --log-level=Warning` になっていることを確認する。

- [ ] **Step 5: multi-package を確認**

```bash
cd /tmp && rm -rf daml-zed-multi && mkdir daml-zed-multi && cd daml-zed-multi
dpm new pkg-a --template skeleton
dpm new pkg-b --template skeleton
cat > multi-package.yaml <<'EOF'
packages:
  - ./pkg-a
  - ./pkg-b
EOF
```

`pkg-b` から `pkg-a` の型を import し、Zed でその型に対して定義ジャンプが
`pkg-a` のソースに飛ぶことを確認する。

- [ ] **Step 6: 見つかった不具合を直す**

不具合ごとに、まず再現するテスト（文法ならコーパステスト、ロジックなら
`cargo test`）を足してから直す。直したら Step 3 をやり直す。

- [ ] **Step 7: 確認結果を README に反映してコミット**

```bash
cd ~/dev/daml-zed
git add -A
git commit -m "docs: record the manual verification checklist"
```

---

## Task 16: 公式レジストリへの公開

**Files:** `zed-industries/extensions` への PR

- [ ] **Step 1: daml-zed を GitHub に push**

```bash
cd ~/dev/daml-zed
gh repo create daml-zed --public --source=. --remote=origin --push
```

- [ ] **Step 2: CI が緑であることを確認**

```bash
gh run watch
```

- [ ] **Step 3: extensions リポジトリを fork してサブモジュールを追加**

サブモジュールは HTTPS URL でなければならない（`git@github.com:` は不可）。

```bash
cd /tmp
gh repo fork zed-industries/extensions --clone
cd extensions
git submodule add https://github.com/<owner>/daml-zed.git extensions/daml
git add extensions/daml
```

- [ ] **Step 4: extensions.toml に登録**

`extensions.toml` に以下を追加する。`version` は
`editors/zed/extension.toml` の `version` と一致していなければならない。

```toml
[daml]
submodule = "extensions/daml"
path = "editors/zed"
version = "0.1.0"
```

- [ ] **Step 5: 並び順を整える**

`extensions.toml` と `.gitmodules` のソートは PR 前に必須。

```bash
pnpm install
pnpm sort-extensions
git diff --stat
```

期待: `extensions.toml` と `.gitmodules` が正しい並び順になる。
サブモジュールが detached HEAD ではなくブランチ上のコミットを指していることも確認する。

```bash
git -C extensions/daml branch --contains HEAD
```

- [ ] **Step 6: PR を出す**

```bash
git add -A
git commit -m "Add Daml extension"
git push origin HEAD
gh pr create --title "Add Daml extension" --body "Adds Daml smart contract language support, backed by \`dpm damlc multi-ide\` and a Daml-specific tree-sitter grammar forked from tree-sitter-haskell."
```

- [ ] **Step 7: レビュー指摘に対応する**

指摘があれば直して push する。マージされたら公式レジストリからインストールできる。

---

## 完了条件

- [ ] Zed で `.daml` を開くと `template` / `choice` / `signatory` / `interface instance` が正しくハイライトされる
- [ ] アウトラインにテンプレートと choice が出る
- [ ] 補完・型ホバー・診断・定義ジャンプが動く
- [ ] multi-package プロジェクトで横断ジャンプが動く
- [ ] `tree-sitter-daml` の CI（コーパステスト + 実世界パース）が緑
- [ ] `daml-zed` の CI（fmt / clippy / test / wasm build / query 検証）が緑
- [ ] `zed-industries/extensions` に PR がマージされている
