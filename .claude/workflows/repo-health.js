export const meta = {
  name: 'repo-health',
  description: 'Read-only health audit: git/GitHub sync, docs freshness, EN/JA parity, doc executability, architecture-map drift, contract scripts',
  whenToUse: 'Run periodically or before planning maintenance work on SynapseGit. Pass "full" as args to also run the full CI-parity verification suite locally (slow).',
  phases: [
    { title: 'Audit', detail: '6 parallel read-only inspectors' },
    { title: 'Full verify', detail: 'optional CI-parity suite (args: "full")' },
  ],
}

const GUARDRAILS = `
## SynapseGit 共通ガードレール (必ず遵守)
- リポジトリ root はカレントワーキングディレクトリ (SynapseGit checkout)。
- cargo は必ず "cargo +1.88.0" で呼ぶ (CI は RUST_TOOLCHAIN=1.88.0 固定。default toolchain は使わない)。cargo を内部で呼ぶ Node スクリプトには環境変数 RUSTUP_TOOLCHAIN=1.88.0 を付ける。
- cargo でビルドが走る場合は CARGO_TARGET_DIR を自分専用のパスに分離する。
- 凍結領域 (編集禁止): docs/evaluation/publication-comprehension/v1/ 配下、spec/application/generic-artifact/v1/ と spec/application/generic-artifact-publication/v1/、docs/releases/v*.md、CHANGELOG.md の過去バージョンセクション、公開済み v* タグ。
- 外向きアクションの実行禁止: git push / git tag / gh issue create / gh pr create / gh pr merge / GitHub 設定変更 (node scripts/manage_github_security.mjs --apply を含む) / リリース公開 / 外部サービスへの送信。必要な場合は実行すべきコマンドをレポートに記載して人間に委ねる。
- 配布物を作らない: crates.io / GHCR / Docker image / hosted デモの公開・作成は Stage 0 とライセンス (source-available, 非 OSI) の制約により禁止。
- 公開向け文章の下書きに credential・実在個人情報・実画像・不要な絶対パスを含めない。
- 出力・レポートは日本語。根拠は「ファイル:行」やコマンド出力で示す。
`

const FINDINGS_SCHEMA = {
  type: 'object',
  required: ['findings'],
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['severity', 'title', 'detail', 'suggested_fix'],
        properties: {
          severity: { type: 'string', enum: ['high', 'medium', 'low', 'info'] },
          title: { type: 'string', description: '1行の問題表題' },
          detail: { type: 'string', description: '根拠 (ファイル:行、コマンド出力の要約)' },
          suggested_fix: { type: 'string', description: '推奨する修正・対応' },
        },
      },
    },
  },
}

const COMMON = 'あなたは SynapseGit リポジトリの read-only 監査担当。working tree とブランチ・コミットは一切変更しない (git fetch と /tmp への一時ファイル作成は可)。問題がなければ findings は空配列でよい。全て正常な場合は info 1件で「正常」を報告する。\n' + GUARDRAILS

const INSPECTORS = [
  {
    key: 'sync-github',
    prompt: COMMON + `
担当: リポジトリ同期と GitHub 状態の監査。
- git fetch origin を実行し、git status -sb と git log --oneline -3 で working tree の状態、ローカル main と origin/main の ahead/behind を確認 (乖離は high。過去に別 checkout との乖離事故あり)。
- gh run list --branch main --limit 3 で main の CI 状態を確認。
- gh pr list --state open --json number,title,author,headRefName で open PR を列挙。dependabot PR があれば gh pr checks <番号> で CI 結果と失敗理由を確認 (既知パターン: THIRD_PARTY_NOTICES.md stale → dependabot-notices ワークフローで修復可能、と suggested_fix に書く)。
- gh issue list --state open で open issue を列挙。
- gh release list --limit 3 の最新リリースと、CHANGELOG.md 先頭・SECURITY.md の Supported versions 表を突合し、版数不整合を検出 (過去実績: SECURITY.md が旧版のまま残存)。`,
  },
  {
    key: 'docs-freshness',
    prompt: COMMON + `
担当: ドキュメント鮮度の監査。
- 最新リリースタグ (git tag --sort=-v:refname を先頭から) を基準に、旧バージョン文字列の残存を全 Markdown から grep で検出 (過去実績: docs/tutorial/README.md, docs/tutorial/README.ja.md, deploy/local/README.md に旧版参照が残存)。凍結領域 (docs/releases/, docs/evaluation/publication-comprehension/v1/, CHANGELOG.md 過去セクション) と、履歴・互換性説明として妥当な言及は除外し、除外判断の根拠も書く。
- 「Last verified」ヘッダを持つ文書 (docs/distribution.md, docs/project_status.md, docs/install.md など) のヘッダ日付と git log -1 --format=%cs -- <file> を突合。ヘッダの無い主要文書 (docs/cli_reference.md, docs/localhost_application_architecture.md など) は最終コミット日を報告し、長期未更新なら low で指摘。
- api/local/v1/openapi.json の info.version と最新タグの対応を確認 (localhost 契約が変わっていない場合の据え置きは info 扱い)。`,
  },
  {
    key: 'enja-parity',
    prompt: COMMON + `
担当: 英日ドキュメント同期の監査。
- 対象ペア: README.md / README.ja.md、docs/tutorial/README.md / docs/tutorial/README.ja.md。
- 各ペアで見出し構造 (##/### の数・順序) の対応、段落・文の欠落、バージョン記述や能力境界 (boundary) 記述の乖離を検出する (過去実績: README.ja.md で EN の trust 前提文「The binding assumes untampered trusted local configuration...」が欠落)。
- capability 記述の 4 文書連動規律 (docs/README.md の capability table / docs/project_status.md / README.md / README.ja.md) の相互矛盾を確認。
- 英語 README から日本語主体の文書 (docs/quickstart.md, docs/cli_reference.md, docs/usage_guide.md など) へ直接リンクしている箇所は「英語話者導線の欠落」として low〜info で列挙。`,
  },
  {
    key: 'doc-exec',
    prompt: COMMON + `
担当: ドキュメント内コードブロックの実行可能性監査。
- 対象: README.md, README.ja.md, docs/quickstart.md, docs/install.md, docs/tutorial/README.md, docs/tutorial/README.ja.md, docs/usage_guide.md, docs/cli_reference.md。
- fenced の bash/sh コードブロックを抽出し、(a) /tmp の一時ファイルに書き出して bash -n で構文検査 (b) コピペを壊すエスケープ混入 — 特にリテラルのバックスラッシュ+引用符 \\" (過去実績: docs/quickstart.md セクション2に21行混入、CI の verify_docs.mjs はリンク検査のみで検出不能) (c) 参照するスクリプト・パスの実在、synapse のサブコマンド名が crates/synapse-cli/src/main.rs の実装と一致するか、を検査。
- 環境依存 (ダウンロード URL 等) で実行不能なのは構文が正しければ問題にしない。`,
  },
  {
    key: 'arch-map',
    prompt: COMMON + `
担当: アーキテクチャ宣言と実態の突合。
- cargo +1.88.0 metadata --no-deps --format-version 1 で 15 クレートの workspace 内 non-dev 依存グラフを抽出する (ビルドは走らない)。
- CONTRIBUTING.md の workspace map (mermaid) とクレート責務表、docs/runtime_architecture.md、docs/localhost_application_architecture.md の宣言と突合し、ノード欠落・辺欠落・余分な辺・方向逆転を検出 (過去実績: CONTRIBUTING.md に synapse-local-service / synapse-local-http が欠落、Publication→Artifact 辺が欠落)。
- 依存方向の不変条件を確認: synapse-local-http は synapse-local-service のみに依存 / synapse-canonical は DB・CLI・media に非依存 / synapse-projection は synapse-core 非依存の使い捨て index / synapse-artifact-journal は workspace 内依存ゼロ。違反は high。`,
  },
  {
    key: 'contract-scripts',
    prompt: COMMON + `
担当: 軽量契約検証の実行 (cargo ビルドなし)。以下を順に実行し、fail したものを findings 化 (fail の出力要約を detail に)。全て pass なら info 1件で報告。
- for s in scripts/*.mjs; do node --check "$s"; done
- for s in scripts/*.sh; do bash -n "$s"; done
- node scripts/verify_docs.mjs
- node scripts/verify_license.mjs
- RUSTUP_TOOLCHAIN=1.88.0 node scripts/verify_core_fixtures.mjs
- RUSTUP_TOOLCHAIN=1.88.0 node scripts/verify_local_api.mjs
- node scripts/test_publication_comprehension_scorer.mjs
- RUSTUP_TOOLCHAIN=1.88.0 node scripts/generate_third_party_notices.mjs --check
- node scripts/manage_github_security.mjs --validate
- git diff --check
- node scripts/verify_mermaid.mjs はネットワーク (npx) が必要なため任意: 実行できない場合は info で skip を報告。`,
  },
]

phase('Audit')
const results = await parallel(INSPECTORS.map(t => () =>
  agent(t.prompt, { label: 'audit:' + t.key, phase: 'Audit', schema: FINDINGS_SCHEMA, model: 'sonnet' })
    .then(r => ({ key: t.key, findings: (r && r.findings) || [] }))
))

let fullVerify = null
if (args === 'full' || (args && args.full === true)) {
  phase('Full verify')
  fullVerify = await agent(COMMON + `
担当: CI 同等検証スイートの一括実行 (時間がかかる)。リポジトリ root で以下を順に実行し、各ステップの pass/fail と失敗時の出力要約を表で報告する。cargo 系は CARGO_TARGET_DIR=/tmp/synapsegit-repo-health-target を付ける。途中で fail しても最後まで全ステップを実行する。
 1. cargo +1.88.0 fmt --all -- --check
 2. cargo +1.88.0 test --workspace --all-targets --locked
 3. cargo +1.88.0 test --workspace --doc --locked
 4. cargo +1.88.0 clippy --workspace --all-targets --all-features --locked -- -D warnings
 5. RUSTDOCFLAGS="-D warnings" cargo +1.88.0 doc --workspace --no-deps --locked
 6. for s in scripts/*.mjs; do node --check "$s"; done
 7. for s in scripts/*.sh; do bash -n "$s"; done
 8. RUSTUP_TOOLCHAIN=1.88.0 node scripts/verify_core_fixtures.mjs
 9. RUSTUP_TOOLCHAIN=1.88.0 node scripts/verify_local_api.mjs
10. node scripts/test_publication_comprehension_scorer.mjs
11. node scripts/verify_docs.mjs
12. node scripts/verify_license.mjs
13. RUSTUP_TOOLCHAIN=1.88.0 node scripts/generate_third_party_notices.mjs --check
14. node scripts/verify_mermaid.mjs (ネットワーク不可なら MERMAID_CLI=<ローカル mmdc> を試し、それも不可なら skip と明記)
15. node scripts/manage_github_security.mjs --validate
16. git diff --check`, { label: 'verify:ci-parity', phase: 'Full verify', model: 'sonnet', effort: 'low' })
}

const order = { high: 0, medium: 1, low: 2, info: 3 }
const findings = results.filter(Boolean)
  .flatMap(r => r.findings.map(f => Object.assign({ area: r.key }, f)))
  .sort((a, b) => (order[a.severity] !== undefined ? order[a.severity] : 9) - (order[b.severity] !== undefined ? order[b.severity] : 9))
log(findings.length + ' 件の finding を収集しました')

return { findings, full_verify: fullVerify }
