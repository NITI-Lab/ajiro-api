# Ajiro API 要件定義書（公開版）

2026-11-18 更新

この文書は、Ajiro API（`ajiro-api`）が何を解決し、何を作り、何を作らないかをまとめたものです。コマンドとYAMLの詳しい仕様は [cli-reference.md](cli-reference.md) と [yaml-reference.md](yaml-reference.md) にあります。

## 1. 概要・背景・目的

Ajiro API は、Claude Code / Codex がバックエンドのコードからAPI定義・回帰テスト・モックを作り、チーム内とクライアントへ仕様を共有するためのローカルファーストなツールです。CLI・Claude Codeスキル・MCPサーバーはMITライセンスの無料OSSとして提供します。

**背景（解決する痛み）**

- AIにAPIを実装させると、全エンドポイントの疎通確認と、壊れていないかの確認が人手のまま残る。
- 受託では、バックエンド担当がフロント担当（外注を含む）やクライアントにAPI仕様とサンプルレスポンスを渡す作業が必ず発生する。
- 既存のHTTPクライアントは「人間がGUIでリクエストを送る」前提で作られていて、AIエージェントが読み書きする前提になっていない。

**ポジショニング**

「AIコーディングエージェントのための、API検証と仕様共有の基盤」と位置付けます。人間が手を動かすのは、確認・共有・修正のときだけにします。

**Ajiroシリーズでの位置付け**

Ajiro（WBS・タスク管理）の姉妹製品です。将来は、タスクID（例: TSK-012）とテスト結果・モックを結び付けることを考えています。

## 2. 対象ユーザーと利用シーン

主な対象は、Claude CodeやCodexで日常的に実装している、少人数の開発チーム（受託を含む）です。

| ペルソナ | 役割 | 今の困りごと | 使う場面 |
| --- | --- | --- | --- |
| A. バックエンド担当 | AIでAPIを実装する | AIが書いたAPIの動作確認と、修正後の度重なる回帰確認が手作業 | 実装直後にエージェントへテスト生成と実行を任せる。CIでも回す |
| B. フロント担当（外注含む） | 画面を実装する | APIの完成待ち。仕様が口頭やチャットで飛んでくる | モックサーバーにつないで先行開発する |
| C. クライアント・協力会社 | 仕様を受け取る | Swaggerの環境を用意できない。Excelの仕様書は古くなる | 送られてきたHTML仕様書をブラウザで開くだけ |

**代表的な利用の流れ**

1. バックエンド担当がClaude Codeに新しいエンドポイントを実装させる。
2. スキルに従って、エージェントが `ajiro-api scan` でルーティングを読み取り、YAML定義を生成・更新する。
3. `ajiro-api test` で全エンドポイントを叩き、失敗があればエージェントが自分で直して再実行する。
4. 生成されたYAMLをコードと一緒にGitへコミットし、CIでも同じテストを回す。

## 3. 差別化と、狙わないこと

差別化の軸は三つです。AIエージェントが主な利用者であること、独自形式ではない標準YAMLであること、人数課金をしないことです。他製品との比較は [README](../README.ja.md#他のツールとの比較) にまとめています。

**狙わないこと**

- 手動でリクエストを組み立てる使い勝手では、PostmanやBrunoなどのGUIクライアントと競わない。
- GraphQL、gRPC、WebSocketは対象外とする。

## 4. 機能要件

完了条件は、「Claude CodeがサンプルリポジトリのAPIを実装してから、YAML生成・全件テスト・失敗の自己修正までを人手なしで完走すること」です。

| ID | 機能 | 内容 | 状態 |
| --- | --- | --- | --- |
| F1-01 | `init` | 設定ファイル `ajiro-api.yaml` と `api/` ディレクトリを作る。フレームワークを自動判定する | 提供中 |
| F1-02 | `scan` | ルーティングを静的解析し、エンドポイント一覧（メソッド・パス・パラメータ）をYAMLに書き出す。手書き部分は上書きしない | 提供中 |
| F1-03 | 対応フレームワーク | Next.js Route Handlers、Hono、FastAPI、Express、Go（net/http・chi・Echo） | 提供中 |
| F1-04 | `test` | YAMLのリクエストを順に実行し、ステータス・ヘッダ・ボディを検証する。終了コードで成否を返す | 提供中 |
| F1-05 | 検証方法 | 完全一致、JSON Schema、JSONPathでの部分一致、スナップショット比較（IDや日時など変わる項目は除外指定） | 提供中 |
| F1-06 | 変数と環境 | `local`・`staging` などの環境切り替え。秘密情報は `.env` 参照でYAMLに書かない | 提供中 |
| F1-07 | リクエストの連鎖 | ログインで得たトークンを後続に渡すなど、前のレスポンスの値を使う | 提供中 |
| F1-08 | 機械向け出力 | `--json` で失敗箇所、期待値、実際の値、該当するソースの位置を返す。エージェントがそのまま修正に使える形にする | 提供中 |
| F1-09 | Claude Codeスキル | `SKILL.md`。実装後に scan、test、修正、再実行の順で回す手順と、YAMLの書き方を含む | 提供中 |
| F1-10 | Codex対応 | `AGENTS.md` に貼れる同等の手順書 | 提供中 |
| F1-11 | MCPサーバー | scan・test・list などをMCPツールとして公開する | 提供中 |
| F1-12 | CI向け出力 | JUnit XMLとGitHub Actionsのサンプルワークフロー | 提供中 |
| F1-13 | OpenAPI変換 | OpenAPI 3.xとの双方向変換（import と export） | 提供中 |
| F1-14 | モック | `mock` でYAMLのサンプルレスポンスを返すサーバーを立てる。シナリオ切り替えと遅延の指定ができる | 提供中 |
| F1-15 | 移行インポーター | Postman Collection v2.1とBrunoの `.bru` フォルダを標準YAMLに変換 | 提供中 |
| F1-16 | HTML仕様書 | ブラウザだけで開ける1ファイルのAPIリファレンスを出力 | 提供中 |

**将来の検討事項**

デスクトップGUI（YAMLの画面編集、テスト結果の履歴表示、本格的なモックサーバーなど）とAjiroとのタスク連携を、有料の別製品として検討しています。公開後の利用状況を見て判断するので、日程は決めていません。ここに挙げたCLIの機能は、その後もMITライセンスで無料のまま提供します。

## 5. YAMLスキーマ

エンドポイント1つにつきYAMLファイル1つとし、`api/<リソース>/<メソッド>-<パス>.yaml` に置きます。ファイルを小さく分けるのは、Gitの競合を減らし、AIが編集する範囲を狭くするためです。`source` はscanが書き、`tests` と `mock` は人とAIが書く欄として分けます。

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

- スキーマ自体をJSON Schemaで公開し、エディタの補完とAIの構文ミス検出に使う（`schema/`）。
- `version` でスキーマの互換性を管理する。破壊的な変更には移行コマンドを用意する。

## 6. 非機能要件・技術構成

サーバーにAPI定義やテスト結果は一切置きません。

| 項目 | 方針 |
| --- | --- |
| 実装言語 | Rust（単一バイナリ。実行時のNodeやPythonを要求しない） |
| ルーティング解析 | tree-sitterによる静的解析（TypeScript・Python・Go）。フレームワークごとに解析ルールを分ける |
| 配布 | GitHub Releases（macOS・Windows・Linux）、npm（`npx ajiro-api`）、Homebrew、cargo |
| 性能目標 | 100エンドポイントのscanを3秒以内。起動は0.1秒以内 |
| セキュリティ | 秘密情報は `.env` 参照のみ。出力ログではトークンやパスワードを伏せ字にする |
| 利用統計 | 初期状態はオフ。同意した場合のみ、匿名のコマンド実行回数を記録する |
| テスト | 対応フレームワークごとにサンプルプロジェクトを用意し、scan結果のスナップショットで回帰を防ぐ |

## 7. ライセンス

CLI、Claude Codeスキル、MCPサーバー、インポーター、テスト実行はMITライセンスで、すべて無料です。将来有料の製品を出す場合も、人数ごとの課金はしません。
