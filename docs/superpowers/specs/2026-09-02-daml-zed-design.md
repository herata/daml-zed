# Daml support for Zed — 設計

- 日付: 2026-09-02
- ステータス: 承認済み（フェーズ1の実装プランへ）
- リポジトリ: `daml-zed`（旧称 `zed-daml-ls`）

## 1. 背景と目的

Zed には Daml のサポートが存在しない。公式レジストリ (`zed.dev/extensions`) に Daml
拡張は 0 件で、GitHub 上の個人リポジトリが2つあるのみ。

| リポジトリ | 最終更新 | 状態 |
|---|---|---|
| [Papsanly/daml-zed-extension](https://github.com/Papsanly/daml-zed-extension) | 2026-06 | 未公開・star 0・tree-sitter-haskell 流用・Apache-2.0 |
| [DLC-link/zed-daml-lsp](https://github.com/DLC-link/zed-daml-lsp) | 2026-03 | 未公開・star 0・tree-sitter-haskell 流用・Apache-2.0 |

どちらも Daml 固有構文 (`template` / `choice` / `interface`) をハイライトできない。
また VSCode 専用の Daml Studio に相当する Script results の閲覧手段が Zed には無い。

本プロジェクトの目的は次の2つ。

1. Daml のバージョンに追従し続けられる、品質の高い Zed 拡張を自前で持つ
2. Daml Studio 相当の Script results 閲覧体験を Zed 利用者に提供する

## 2. 前提と決定事項

| 項目 | 決定 |
|---|---|
| 対象ツールチェーン | Daml 3.4+ / `dpm`（3.5 で daml assistant は削除予定） |
| 公開方針 | OSS。`zed-industries/extensions` に公開する |
| tree-sitter 文法 | `tree-sitter/tree-sitter-haskell` (MIT) を GitHub fork して自前維持 |
| リポジトリ構成 | 2リポジトリ（文法は独立、拡張とブリッジは monorepo） |
| スコープ | 段階的。フェーズ1（文法+拡張）→ フェーズ2（ブリッジ） |

### 未充足の前提条件

開発マシンに `dpm` / `daml` がインストールされていない（`which dpm daml` = not found）。
フェーズ1の LSP 起動部分は実機検証なしには完成しないため、**実装開始前に Daml SDK 3.4+
と dpm のインストールが必要**。文法（セクション 5）の作業はこの前提なしに着手できる。

## 3. 調査で確定した事実

### 3.1 Daml Studio (VSCode) の内部構造

`digital-asset/daml` の `sdk/compiler/daml-extension` を読んで確認した。

- 言語サーバの起動: `<dpm|daml> damlc <multi-ide|ide> [--telemetry|--optOutTelemetry|--telemetry-ignored] [--log-level=…] [--ide-identifier=…] [extra args]`
- アシスタント選択ロジック: `daml.useDPMWhenAvailable` が真なら `dpm` を優先し、
  無ければ `daml`。SDK バージョンが 3.4 未満なら daml assistant に強制フォールバック。
  multi-ide は SDK 2.9.0 以上で有効
- dpm の探索: `PATH` → `~/.dpm/bin/dpm`（Windows は `%APPDATA%/dpm/bin/dpm`）
- Script results は **VSCode クライアント側の機能**:
  1. サーバが CodeLens でクライアントコマンド `daml.showResource(title, uri)` を返す
  2. 拡張が Webview パネルを開き、`daml://` 仮想リソース URI を `textDocument/didOpen`
  3. サーバが `daml/virtualResource/didChange` 通知で HTML を push、拡張が描画
  4. 付随して `daml/virtualResource/note` / `didProgress`、`daml/keepAlive` リクエスト、
     SDK インストール進捗通知がある
- 提供コマンド: `daml.showResource` / `daml.openDamlDocs` / `daml.resetTelemetryConsent`
  / `daml.installRecommendedDirenv` / `daml.shutdown`
- 設定キー: `daml.logLevel` / `profile` / `telemetry` / `autorunAllTests` /
  `extraArguments` / `multiPackageIdeSupport` / `multiPackageIdeGradleSupport` /
  `useDPMWhenAvailable`
- シンタックスハイライトは TextMate 文法 (`syntaxes/daml12.tmLanguage.xml`)。Zed では使えない

### 3.2 Zed 側の制約

- Zed 拡張は WASM (`wasm32-wasip1`)。Webview もクライアント側コマンドも作れない
- `window/showDocument` は client capability に**含まれていない**
  （`crates/lsp/src/lsp.rs` の `WindowClientCapabilities` は `work_done_progress` と
  `show_message` のみ）
- CodeLens は 2026 年に実装済み（PR #54100 ほか）だが `"code_lens": "on"` でオプトイン。
  クライアント側コマンドは登録できないため、`daml.showResource` は Zed では実行できない
- `zed_extension_api` の公開最新版は **0.7.0**
- 公式レジストリの `extensions.toml` は `path` キーでリポジトリのサブディレクトリにある
  拡張を登録できる（`editors/zed`、`crates/extension` などの実例多数）→ monorepo で問題ない

**帰結**: Daml Studio 機能の大半（補完・型ホバー・診断・定義ジャンプ・リネーム・
multi-package 横断ジャンプ）は `damlc multi-ide` サーバ側にあるため、拡張は
「正しい引数でサーバを起動する」だけでよい。逆に Script results は
**Zed 拡張の枠内では原理的に実現不可能**で、別プロセスが必須。

## 4. アーキテクチャ

```
┌─ tree-sitter-daml (独立リポジトリ) ───────────────┐
│  構文木のみ。エディタ非依存。                      │
│  tree-sitter-haskell (MIT) の GitHub fork          │
└───────────────────────────────────────────────────┘
              ↑ extension.toml が repository + commit で参照
┌─ daml-zed (本リポジトリ) ─────────────────────────┐
│  editors/zed/            Zed 拡張 (wasm32-wasip1)  │
│    ・言語登録 (.daml)                               │
│    ・tree-sitter クエリ (.scm)                      │
│    ・LSP プロセスの発見と起動のみ                    │
│  crates/daml-ide-bridge/ フェーズ2                 │
│    ・LSP プロキシ (stdio 中継)                      │
│    ・Script results をブラウザへ配信                 │
└───────────────────────────────────────────────────┘
              ↓ 起動
        dpm damlc multi-ide   (Daml SDK が提供)
```

### 設計原則

**Zed 拡張は意図的に薄く保つ。** IDE 機能の実体はサーバ側にあるため、拡張に
ロジックを持たせるほど Daml のバージョン追従が難しくなる。差別化は
(a) 文法の質、(b) サーバに無い Script results の実現手段、の2点に集中させる。

### リポジトリ構成の根拠

- 文法が独立リポジトリなのは必然。tree-sitter-haskell の GitHub fork として
  `git merge upstream` を続ける必要があり、Zed も文法を `repository` + `commit`
  で取得するため
- 拡張とブリッジを同一リポジトリにするのは、両者のバージョンが密結合するため。
  拡張は GitHub Releases からブリッジのバイナリを取得して起動するので、
  拡張 vX.Y.Z はブリッジ vX.Y.Z を要求する。単一タグでリリースすれば
  バージョン skew が構造的に起きない
- ブリッジを他エディタ向けに独立プロダクト化したくなった場合は、リポジトリを
  分けずとも「バイナリの単体配布 + エディタ中立な名前」で大半の目的を達せる。
  実需が出た時点での分割は安価

## 5. フェーズ1-A: tree-sitter-daml

### 5.1 作成手順

1. `tree-sitter/tree-sitter-haskell` を GitHub で fork し、`tree-sitter-daml` にリネーム
2. `upstream` remote を残し、上流の変更を継続的に merge できる状態を維持
3. MIT の著作権表示を保持し、追加分の著作権を併記した LICENSE とする

### 5.2 追加する構文

damlc（GHC フォーク）が Haskell に追加している範囲のみ。

- **template 宣言**: `template T with <fields> where`、本体要素として
  `signatory` / `observer` / `ensure` / `agreement` / `key … maintainer` / `choice`
  / `interface instance`
- **choice 宣言**: `choice C : R with <fields> controller <exprs> do`、
  修飾子 `nonconsuming` / `preconsuming` / `postconsuming`。
  `observer` 句は `controller` の**前にのみ**置ける（damlc 3.5.7 で確認。
  後置は parse error になる）
- **interface 宣言**: `interface I where`、`viewtype`、
  `interface instance I for T where`
- `create` / `exercise` / `fetch` / `archive` は通常の関数適用であり文法追加は不要。
  ハイライトはクエリ側で扱う

### 5.3 上流追従を安く保つための制約

- Daml 固有の規則は `grammar/daml.js` に隔離する
- 上流ファイルへの差分は最小限に抑える

**実装で判明した例外（当初の想定を修正）**: 「`src/scanner.c` には触らない」は
成立しなかった。Daml は Haskell と `:` / `::` の意味が逆で、型注釈が `:`、cons が
`::` である（Daml stdlib で確認: 単一コロンの型注釈 494 箇所、`::` の型注釈 0 箇所、
`::` の出現 105 箇所はすべて cons）。これは語彙レベルの差分なのでスキャナを直す以外に
手段がない。結果として上流ファイルへの変更は次に限定した。

| ファイル | 変更 |
|---|---|
| `src/scanner.c` | `:` を予約語化、`::` を consym 化、コンストラクタ先読みの反転、Daml `with` 専用のレイアウト種別、`catch` によるレイアウト終端 |
| `grammar/lexeme.js` | `_colon2` を `:` に |
| `grammar/module.js` | `declaration` に `template` / `interface` / `exception` を追加 |
| `grammar/data.js` `grammar/exp.js` `grammar/pat.js` | `with` レコードと `;` 区切りの登録（各1〜2行） |
| `grammar/precedences.js` `grammar/conflicts.js` | `daml-clause` の優先度 |

`test/corpus` の Haskell コーパスは `script/swap-colons.py` で Daml 構文に機械変換した。

### 5.3.1 Daml `with` ブロックにレイアウト種別が必要な理由

`with` ブロックは他のどのレイアウトとも終端条件が違う。括弧の中では `=` と `,` が
他のレイアウトを終端するが、`with` ブロックではどちらもフィールドの一部である
（damlc は `(P with a = 1, b = 2)` を1つのレコードとして受理し、
`(P with a = 1, 2)` を拒否する）。一方で単一の `:` は終端する
（`key K with a; b : KeyType` は key 式への型注釈）。この違いを表現するために
`DamlLayout` という専用の ContextSort をスキャナに追加した。

### 5.4 クエリの置き場所

Zed が読むのは拡張リポジトリ側の `editors/zed/languages/daml/*.scm`。
二重管理を避けるため**正は拡張リポジトリ側**とし、文法リポジトリには
`tree-sitter test` に必要な最小限のみ置く。

### 5.5 テスト

1. **コーパステスト**: `test/corpus/daml/*.txt` に Daml 固有構文を網羅
2. **バージョン追従テスト**: CI で `digital-asset/daml` と `daml-finance` を
   shallow clone し、全 `.daml` をパースして `ERROR` / `MISSING` ノードが
   0 件であることを検証。週次 cron で回すことで、新しい Daml バージョンで
   文法が壊れたことを自動検出する

   実績: 634 ファイル中 632 が clean。残る 2 件（同一ファイルの重複）は
   **上流 tree-sitter-haskell のバグ**で、括弧付き負数で始まる do 文
   （`f = do` の次行に `(-1)`）が upstream master でもパースできない。
   スクリプト内で理由付きの allowlist にしてある

この追従テストが「Daml のバージョンに追従できていない」を構造的に防ぐ主要な仕掛けである。

## 6. フェーズ1-B: Zed 拡張 (`editors/zed/`)

### 6.1 `extension.toml`

```toml
id = "daml"
name = "Daml"
description = "Daml smart contract language support"
version = "0.1.0"
schema_version = 1
repository = "https://github.com/<owner>/daml-zed"

[grammars.daml]
repository = "https://github.com/<owner>/tree-sitter-daml"
commit = "<pinned sha>"

[language_servers.daml-language-server]
name = "Daml Language Server"
languages = ["Daml"]
language_ids = { "Daml" = "daml" }
```

`language_ids` は damlc が期待する languageId (`daml`) を明示的に送るために必要。

### 6.2 `languages/daml/config.toml`

- `name = "Daml"`, `grammar = "daml"`, `path_suffixes = ["daml"]`
- `line_comments = ["-- "]`, `block_comment = ["{- ", " -}"]`
- `tab_size = 2`, `hard_tabs = false`
- `word_characters = ["'"]`（Haskell 系のプライム付き識別子）
- brackets / autoclose 設定

### 6.3 tree-sitter クエリ

| ファイル | 内容 |
|---|---|
| `highlights.scm` | `template` / `choice` / `signatory` / `observer` / `controller` / `ensure` / `key` / `maintainer` / `interface` / `viewtype` / `nonconsuming` 等を `@keyword`、テンプレート名を `@type`、choice 名を `@constructor` |
| `outline.scm` | **テンプレートと choice をアウトラインに出す**。Daml 開発で最も効く箇所なので重点的に |
| `brackets.scm` | 括弧対応 |
| `indents.scm` | インデント |
| `textobjects.scm` | テキストオブジェクト |
| `injections.scm` | 言語インジェクション |

### 6.4 `src/lib.rs`

依存は `zed_extension_api = "0.7"`。

`language_server_command()` の決定順序:

1. `LspSettings::for_worktree()` の `binary.path` / `binary.arguments` があれば最優先
2. `worktree.which("dpm")` で dpm を探す
3. 見つからなければ、インストール導線を含む明示的なエラーメッセージを返す
4. 引数: `["damlc", "multi-ide", "--telemetry-ignored", "--log-level=<設定値>"]`
5. `env: worktree.shell_env()`

設定は Zed の `lsp."daml-language-server".settings` 配下に置き、
`LspSettings::for_worktree()` 経由で読む。

| キー | 型 | 既定値 | 意味 |
|---|---|---|---|
| `log_level` | `"Debug" \| "Info" \| "Warning" \| "Error"` | `"Warning"` | `--log-level=` に渡す。VSCode 版と同じ既定値。`"Telemetry"` は提供しない（セクション 6.5） |
| `extra_arguments` | `string[]` | `[]` | `damlc multi-ide` への追加引数。例: `["--ghc-option", "-W"]` |

`multi-ide` は dpm 使用時には常に有効であり、無効化できるのは legacy daml assistant
使用時のみ（VSCode 版の `daml.multiPackageIdeSupport` の説明に明記されている）。
本拡張は dpm 前提なので、**`multi-ide` の on/off 設定は提供しない**。
`ide` を使いたい場合は `binary.path` / `binary.arguments` による完全上書きで対応する。

`label_for_completion()` / `label_for_symbol()`: `name :: Type` の形を Haskell 文法で
色付けして表示する。既存2拡張が同等の実装を持ち、どちらも Apache-2.0 なので
参考にする、あるいは帰属表示のうえ取り込んでよい。

### 6.5 テレメトリ

**常に `--telemetry-ignored` を渡す。** VSCode 版のような同意 UI を Zed 拡張で
出す手段が無く、利用者に無断で送信することは受け入れられないため、送らない一択とする。

### 6.6 SDK バージョン分岐を実装しない理由

VSCode 版は SDK < 3.4 なら daml assistant、< 2.9 なら multi-ide 無効、という分岐を
持つ。本プロジェクトは「Daml 3.4+ / dpm 中心」で確定しているため、この分岐は
実装しない（YAGNI）。古い SDK の利用者には、`lsp."daml-language-server".binary.path` と
`binary.arguments` による完全上書きというエスケープハッチのみ提供する。

### 6.7 利用者向けドキュメント

README に記載する内容:

- Daml SDK 3.4+ / dpm のインストール手順へのリンク
- `"code_lens": "on"` の設定（フェーズ2で意味を持つ）
- `lsp."daml-language-server".binary.path` による dpm パスの上書き方法
- `log_level` / `extra_arguments` の説明

## 7. テスト戦略

Zed 拡張の WASM 部分は自動テストが困難であるため、次の4層で担保する。

1. **純ロジックの単体テスト**: 引数組み立てとエラー分岐を純関数に切り出し、
   `cargo test`（ネイティブターゲット）で網羅する。`worktree` は trait で
   抽象化してモックを注入する
2. **WASM ビルド検証**: CI で `cargo build --target wasm32-wasip1` が通ることを保証
3. **クエリ検証**: tree-sitter CLI の `tree-sitter query` をサンプル `.daml` に対して
   実行し、想定したキャプチャが得られることを CI で検証
4. **手動検証チェックリスト**: dev extension としてローカル install し、
   `.daml` を開く / 補完 / 型ホバー / 診断 / 定義ジャンプ / multi-package 横断ジャンプ
   を確認する。手順は README に記載

## 8. リリース手順

1. `tree-sitter-daml` でタグを切る
2. `daml-zed` の `editors/zed/extension.toml` の `commit` を更新
3. `daml-zed` でタグを切る
4. `zed-industries/extensions` に version bump の PR を出す（`path = "editors/zed"`）

この手順を `.github/workflows/` と `RELEASING.md` に固定する。

## 9. フェーズ2: daml-ide-bridge（方向性のみ）

フェーズ2は別途 spec を書く。ここでは方向性のみ記録する。

Zed は Webview もクライアント側コマンドも持たず、`window/showDocument` も
サポートしないため、エディタ内に Script results を出すことは原理的に不可能。
したがってブリッジは次の形をとる。

- Zed ↔ ブリッジ ↔ `dpm damlc multi-ide` の stdio を中継する LSP プロキシ
- サーバからの `daml/virtualResource/didChange`（Script 実行結果の HTML）を横取りし、
  ブリッジ内蔵の localhost HTTP サーバで配信する
- ブラウザを Zed の横に置いておくと、保存のたびに結果が自動更新される（SSE で push）
- サーバが返す CodeLens の `daml.showResource` は Zed では実行できないため、
  ブリッジが CodeLens を書き換え、加えて同等の **Code Action**（Zed が確実に
  サポートする）を注入する

VSCode のようなエディタ内パネルにはならないが、常時開いた自動更新ブラウザという形で
実用上は同等の体験を狙う。

フェーズ1の `language_server_command()` は「返すコマンドを差し替えるだけ」で
ブリッジを挟めるため、フェーズ1側に前借りの複雑さは発生しない。

## 10. 非目標

明示的に実装しないもの。

- `.daml-core`（コンパイラ内部用の言語）のサポート
- テレメトリ送信
- Gradle multi-IDE 対応（`--ide-identifier`）
- DAP（デバッグアダプタ）
- フォーマッタ統合（damlc に公式フォーマッタが存在しないため）
- SDK 3.4 未満の自動検出とフォールバック（設定によるエスケープハッチのみ提供）

## 11. フェーズ1 の完了条件

- Zed で `.daml` を開くと、`template` / `choice` / `signatory` / `interface instance`
  が正しくハイライトされる
- アウトラインにテンプレートと choice が表示される
- 補完・型ホバー・診断・定義ジャンプが動作する
- multi-package プロジェクトで横断ジャンプが動作する
- 文法の追従テストが CI で緑
- `zed-industries/extensions` にマージされ、公式レジストリからインストールできる
