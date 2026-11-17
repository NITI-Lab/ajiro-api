# Ajiro API

**AIが書いたAPIを、AI自身にテストさせる。**

[![npm](https://img.shields.io/npm/v/ajiro-api?label=npm)](https://www.npmjs.com/package/ajiro-api) [![crates.io](https://img.shields.io/crates/v/ajiro-api)](https://crates.io/crates/ajiro-api) [![CI](https://github.com/niti-lab/ajiro-api/actions/workflows/ci.yml/badge.svg)](https://github.com/niti-lab/ajiro-api/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[English](README.md) | 日本語

Ajiro API（`ajiro-api`）は、バックエンドのルーティングを静的解析してエンドポイントごとのYAMLに書き出し、そのYAMLを回帰テストとして実行し、モックとしても配信するCLIです。Claude CodeやCodexが「実装 → scan → test → 修正 → 再テスト」を人手なしで回せるように作っています。

![デモ：scan と test が通る → ルートの移動で GET /todos/stats が 404 になる → 修正して 34 件すべて通る](docs/demo/demo.gif)

`--json` を付けると、同じ失敗がリクエスト・レスポンス本文・ハンドラの位置付きで出力されるので、エージェントはそのまま直して再実行できます。Claude Codeでは次の2行で入ります。

```text
/plugin marketplace add niti-lab/ajiro-api
/plugin install ajiro-api@ajiro-api
```

対応フレームワーク：Next.js Route Handlers、Hono、FastAPI、Express、Go（net/http、chi、Echo）。

- ローカルファースト。API定義もテスト結果も外部に送りません
- 保存形式は標準的なYAML。エンドポイント1つにつき1ファイルなので、Gitの競合が少なく、AIが編集する範囲も狭くなります
- 単一バイナリ（Rust）。実行時にNodeやPythonは不要です
- MITライセンス。CLI・スキル・テスト実行はすべて無料です

Ajiro（WBS・タスク管理）の姉妹製品です。

## なぜ作ったか

- AIにAPIを実装させると、全エンドポイントの疎通確認と「壊れていないか」の確認が人手のまま残ります。
- 既存のHTTPクライアントは、人間がGUIでリクエストを送る前提です。AIエージェントが読み書きする前提ではありません。
- 受託開発では、API仕様とサンプルレスポンスをフロント担当やクライアントへ渡す作業が必ず発生します。

## 30秒で試す

### インストール

```bash
# npm（インストール不要で実行）
npx -y ajiro-api --version

# Homebrew
brew install niti-lab/tap/ajiro-api

# cargo
cargo install ajiro-api

# GitHub Releases（macOS / Linux / Windows のバイナリ）
# https://github.com/niti-lab/ajiro-api/releases
```

以降のコマンドは、`ajiro-api` が使えない場合は `npx -y ajiro-api` に読み替えてください。

### 使う

```bash
cd your-backend-project
ajiro-api init            # ajiro-api.yaml と api/ を作る（フレームワークは自動判定）
ajiro-api scan            # ルーティングを読み取り、api/ にYAMLを生成
# アプリを起動してから
ajiro-api test            # 全エンドポイントを実行して検証
```

`tests:` が空のエンドポイントには、ステータスが500未満であることを確認する `smoke` テストが自動で適用されます（4xx は成功扱いですが警告を出します。401 は未ログインのことが多いため）。データを変えうる POST・PUT・PATCH・DELETE には送らず、スキップします（送るときは `ajiro-api test --smoke-unsafe`）。ログインが必要なAPIは、環境の `setup` にログインのエンドポイントを書くと最初に実行され、`capture` したトークンやCookieを環境の `headers` で全リクエストに付けられます（[YAMLリファレンス](docs/yaml-reference.md)）。

## YAMLの例

エンドポイント1つにつきYAMLファイル1つで、`api/<リソース>/<メソッド>-<パス>.yaml` に置かれます。`source` は `scan` が書き、`tests` と `mock` は人とAIが書きます。`scan` は `source` 以外を書き換えません。

```yaml
# api/users/post-users.yaml
version: 1
id: users.create
request:
  method: POST
  path: /users
  headers:
    Authorization: Bearer {{ auth.token }}
  body:
    name: Taro
    email: taro@example.com
source:            # scan が自動で更新する欄
  framework: hono
  file: src/routes/users.ts
  line: 42
tests:             # 人とAIが書く欄（scan は触らない）
  - name: 正常に作成できる
    expect:
      status: 201
      body:
        schema: ./schemas/user.json
        match:
          $.email: taro@example.com
      ignore: [$.id, $.createdAt]
    capture:
      userId: $.id
  - name: メール重複は409
    request:
      body: { email: dup@example.com }
    expect:
      status: 409
mock:              # ajiro-api mock で使う
  default: { status: 201, body: { id: usr_001, name: Taro } }
  scenarios:
    conflict: { status: 409 }
```

書き方の詳細は [docs/yaml-reference.md](docs/yaml-reference.md)、JSON Schemaは `ajiro-api schema endpoint` で取得できます（エディタの補完にも使えます）。

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/endpoint.schema.json
```

## Claude Codeで使う

プラグインを入れると、スキル（実装後に scan → test → 修正 → 再実行を回す手順）とMCPサーバーが使えるようになります。

```text
/plugin marketplace add niti-lab/ajiro-api
/plugin install ajiro-api@ajiro-api
```

スキルは、エージェントがHTTP APIのエンドポイントを実装・変更したときや、APIのテスト・検証を頼まれたときに働きます。手順の内容は [skills/ajiro-api/SKILL.md](skills/ajiro-api/SKILL.md) にあります。

MCPサーバーだけを使う場合は、`.mcp.json` に次を追加します。

```json
{
  "mcpServers": {
    "ajiro-api": { "command": "npx", "args": ["-y", "ajiro-api", "mcp"] }
  }
}
```

公開しているツールは `scan`、`test`、`list`、`validate`、`get_endpoint`、`schema` です。

## Codexで使う

[docs/agents/AGENTS.snippet.md](docs/agents/AGENTS.snippet.md) の内容を、プロジェクトの `AGENTS.md` に貼り付けてください。Claude Codeスキルと同等の手順書です。

## コマンド一覧

| コマンド | 内容 |
| --- | --- |
| `ajiro-api init` | `ajiro-api.yaml` と `api/` を作る。フレームワークを自動判定 |
| `ajiro-api scan` | ルーティングを静的解析してYAMLを生成・更新。`--check` でCI向けの差分検出（`--ignore-lines` で行ずれだけの変更は無視）、`--dry-run` で書き込まず確認 |
| `ajiro-api test` | YAMLを実行して検証。`--json`、`--junit <FILE>`、`-u`（スナップショット更新）、`--env`、`--bail` など |
| `ajiro-api list` | 定義済みエンドポイントの一覧 |
| `ajiro-api validate` | YAMLの構文・スキーマ・参照（重複id、`depends_on`）を検証 |
| `ajiro-api mock` | `mock:` のサンプルレスポンスを返すサーバーを起動（既定ポート4010） |
| `ajiro-api mcp` | MCPサーバー（stdio）を起動 |
| `ajiro-api openapi import / export` | OpenAPI 3.x との相互変換 |
| `ajiro-api import postman / bruno` | PostmanコレクションやBrunoフォルダをYAMLへ変換 |
| `ajiro-api docs` | 単一HTMLのAPI仕様書を生成 |
| `ajiro-api schema [endpoint\|config]` | JSON Schemaを出力 |
| `ajiro-api telemetry [on\|off\|status]` | 利用統計の同意設定（初期状態はオフ） |

全フラグは [docs/cli-reference.md](docs/cli-reference.md) を参照してください。終了コードは、`0` 成功、`1` テスト失敗・差分あり・検証エラー、`2` ツール自体が実行できなかった（設定・I/Oエラー）です。

`--json` の出力は、失敗箇所・期待値・実際の値・該当するソースの位置を含み、エージェントがそのまま修正に使える形です。トークンやパスワードは `***` で伏せられます。

## 対応フレームワーク

| フレームワーク | `framework` の値 |
| --- | --- |
| Next.js Route Handlers | `nextjs` |
| Hono | `hono` |
| FastAPI | `fastapi` |
| Express | `express` |
| Go net/http | `go-nethttp` |
| Go chi | `go-chi` |
| Go Echo | `go-echo` |

GraphQL、gRPC、WebSocketは対象外です。

## CIで使う

```bash
ajiro-api scan --check                          # YAMLがコードと食い違っていたら終了コード1
ajiro-api scan --check --ignore-lines           # ただしルートの行がずれただけなら0
ajiro-api test --env ci --junit report.xml      # JUnit XMLも出力
```

GitHub Actionsのサンプルは [examples/github-actions/ajiro-api.yml](examples/github-actions/ajiro-api.yml) にあります（インストール、アプリのバックグラウンド起動、ポート待ち、scan --check、test、JUnit公開、成果物アップロード）。

## OpenAPI・Postman・Brunoとの連携

```bash
ajiro-api openapi import openapi.yaml           # OpenAPI 3.x → YAML（tests は保持）
ajiro-api openapi export -o openapi.yaml        # YAML → OpenAPI 3.1
ajiro-api import postman collection.json        # Postman Collection v2.1
ajiro-api import bruno ./bruno-collection       # Bruno の .bru フォルダ
```

既存ファイルの `request` と `mock` を上書きしたい場合は `--overwrite` を付けます。`tests` は常に保持されます。

## モック

```bash
ajiro-api mock                      # http://127.0.0.1:4010
ajiro-api mock --scenario conflict  # 起動時にシナリオを指定
```

リクエストごとに `X-Ajiro-Scenario` ヘッダーまたは `?__scenario=` クエリでシナリオを切り替えられます。シナリオが無いエンドポイントは `default` を返します。`--delay` で全レスポンスに遅延を足せます。

## HTML仕様書

```bash
ajiro-api docs -o api-docs.html --title "My API"
```

ブラウザだけで開ける1ファイルです。Swaggerの環境を用意できない相手にもそのまま渡せます。

## 他のツールとの比較

差別化の軸は、AIエージェントが主な利用者であること、独自形式ではない標準YAMLであること、人数課金をしないことです。料金は2026年11月時点の公式ページによる情報です（年払い表示）。

| 製品 | 無料でできること | チーム利用の料金 | 保存形式 |
| --- | --- | --- | --- |
| [Postman](https://www.postman.com/pricing/) | 1ユーザーのみ。チーム作成不可（2026年3月に変更） | Team $19/人・月 | クラウド中心、Collection JSON |
| [Bruno](https://usebruno.com/pricing) | OSS版。公開Gitプロバイダーとの連携 | Pro $6・Ultimate $11/人・月 | 独自形式 `.bru` |
| **Ajiro API** | CLI・スキル・テスト実行はすべて無料（MIT） | CLIは無料 | 標準YAML（OpenAPIと相互変換） |

Postman・Brunoはどちらも優れたツールで、人間がGUIでリクエストを組み立てる用途には向いています。Ajiro APIはその用途では競いません。コードから生成したYAMLをAIが読み書きし、CIで回し続けることに絞っています。移行用に `ajiro-api import postman` と `ajiro-api import bruno` があります。

## プライバシー

- ローカルファースト。API定義やテスト結果をサーバーへ送ることはありません。
- 利用統計は初期状態でオフです。`ajiro-api telemetry on` で同意した場合のみ、匿名のコマンド実行回数（コマンド名・バージョン・OS）を扱います。パス、URL、API定義は含みません。`DO_NOT_TRACK=1` で常に無効になります。
- 秘密情報は `.env` 参照（`{{ env.NAME }}`）のみとし、YAMLには書きません。出力ログではトークンやパスワードを `***` で伏せます。

## ロードマップ

- **Phase 1（現在）**: OSS CLI、Claude Codeスキル、Codex向け手順書、MCPサーバー
- **Phase 2**: 有料デスクトップGUI（モックの本格版、テスト結果ビューなど）。公開後のKPIを見て着手するかどうかを判断します。日程は約束していません。
- **Phase 3**: Ajiro（WBS・タスク管理）とのタスク連携

CLI、スキル、MCP、インポーターはPhase 2以降もMITで無料です。

## 開発に参加する

[CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。Issueでは「GUI・モック・仕様書を求める声」のラベルも受け付けています。セキュリティ上の報告は [SECURITY.md](SECURITY.md) に従ってください。変更履歴は [CHANGELOG.md](CHANGELOG.md) です。

## ライセンス

[MIT](LICENSE) © 2026 NITI
