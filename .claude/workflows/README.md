# Claude Code saved workflows for SynapseGit

このディレクトリは [Claude Code](https://code.claude.com/docs/en/workflows.md) の保存ワークフロー (マルチエージェント・オーケストレーション・スクリプト) を収めます。SynapseGit を「クリエイターとエンジニアに有益なツール」として整備・進化させ続けるための、リポジトリ固有の運用知識 (検証プロトコル・凍結領域・マージ規律) を機械実行可能な形で保存したものです。

各ワークフローは Claude Code セッション内で `/名前` (例: `/repo-health`) または Workflow ツール (`Workflow({name: "repo-health", args: ...})`) で起動します。

## 必要環境

- Claude Code v2.1.154 以上 (saved workflows 対応)
- rustup で Rust toolchain **1.88.0** がインストール済みであること (CI と同一 pin。スクリプトは常に `cargo +1.88.0` を使う)
- Node.js 18 以上、`gh` CLI (認証済み)、`git`
- `node scripts/verify_mermaid.mjs` はネットワーク (npx) を使うため、オフライン時は各ワークフローが skip を明記します

## ワークフロー一覧

| 名前 | 役割 | args | リポジトリへの影響 |
|---|---|---|---|
| `repo-health` | 健全性監査: git/GitHub 同期、docs 鮮度、英日パリティ、コードブロック実行可能性、アーキ宣言と実態の突合、契約スクリプト | 省略可。`"full"` または `{full:true}` で CI 同等スイートも実行 (重い) | なし (read-only) |
| `docs-sync` | 文書乖離の検出と修正適用 (バージョン残存・英日乖離・壊れた bash ブロック) | なし | working tree に未コミット変更 (clean tree 必須) |
| `dependabot-notices` | dependabot PR の THIRD_PARTY_NOTICES.md stale を worktree で修復しローカルコミット作成 | 省略可。`{check:true}` で cargo check も実行 | /tmp の worktree のみ。push はコマンド提示のみ |
| `slice-pr` | 1 slice = 1 PR の実装パイプライン: 計画→批判→実装→CI 同等検証→3 レンズ敵対的レビュー→PR 本文組立 | **必須**: ゴール記述 or Issue 番号 | /tmp の worktree に `agent/<slug>` ブランチ。push はしない |
| `contract-review` | リポジトリ固有契約のレビュー: 凍結領域・依存方向・error code 契約・docs 連動・境界言語 (敵対的検証つき) | 省略可: PR 番号 / ブランチ名。省略時は working tree vs origin/main | なし (read-only) |
| `eval-comprehension` | 凍結 v1 理解度評価プロトコルの AI トラック実行 (zero-context 評価者 + scorer) | 省略可。`{runs:N}` (既定 3) | なし (成果物は /tmp/synapsegit-eval/) |
| `release-prep` | docs/distribution.md の Release gate に沿ったリリース準備ブランチ作成と全量検証 | **必須**: `"vX.Y.Z"` | /tmp の worktree に `agent/release-vX-Y-Z` ブランチ。tag/push はしない |

## 共通の安全設計

すべてのワークフローは次を厳守します (各スクリプト内の GUARDRAILS 参照):

- **外向きアクションを実行しない**: `git push` / `git tag` / `gh issue create` / `gh pr create` / merge / リリース公開は行わず、人間が実行すべきコマンドをレポートに含めて停止します。
- **凍結領域に不可侵**: `docs/evaluation/publication-comprehension/v1/` (バイト不変)、`spec/application/generic-artifact/v1/`・`spec/application/generic-artifact-publication/v1/` (frozen contract)、`docs/releases/v*.md`、CHANGELOG の過去セクション、公開済み `v*` タグ。
- **toolchain pin**: cargo は常に `cargo +1.88.0`。cargo を呼ぶ Node スクリプトには `RUSTUP_TOOLCHAIN=1.88.0` を付与。並列実行時は `CARGO_TARGET_DIR` を分離。
- **リポジトリ本体の working tree を汚さない**: 実装系ワークフローは /tmp 配下の git worktree で作業します (docs-sync のみ、明示された役割として clean tree 上に未コミット修正を残します)。
- **配布チャネル制約**: Stage 0 とライセンス (source-available, 非 OSI) に従い、crates.io / GHCR / コンテナ / hosted デモの公開・作成は行いません。

## 運用の目安

- 日常: `repo-health` → 指摘に応じて `docs-sync` / `dependabot-notices` / `slice-pr`
- 変更前後: `contract-review` (リポジトリ固有契約) + 汎用レビューは `/code-review`
- 節目: `eval-comprehension` (公開バンドルの理解度エビデンス取得)、`release-prep` (リリース準備)
- `slice-pr` / `release-prep` / `repo-health full` はワークスペース全量のビルド・テストを含むため実行時間とエージェント消費が大きい点に留意してください。

> **注**: ワークフロー名のレジストリはセッション開始時に読み込まれます。ファイルを追加・改名した直後の同一セッションでは名前解決 (`/名前` や `Workflow({name})`) が効かないことがあり、その場合は `Workflow({scriptPath: ".claude/workflows/<名前>.js"})` で起動するか、新しいセッションで利用してください。

## 変更時の注意

- スクリプトの `meta` は純リテラル (変数・関数呼び出し不可)。`meta.name` はファイル名と一致させます。
- スクリプト内で `Date.now()` / `Math.random()` / 引数なし `new Date()` は使えません (resume 互換のため)。日付が必要な処理はエージェントにシェルの `date` で取得させます。
- 検証コマンド列は `.github/workflows/ci.yml` が正です。CI 側を変更した場合は各スクリプトの CI 同等ステップも追随させてください。
