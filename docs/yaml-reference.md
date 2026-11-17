# YAMLリファレンス

Ajiro APIが読み書くファイルは2種類です。

- エンドポイントファイル: `api/<リソース>/<メソッド>-<パス>.yaml`（エンドポイント1つにつき1ファイル）
- プロジェクト設定: `ajiro-api.yaml`

どちらも未知のフィールドはエラーになります（typoの検出のため）。JSON Schemaは `ajiro-api schema endpoint` / `ajiro-api schema config` で取得でき、[schema/](../schema/) にも置いています。エディタ（YAML Language Server）では、ファイル先頭に次の1行を書くと補完と検証が効きます。

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/endpoint.schema.json
```

## エンドポイントファイル

```yaml
version: 1
id: users.create
summary: ユーザーを作成する
tags: [users]
depends_on: [auth.login.create]
request:
  method: POST
  path: /users
  headers:
    Authorization: Bearer {{ token }}
  body:
    name: Taro
    email: "{{ $randomEmail }}"
source:
  framework: hono
  file: src/routes/users.ts
  line: 42
tests: []
mock: {}
```

### トップレベル

| フィールド | 型 | 必須 | 説明 |
| --- | --- | --- | --- |
| `version` | 整数 | はい | スキーマのバージョン。現在は `1` |
| `id` | 文字列 | はい | プロジェクト内で一意な識別子（例: `users.create`）。`depends_on`、スナップショット、`test` の絞り込みで使う |
| `summary` | 文字列 | いいえ | 1行の概要 |
| `description` | 文字列 | いいえ | 詳しい説明（Markdown可） |
| `tags` | 文字列の配列 | いいえ | 分類用タグ |
| `task` | 文字列 | いいえ | Ajiroのタスク（例: `TSK-012`）。Phase 3の連携用 |
| `depends_on` | 文字列の配列 | いいえ | 先に実行するエンドポイントの `id`。存在しない `id` は `validate` がエラーにする |
| `request` | オブジェクト | はい | 基本のリクエスト |
| `source` | オブジェクト | いいえ | `scan` が書く。**手で編集しない** |
| `tests` | 配列 | いいえ | テストケース。人とAIが書く。`scan` は触らない。空なら暗黙の `smoke` テスト（ステータス500未満）を実行 |
| `mock` | オブジェクト | いいえ | `ajiro-api mock` が返すサンプルレスポンス |

### `request`

| フィールド | 型 | 必須 | 説明 |
| --- | --- | --- | --- |
| `method` | 文字列 | はい | 大文字のHTTPメソッド（`GET` `POST` `PUT` `PATCH` `DELETE` `HEAD` `OPTIONS`） |
| `path` | 文字列 | はい | `/` で始まるOpenAPI形式のパス。パラメータは `{id}` |
| `params` | マップ | いいえ | `path` の `{param}` に入れる値 |
| `query` | マップ | いいえ | クエリ文字列。値が `null` のキーは送信しません（`scan`・`openapi import` が、存在は分かるが値が不明なパラメータを `null` で書き出します） |
| `headers` | マップ（文字列） | いいえ | リクエストヘッダー。秘密情報は書かず `{{ env.NAME }}` を使う |
| `body` | 任意のYAML値 | いいえ | リクエストボディ。オブジェクトや配列はJSONで送られる |

フレームワーク固有のパス表記は、`scan` が次のように正規化します。`:id`（Express・Hono・Echo）、`[id]`（Next.js）、`{id:int}`（FastAPI）、`{id...}`（Go）はすべて `{id}` になります。

### `source`（`scan` が書く）

| フィールド | 型 | 説明 |
| --- | --- | --- |
| `framework` | 文字列 | `nextjs` `hono` `fastapi` `express` `go-nethttp` `go-chi` `go-echo` |
| `file` | 文字列 | プロジェクトルートからの相対パス（`/` 区切り） |
| `line` | 整数 | ルート定義の行番号 |
| `handler` | 文字列 | ハンドラ名（分かる場合のみ） |

### `tests[]`

| フィールド | 型 | 説明 |
| --- | --- | --- |
| `name` | 文字列（必須） | テスト名。ファイル内で一意にする。スナップショットのファイル名にも使う |
| `request` | オブジェクト | `request` の上書き（下記） |
| `expect` | オブジェクト | 期待するレスポンス |
| `capture` | マップ | レスポンスから変数を取り出す |
| `skip` | 真偽値 | `true` でスキップ |
| `only_env` | 文字列の配列 | 指定した環境でのみ実行（空なら全環境） |

#### `tests[].request`（上書き）

`method`、`path`、`params`、`query`、`headers`、`body` を指定できます。

- `params`・`query`・`headers` はキー単位でマージされます。
- `body` がオブジェクトの場合は深くマージされ、値が `null` のキーは削除されます。それ以外の型の `body` は丸ごと置き換えます。

```yaml
- name: メール重複は409
  request:
    body: { email: dup@example.com }   # name など他のキーは基本のbodyのまま
  expect:
    status: 409
```

#### `expect`

| フィールド | 型 | 説明 |
| --- | --- | --- |
| `status` | 整数 または 整数の配列 | 期待するステータス。配列はいずれかに一致すればOK |
| `headers` | マップ | ヘッダー名（大文字小文字を区別しない）→期待値。`/` で始まり `/` で終わる値は正規表現（例: `/^application\/json/`） |
| `body` | オブジェクト | ボディの検証（下記） |
| `ignore` | JSONPathの配列 | `equals` と `snapshot` の比較から除外する項目（ID・日時など） |
| `max_ms` | 整数 | 応答時間の上限（ミリ秒） |

#### `expect.body`

| フィールド | 型 | 説明 |
| --- | --- | --- |
| `equals` | 任意のYAML値 | ボディ全体との完全一致（`ignore` を除く）。壊れやすいので `match` と `schema` を優先 |
| `schema` | 文字列 または オブジェクト | JSON Schema。文字列はこのYAMLからの相対パス、オブジェクトはインラインのスキーマ |
| `match` | マップ | JSONPath→期待値（部分一致）。値はリテラルかマッチャー |
| `snapshot` | 真偽値 | `api/__snapshots__/<id>/<テスト名のslug>.json` と比較 |
| `contains` | 文字列 | JSONでない（テキスト）ボディに含まれるべき文字列 |

複数を同時に指定でき、すべてを満たす必要があります。

#### マッチャー（`match` の値）

オブジェクトで、キーはちょうど1つです。

| マッチャー | 意味 |
| --- | --- |
| `{ $exists: true }` | パスが存在する（`false` なら存在しない） |
| `{ $type: string }` | 型。`string` `number` `integer` `boolean` `array` `object` `null` |
| `{ $regex: "^usr_" }` | 文字列が正規表現に一致 |
| `{ $contains: x }` | 文字列の部分一致、または配列に要素を含む |
| `{ $len: 3 }` / `{ $len: { $gte: 1 } }` | 配列・文字列・オブジェクトの長さ（完全一致、または数値マッチャー） |
| `{ $gt: 0 }` `{ $gte: 0 }` `{ $lt: 10 }` `{ $lte: 10 }` | 数値の比較 |
| `{ $ne: x }` | 等しくない |
| `{ $oneOf: [open, done] }` | いずれかに等しい |

```yaml
expect:
  status: 200
  body:
    match:
      $.items: { $len: 3 }
      $.items[0].id: { $type: string }
      $.status: { $oneOf: [open, done] }
```

**JSONPathの扱い**

- 構文は RFC 9535 です。フィルターは `[?@.done == true]`、正規表現は `[?match(@.id, '^usr_')]` と書きます（`=~` は使えません）。
- `$.a.b` のように1つの値を指すパスは、その値と比較します。
- `[*]`・`..`・`[?...]` のように複数の値を指しうるパスは、**一致したすべての値の配列**と比較します。例: `$.items[*].id: { $contains: "{{ todoId }}" }`、`$.items[?@.done == true]: { $len: { $gte: 1 } }`。何も一致しない場合、`{ $exists: false }` は成功します。
- `match` のキー（JSONPath）にもテンプレートを書けます。
- テストの `request.headers` で値を `""` にすると、そのヘッダーは送信されません（例: `Authorization: ""` で401を確認）。

#### `capture`

`変数名: 取り出し方` の形です。取り出し方は次のいずれかです。

| 値 | 取り出すもの |
| --- | --- |
| `$.jsonpath` | JSONレスポンスのJSONPathの値（例: `$.token`） |
| `header:Name` | レスポンスヘッダー（例: `header:Location`） |
| `status` | ステータスコード |
| `body` | ボディ全体 |

取り出した変数は、後続のテストと、その `id` を `depends_on` に書いたエンドポイントで `{{ 変数名 }}` として使えます。

### `mock`

```yaml
mock:
  default: { status: 201, body: { id: usr_001, name: Taro } }
  scenarios:
    conflict: { status: 409 }
    slow: { status: 200, delay_ms: 3000 }
```

| フィールド | 説明 |
| --- | --- |
| `default` | シナリオ指定がないとき、または該当シナリオが無いときのレスポンス |
| `scenarios` | シナリオ名→レスポンス |

レスポンスは `status`（既定200）、`headers`、`body`、`delay_ms`（遅延、ミリ秒）を持てます。シナリオは `ajiro-api mock --scenario <名前>`、`X-Ajiro-Scenario` ヘッダー、`?__scenario=<名前>` で選びます。

## テンプレート

文字列の中で使えます（`params`・`query`・`headers`・`body`・環境の `variables` など）。

| 書き方 | 意味 |
| --- | --- |
| `{{ name }}` | `capture` した変数、または環境の `variables` |
| `{{ env.NAME }}` | `.env` ファイルの値。無ければプロセスの環境変数（CIではSecretsを環境変数で渡せます） |
| `{{ $uuid }}` | UUID |
| `{{ $timestamp }}` | UNIX時刻（秒） |
| `{{ $timestampMs }}` | UNIX時刻（ミリ秒） |
| `{{ $isoDate }}` | ISO 8601形式の日時 |
| `{{ $randomInt }}` | ランダムな整数 |
| `{{ $randomEmail }}` | ランダムなメールアドレス |

`test` は `depends_on` の順にエンドポイントを実行します。

## プロジェクト設定 `ajiro-api.yaml`

```yaml
version: 1
framework: hono
sources: [src]
exclude: [node_modules, dist]
api_dir: api
env_file: .env
default_env: local
environments:
  local:
    base_url: http://localhost:3000
    variables:
      adminPassword: "{{ env.ADMIN_PASSWORD }}"
  staging:
    base_url: https://staging.example.com
    env_file: .env.staging
    headers:
      X-Client: ajiro-api
redact: [x-session-id]
timeout_ms: 10000
```

| フィールド | 既定値 | 説明 |
| --- | --- | --- |
| `version` | （必須） | `1` |
| `framework` | 自動判定 | `nextjs` `hono` `fastapi` `express` `go-nethttp` `go-chi` `go-echo` |
| `sources` | `["."]` | スキャン対象のディレクトリ（プロジェクトルートからの相対） |
| `exclude` | なし | スキャンから除外するパスの部分文字列 |
| `api_dir` | `api` | エンドポイントYAMLのディレクトリ |
| `env_file` | `.env` | 秘密情報を置くdotenvファイル。`{{ env.NAME }}` で参照 |
| `default_env` | `local` | `--env` を省略したときの環境 |
| `environments` | `local`（`http://localhost:3000`） | 環境の定義 |
| `redact` | なし | ログで `***` に伏せる追加のヘッダー・JSONキー名（大文字小文字を区別しない） |
| `timeout_ms` | `10000` | リクエストのタイムアウト（ミリ秒） |

`environments.<名前>` のフィールド:

| フィールド | 説明 |
| --- | --- |
| `base_url`（必須） | サーバーのベースURL。`ajiro-api test --base-url` で上書き可 |
| `variables` | `{{ name }}` で使える変数。値にテンプレートも書ける |
| `headers` | この環境の全リクエストに付けるヘッダー |
| `env_file` | この環境専用のdotenvファイル（プロジェクトの `env_file` より優先） |

## ファイルの配置とスナップショット

```text
ajiro-api.yaml
.env                          # Gitに入れない
api/
  auth/post-auth-login.yaml
  todos/get-todos.yaml
  todos/get-todos-id.yaml
  schemas/todo.json           # expect.body.schema から参照
  __snapshots__/<id>/<テスト名のslug>.json
```

スナップショットは初回実行時に作られ、`ajiro-api test -u` で更新します。意図した変更のときだけ更新してください。
