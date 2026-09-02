# daml-ide-bridge — 設計（フェーズ2）

- 日付: 2026-09-02
- ステータス: 実装プランへ
- 対象: `crates/daml-ide-bridge/`
- 前提 spec: `2026-09-02-daml-zed-design.md`（セクション 9 の方向性を具体化したもの）

## 1. 目的

Daml Studio の Script results 相当を Zed 利用者に提供する。

Zed 拡張は WebAssembly で動き、Webview もクライアント側コマンドも持てず、
Zed の LSP クライアントは `window/showDocument` を advertise しない
（`crates/lsp/src/lsp.rs` の `WindowClientCapabilities` は `work_done_progress` と
`show_message` のみ）。したがってエディタ内に結果を描画することは**原理的に不可能**で、
別プロセスが必要になる。

## 2. 実測したプロトコル

すべて `dpm damlc multi-ide`（SDK 3.5.7）に実際に喋らせて確認した。
再現用スクリプトを `crates/daml-ide-bridge/scripts/probe-protocol.py` に置いてある。
Daml を上げたあとに同じ手順でプロトコルを確認し直せる。

```sh
DAML_PROJECT=/path/to/built/project ./scripts/probe-protocol.py
```

### 2.1 initialize が返す能力

```json
{
  "codeLensProvider":   { "resolveProvider": false, "workDoneProgress": false },
  "codeActionProvider": true,
  "executeCommandProvider": { "commands": ["typesignature.add"] },
  "hoverProvider": true, "completionProvider": {...}, "definitionProvider": true,
  "documentSymbolProvider": true, "semanticTokensProvider": {...},
  "textDocumentSync": {...}, "workspace": {...}
}
```

`serverInfo` は返さない。

### 2.2 Script results への入口

ビルド済みのパッケージで Script を含むファイルを `textDocument/didOpen` したあと
`textDocument/codeLens` を投げると返る。

```json
{
  "range": { "start": {"line": 7, "character": 0},
             "end":   {"line": 7, "character": 5} },
  "command": {
    "command": "daml.showResource",
    "title": "Script results",
    "arguments": [
      "Script: setup",
      "daml://compiler?file=%2F...%2FTest.daml&top-level-decl=setup"
    ]
  }
}
```

`daml.showResource` は **VS Code 拡張がクライアント側に登録するコマンド**であり、
サーバの `executeCommandProvider.commands` には含まれない。

コードレンズが返るのは Script が実際に評価できるときだけである。依存 DAR が未ビルドだと
診断が出て、レンズは空配列になる。ビルド後は初回で 10〜60 秒かかることがある。

### 2.3 仮想リソース

コマンド引数の `daml://` URI を `textDocument/didOpen` すると、サーバが

```json
{ "method": "daml/virtualResource/didChange",
  "params": { "uri": "daml://compiler?...", "contents": "<!DOCTYPE HTML>..." } }
```

を送ってくる。付随して `daml/virtualResource/didProgress` も飛ぶ。

**ソースを `textDocument/didChange` で編集すると、新しい `didChange` が push される。**
実測で確認済み（`"TV"` を `"Radio"` に書き換えたら、新しい HTML に `Radio` が現れた）。
つまりライブ更新はサーバ側の機能で、ブリッジは中継するだけでよい。

### 2.4 HTML の中身

8,995 バイトの完結した HTML。ただし2点、埋めるべき穴がある。

1. `<script src="$webviewSrc"></script><link rel="stylesheet" href="$webviewCss">`
   という**プレースホルダ**が入っており、VS Code 拡張は自前の `webview.js` /
   `webview.css` の URI で置換している
2. CSS 変数を10個参照している。`--link-color` と、
   `--vscode-terminal-ansi{Red,BrightRed,Yellow,BrightYellow,Green,Blue,BrightBlue,Magenta,White}`

`webview.js`（2,275 バイト）と `webview.css`（1,312 バイト）は
`digital-asset/daml` の `sdk/compiler/daml-extension/src/` にあり Apache-2.0。
帰属表示のうえ vendoring する。テーブル表示とトランザクション表示の切り替え、
archived 契約の表示切り替えといった、HTML 内の `onclick` が呼ぶ関数の実体である。

## 3. アーキテクチャ

```
Zed ──stdio LSP──> daml-ide-bridge ──stdio LSP──> dpm damlc multi-ide
                        │
                        ├─ HTTP + SSE (127.0.0.1:<port>)
                        │
                     ブラウザ（Zed の横に置いておく）
```

ブリッジは LSP のトランスペアレントなプロキシで、以下だけに介入する。

| 方向 | メッセージ | 介入 |
|---|---|---|
| server→client | `initialize` の結果 | `executeCommandProvider.commands` に `daml.showResource` を追加する |
| server→client | `textDocument/codeLens` の結果 | レンズはそのまま通す。Zed が `code_lens: "on"` なら表示される |
| server→client | `textDocument/codeAction` の結果 | Script のある行に「Show script results」アクションを**注入**する |
| client→server | `workspace/executeCommand` の `daml.showResource` | 転送せずブリッジが処理し、`null` を返す |
| server→client | `daml/virtualResource/didChange` | 横取りして保持・配信し、Zed には転送しない |
| server→client | `daml/virtualResource/didProgress` / `note` | 横取りして「実行中」表示に使う。Zed には転送しない |
| client→server | それ以外すべて | 素通し |

### 3.1 コードアクションを注入する理由

Zed のコードレンズ対応は 2026 年に入ったばかりで `"code_lens": "on"` のオプトインである。
コードアクションは確実に動く。したがって**主たる入口はコードアクション**とし、
コードレンズは有効にしている人へのボーナスとする。

コードアクションの注入には、その範囲にどの Script があるかを知る必要がある。
ブリッジはファイルごとに最後に得たコードレンズを保持し、
`textDocument/codeAction` の要求範囲と交差するものをアクションに変換する。
コードレンズをまだ持っていないファイルでは、ブリッジが自分で
`textDocument/codeLens` をサーバに投げて補充する。

### 3.2 なぜ Zed に `daml/virtualResource/*` を転送しないか

Zed は未知の通知を無視するだけだが、9KB の HTML を毎回エディタに送るのは無駄であり、
ログを汚す。ブリッジで止める。

## 4. HTTP サーバ

| パス | 内容 |
|---|---|
| `GET /` | 開いている Script results の一覧。何も開いていなければその旨 |
| `GET /r/{id}` | 1つの結果ページ。SSE で自動更新する |
| `GET /r/{id}/events` | SSE ストリーム。`data:` に HTML 断片ではなく更新イベントを流し、ページ側が再取得する |
| `GET /r/{id}/body` | 最新の HTML 本体。SSE で更新通知を受けたページがこれを取りに来る |
| `GET /assets/webview.js` | vendoring した VS Code 拡張の webview.js |
| `GET /assets/webview.css` | 同 webview.css |
| `GET /assets/theme.css` | CSS 変数の定義（セクション 4.1） |

`{id}` は `daml://` URI の安定ハッシュ（blake3 の先頭 16 桁を hex 化）。URI をそのまま
パスに入れるとエスケープが面倒で、ログにも出したくないため。

**バインドは 127.0.0.1 のみ**、ポートは 0 を指定して OS に選ばせる。選ばれたポートは
起動時に stderr へ 1 行出す。ローカルのみとはいえ、同じマシンの他プロセスから
プロジェクトのソースが読めてしまうため、`/r/{id}` は起動時に生成したランダムな
トークンをクエリに要求する。ブリッジが開くリンクにはトークンが含まれる。

### 4.1 テーマ

`theme.css` は `prefers-color-scheme` で切り替わる2組の CSS 変数を定義する。
VS Code のテーマ変数名をそのまま使うのは、サーバが吐く HTML がその名前を参照しており、
書き換えるとサーバ側の変更に追従できなくなるため。エディタのテーマとの同期はしない
（Zed のテーマを外から読む安定した手段がない）。

## 5. ブラウザを開く

`daml.showResource` を処理したとき、その `{id}` のページをまだどのブラウザも開いていなければ
`open`（macOS）/ `xdg-open`（Linux）/ `cmd /c start`（Windows）で開く。
すでに開いていれば何もしない。ページ側は SSE の再接続で生存を通知し、
最後の接続が切れて 5 分経った仮想リソースはサーバに `textDocument/didClose` を送って解放する。

`--no-open` で自動オープンを止められる。CI や、ブラウザを自分で管理したい人向け。

## 6. Zed 拡張との統合

`language_server_command()` の決定順序を次に変える。

1. `binary.path` が設定されていれば従来どおりそれを使う（ブリッジも dpm も介さない）
2. `script_results` 設定が `false` なら、フェーズ1と同じく `dpm damlc multi-ide` を直接起動する
3. ブリッジのバイナリを探す
   - `bridge_path` 設定
   - `PATH` 上の `daml-ide-bridge`
   - 拡張の作業ディレクトリにダウンロード済みのもの
   - なければ GitHub Releases から、**拡張自身のバージョンと同じタグ**を取得する
4. 見つかったら `daml-ide-bridge -- dpm damlc multi-ide <args>` を起動する
5. ダウンロードに失敗したら警告を出して 2 にフォールバックする。
   Script results が見られないだけで、他の機能は全部動く状態を保つ

拡張とブリッジを同一リポジトリに置いたのは、この「同じタグを取る」が成立するためである。

### 6.1 追加する設定

| キー | 型 | 既定値 | 意味 |
|---|---|---|---|
| `script_results` | bool | `true` | ブリッジを挟んで Script results を有効にする |
| `bridge_path` | string | なし | ブリッジのバイナリを直接指定する |
| `bridge_args` | string[] | `[]` | ブリッジへの追加引数（`--no-open` など） |

## 7. 実装の単位

| ファイル | 責務 |
|---|---|
| `src/main.rs` | 引数解析と起動。他は呼ぶだけ |
| `src/framing.rs` | LSP の `Content-Length` フレーミング。読み書きだけ |
| `src/proxy.rs` | 2本のパイプの中継と、介入ポイントの振り分け |
| `src/intercept.rs` | どのメッセージをどう書き換えるかの純ロジック。`serde_json::Value` を受けて `Value` を返すだけで I/O を持たない |
| `src/resources.rs` | 仮想リソースの状態（URI、タイトル、最新 HTML、購読者） |
| `src/http.rs` | HTTP と SSE |
| `src/assets/` | vendoring した `webview.js` / `webview.css` と自前の `theme.css` |
| `src/open.rs` | ブラウザ起動。プラットフォーム分岐だけ |

`intercept.rs` に I/O を持たせないのが要。LSP メッセージの書き換えは全部ここに集め、
JSON in / JSON out の単体テストで固める。フェーズ1で `server.rs` を分離したのと同じ理由で、
プロキシの配管はテストしにくく、書き換えの規則はテストしやすい。

## 8. テスト戦略

1. **`intercept.rs` の単体テスト**: 実測した JSON をそのままフィクスチャにする。
   initialize 結果へのコマンド追加、コードアクション注入、`daml.showResource` の捕捉、
   `virtualResource/didChange` の吸収、それ以外が素通しであること
2. **`framing.rs` の単体テスト**: 分割到着、複数フレーム連結、不正ヘッダ
3. **統合テスト**: ブリッジを実際に `dpm damlc multi-ide` の前に立て、
   フェーズ1で書いた LSP プローブと同じ手順を踏んで、
   コードアクションが返り、HTTP で HTML が取れることを確認する。
   `dpm` が無い環境ではスキップする
4. **手動確認**: Zed から使い、ブラウザが開いて編集に追随することを見る

## 9. 非目標

- エディタ内での描画（不可能）
- Zed のテーマとの同期
- Script のデバッガ、ステップ実行
- リモート接続（127.0.0.1 のみ）
- VS Code 拡張との設定互換

## 10. 完了条件

- Zed で Script のある行にコードアクション「Show script results」が出る
- 実行するとブラウザが開き、トランザクション表示とテーブル表示を切り替えられる
- ソースを編集して保存すると、ブラウザの内容が自動で更新される
- ブリッジが無い/落ちる環境でも、フェーズ1と同じ機能がそのまま動く
- `cargo test` がブリッジの書き換えロジックを網羅している
