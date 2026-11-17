# CLIリファレンス

```text
ajiro-api [OPTIONS] <COMMAND>
```

全コマンド共通のオプション:

| オプション | 説明 |
| --- | --- |
| `-C, --dir <DIR>` | プロジェクトのディレクトリ（既定: カレントディレクトリ。`ajiro-api.yaml` を親方向に探索） |
| `-h, --help` | ヘルプ |
| `-V, --version` | バージョン |

終了コード:

| コード | 意味 |
| --- | --- |
| `0` | すべて成功 |
| `1` | テスト失敗、`scan --check` で差分あり、検証エラー |
| `2` | ツール自体が実行できなかった（設定・I/Oエラーなど） |

`--json` を持つコマンドは、エージェントが読める安定した出力を返します。出力中のトークンやパスワードは `***` で伏せられます。

## init

`ajiro-api.yaml` と `api/` ディレクトリを作ります。フレームワークは自動判定します。

| オプション | 説明 |
| --- | --- |
| `--framework <名前>` | `nextjs` `hono` `fastapi` `express` `go-nethttp` `go-chi` `go-echo`（省略時は自動判定） |
| `--force` | 既存の `ajiro-api.yaml` を上書き |
| `--base-url <URL>` | `local` 環境のベースURL。省略時はポートを自動検出します（`.env.example` の `PORT=`、`package.json` の `-p`/`--port`、コード中の `serve({ port })`・`process.env.PORT ?? 8000`・`.listen(8000)`・`ListenAndServe(":8080")` など）。見つからなければフレームワークの既定値です |
| `--json` | 機械向け出力 |

## scan

ルーティングを静的解析し、エンドポイントのYAMLを作成・更新します。書き換えるのは最上位の `source:` ブロックだけで、`tests`・`mock`・コメントには触れません。コードから消えたルートのYAMLは `stale` として報告するだけで、削除しません。

```bash
ajiro-api scan [OPTIONS] [PATHS]...
```

| オプション | 説明 |
| --- | --- |
| `[PATHS]...` | 解析するディレクトリまたはファイル（既定: `ajiro-api.yaml` の `sources`） |
| `--framework <名前>` | フレームワーク（既定: 設定ファイルまたは自動判定） |
| `--dry-run` | ファイルを書かずに変更内容を表示 |
| `--check` | YAMLが古ければ終了コード1（CI向け）。何も書き込まない |
| `--json` | 機械向け出力 |

`--json` の主な項目: `framework`、`endpoints[]`（`id` `method` `path` `file` `source` `status`＝`created`/`updated`/`unchanged`）、`created`、`updated`、`unchanged`、`stale`、`warnings`、`duration_ms`。

## test

YAMLのリクエストを実行し、レスポンスを検証します。`depends_on` の順に実行し、`tests` が空のエンドポイントには暗黙の `smoke` テスト（ステータス500未満）を適用します。

```bash
ajiro-api test [OPTIONS] [FILTER]
```

| オプション | 説明 |
| --- | --- |
| `[FILTER]` | `id`・ファイル・テスト名に含まれる文字列で絞り込み |
| `-e, --env <名前>` | `ajiro-api.yaml` の環境（既定: `default_env`） |
| `--base-url <URL>` | 環境のベースURLを上書き |
| `--json` | 機械向け出力（失敗箇所、期待値、実際の値、ソースの位置） |
| `--junit <FILE>` | JUnit XMLも書き出す |
| `-u, --update-snapshots` | 保存済みスナップショットを実際のレスポンスで上書き |
| `--bail` | 最初の失敗で停止 |
| `--timeout <MS>` | リクエストのタイムアウト（ミリ秒） |
| `-v, --verbose` | 成功したテストのリクエスト・レスポンスも表示 |

`--json` の主な項目:

- `report_version`、`env`、`base_url`、`ok`
- `summary`: `total` `passed` `failed` `skipped` `errors` `duration_ms`
- `results[]`: `endpoint_id` `file` `test` `test_index` `status`（`passed`/`failed`/`skipped`/`error`）`duration_ms` `request` `response` `failures[]` `error` `source`
- `failures[]`: `check`（`status` `header` `body.equals` `body.schema` `body.match` `body.snapshot` `body.contains` `max_ms` `capture`）、`path`、`expected`、`actual`、`message`
- `load_errors[]`: 読み込めなかったファイル

`status` が `error` のものは、接続拒否やテンプレートエラーなど、テストを実行できなかったものです。

## list

定義済みのエンドポイントを一覧表示します。

| オプション | 説明 |
| --- | --- |
| `--json` | 機械向け出力（`endpoints[]` と `load_errors[]`） |

## validate

YAMLの構文・スキーマ・参照（重複する `id`、存在しない `depends_on`、未対応のメソッド、`/` で始まらない `path`、空のテスト名など）を検証します。問題があれば終了コード1です。

| オプション | 説明 |
| --- | --- |
| `--json` | 機械向け出力（`ok` と `errors[]`） |

## mock

`mock:` のサンプルレスポンスを返すサーバーを起動します。

| オプション | 説明 |
| --- | --- |
| `-p, --port <PORT>` | 待ち受けポート（既定: `4010`） |
| `--host <HOST>` | バインドするホスト（既定: `127.0.0.1`） |
| `--scenario <名前>` | シナリオ名。そのシナリオがないエンドポイントは `default` |
| `--delay <MS>` | 全レスポンスに足す遅延（ミリ秒） |

リクエストごとに、`X-Ajiro-Scenario` ヘッダーまたは `?__scenario=` でもシナリオを選べます。

## mcp

MCPサーバーをstdioで起動します。ツール: `scan`、`test`、`list`、`validate`、`get_endpoint`、`schema`。

```json
{ "mcpServers": { "ajiro-api": { "command": "npx", "args": ["-y", "ajiro-api", "mcp"] } } }
```

## openapi

```bash
ajiro-api openapi import <FILE> [--overwrite] [--json]
ajiro-api openapi export [-o <FILE>] [--title <TITLE>]
```

| サブコマンド | 説明 |
| --- | --- |
| `import <FILE>` | OpenAPI 3.x（JSONまたはYAML）からエンドポイントYAMLを作成・更新。`--overwrite` で既存ファイルの `request` と `mock` を上書き（`tests` は常に保持） |
| `export` | エンドポイントYAMLからOpenAPI 3.1を書き出す。`-o` は `.json` または `.yaml`（省略時は標準出力）、`--title` でAPIのタイトル |

## import

```bash
ajiro-api import postman <FILE> [--overwrite] [--json]
ajiro-api import bruno <DIR> [--overwrite] [--json]
```

| サブコマンド | 説明 |
| --- | --- |
| `postman <FILE>` | Postman Collection v2.1のJSONを取り込む |
| `bruno <DIR>` | Brunoコレクションのディレクトリ（`.bru`）を取り込む |

## docs

単一HTMLのAPI仕様書を生成します。

| オプション | 説明 |
| --- | --- |
| `-o, --out <FILE>` | 出力ファイル（既定: `api-docs.html`） |
| `--title <TITLE>` | ドキュメントのタイトル |
| `-e, --env <名前>` | 例に表示するベースURLの環境 |

## schema

JSON Schemaを標準出力へ出します。

```bash
ajiro-api schema [endpoint|config]
```

`endpoint`（既定）は `api/**/*.yaml`、`config` は `ajiro-api.yaml` のスキーマです。

## telemetry

匿名の利用統計への同意を確認・変更します。初期状態はオフです。

```bash
ajiro-api telemetry [on|off|status]
```

オンにした場合に扱うのは、コマンド名・バージョン・OSごとの実行回数だけです。パス、URL、API定義は含みません。`DO_NOT_TRACK=1` を設定すると常に無効です。`off` にすると、ローカルに溜めた回数も削除されます。
