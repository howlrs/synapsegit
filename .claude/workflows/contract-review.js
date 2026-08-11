export const meta = {
  name: 'contract-review',
  description: 'SynapseGit-specific contract review: frozen areas, layering, error-code contract, EN/JA doc obligations, security boundary language',
  whenToUse: 'Review a change against repository-specific invariants before opening or merging a PR. Args: a PR number, a branch name, or empty for the working tree vs origin/main. Generic bug-hunting is better served by /code-review.',
  phases: [
    { title: 'Resolve', detail: 'determine review target and diff' },
    { title: 'Review', detail: '6 contract dimensions in parallel' },
    { title: 'Verify', detail: 'adversarial verification per dimension' },
  ],
}

const GUARDRAILS = `
## SynapseGit 共通ガードレール (必ず遵守)
- リポジトリ root はカレントワーキングディレクトリ (SynapseGit checkout)。read-only 作業: working tree・ブランチ・コミットを変更しない (git fetch は可)。
- 外向きアクションの実行禁止: git push / gh issue・pr create / gh pr merge / GitHub 設定変更。
- 出力・レポートは日本語。根拠は「ファイル:行」と規範文書 (spec/, CONTRIBUTING.md, docs/) の該当箇所で示す。
`

const CTX_SCHEMA = {
  type: 'object',
  required: ['description', 'diff_command', 'changed_files'],
  properties: {
    description: { type: 'string', description: 'レビュー対象の説明 (PR番号/ブランチ/working tree、base と head)' },
    diff_command: { type: 'string', description: 'レビュアーが差分を得るために実行すべきコマンド (例: git diff origin/main...HEAD / gh pr diff 42)' },
    changed_files: { type: 'array', items: { type: 'string' } },
    untracked_files: { type: 'array', items: { type: 'string' }, description: 'working tree レビュー時の未追跡ファイル' },
    stats: { type: 'string', description: 'diff --stat 要約' },
  },
}

const FINDINGS_SCHEMA = {
  type: 'object',
  required: ['findings'],
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['severity', 'file', 'claim', 'evidence'],
        properties: {
          severity: { type: 'string', enum: ['high', 'medium', 'low'] },
          file: { type: 'string' },
          line: { type: 'integer' },
          claim: { type: 'string', description: '何が契約に違反する/リスクか (1-2文)' },
          evidence: { type: 'string', description: '差分の該当箇所と、根拠となる規範文書の該当箇所' },
        },
      },
    },
  },
}

const VERDICTS_SCHEMA = {
  type: 'object',
  required: ['verdicts'],
  properties: {
    verdicts: {
      type: 'array',
      items: {
        type: 'object',
        required: ['index', 'verdict', 'reason'],
        properties: {
          index: { type: 'integer', description: '検証対象 finding の 0-based index' },
          verdict: { type: 'string', enum: ['confirmed', 'refuted'] },
          reason: { type: 'string' },
        },
      },
    },
  },
}

const DIMENSIONS = [
  {
    key: 'correctness',
    focus: '変更コードの正しさ: ロジック誤り、境界条件、エラー処理の欠落、テストの妥当性と不足。テストが実装の自己申告に依存していないか (循環検証)。',
  },
  {
    key: 'frozen-invariants',
    focus: '凍結領域と不変条件: 凍結パス (docs/evaluation/publication-comprehension/v1/, spec/application/generic-artifact/v1/, spec/application/generic-artifact-publication/v1/, docs/releases/v*.md, CHANGELOG.md 過去セクション) への変更混入。OID・canonical bytes・schema・fixture・Ref・archive 契約への影響 — spec/core/v0.1 が normative であり、spec/fixture を伴わない code 先行変更は CONTRIBUTING 違反。fixture 変更時の golden.json / JS・Rust 両実装一致。',
  },
  {
    key: 'layering',
    focus: '依存方向の不変条件: レイヤ逆転の導入。synapse-local-http は synapse-local-service のみに依存 (synapse-core / synapse-sqlite の直接 import 禁止)。synapse-canonical は DB・CLI・media adapter に非依存。synapse-projection は正本・authorization に使わない使い捨て index。synapse-artifact-journal は workspace 内依存ゼロの独立プリミティブ。Cargo.toml への依存辺追加はアーキテクチャ宣言 (docs/runtime_architecture.md, docs/localhost_application_architecture.md, CONTRIBUTING.md workspace map) と同一変更で整合しているか。',
  },
  {
    key: 'error-contract',
    focus: 'エラー契約: 安定契約は machine-readable error code (spec/core/v0.1/operations.md の stable semantic error codes) であり Rust enum の形状ではない。既存 error code の変更・削除・意味変更。public な「error enum」への #[non_exhaustive] の新規付与は禁止 — error enum の既存例外は synapse-canonical::ErrorCode と synapse-publication::GenericArtifactPublicationError の 2 型のみ (OutputTarget / PublicationVisibility / ValueOrigin など非 error enum の既存 #[non_exhaustive] はこの政策の対象外なので誤検出しない)。variant 追加はソース互換破壊として PR 本文で明示されるべき。',
  },
  {
    key: 'docs-sync',
    focus: 'ドキュメント連動義務: capability に影響する変更で docs/README.md capability table / docs/project_status.md / README.md / README.ja.md の 4 文書同時更新があるか。英日ペア (README, docs/tutorial/README) の片側だけの変更。api/local/v1/openapi.json と docs/localhost_application_architecture.md の食い違い (食い違えば実装停止し同一変更で両修正が規律)。CONTRIBUTING.md の変更別 checklist (OID/schema/store/CLI/generic-artifact/observation/docs) の該当項目の充足。依存変更時の THIRD_PARTY_NOTICES.md 再生成。',
  },
  {
    key: 'security-language',
    focus: 'セキュリティ境界と言語規律: 127.0.0.1 (loopback) 限定境界を緩める実装・記述 (reverse proxy 公開の推奨等)。能力の誇張 (「真正性証明」「AI 検証」「著作権証明」等、byte identity と記録の検証可能性を超える主張)。secret・credential・実在個人情報・実画像・不要な絶対パスの混入。明示確認なしの外部送信の導入。設計正本 Issue #17 の安全境界 (default-deny、visibility=private_review 既定) との整合。',
  },
]

const target = args ? String(args) : ''

phase('Resolve')
const ctx = await agent(
  `レビュー対象の解決 (read-only)。git fetch origin を先に実行する。指定: "${target}"
- 指定が数字 (例: 42 / #42) → PR レビュー: gh pr view <n> --json title,headRefName,baseRefName,url で概要、diff_command は "gh pr diff <n>"。changed_files は gh pr diff <n> --name-only で。
- 指定がブランチ名 → diff_command は "git diff origin/main...<branch>"。
- 指定が空 → working tree レビュー: diff_command は "git diff origin/main" (コミット済み+未コミットの tracked 変更を含む)。git status --porcelain で未追跡ファイルも untracked_files として列挙する。
差分が空なら changed_files を空にして返す。` + GUARDRAILS,
  { label: 'resolve', schema: CTX_SCHEMA, model: 'sonnet', effort: 'low' }
)
if (!ctx || !ctx.changed_files || (ctx.changed_files.length === 0 && !(ctx.untracked_files || []).length)) {
  return { message: 'レビュー対象の差分がありません。', context: ctx }
}
log('対象: ' + ctx.description + ' (' + ctx.changed_files.length + ' files)')

const reviewed = await pipeline(
  DIMENSIONS,
  (_first, d) => agent(
    `あなたは SynapseGit 専属のコントラクトレビュアー。次の 1 次元だけに集中して差分をレビューする。
## 次元: ${d.key}
${d.focus}
## 対象
${ctx.description}
差分取得コマンド: ${ctx.diff_command}
変更ファイル: ${ctx.changed_files.join(', ')}${(ctx.untracked_files || []).length ? '\n未追跡ファイル: ' + ctx.untracked_files.join(', ') : ''}
差分を読み、必要に応じて周辺コードと規範文書 (spec/, CONTRIBUTING.md, docs/) を読んで、この次元の findings のみを返す。該当なしなら空配列。推測ではなく差分に実在する根拠を evidence に書く。` + GUARDRAILS,
    { label: 'review:' + d.key, phase: 'Review', schema: FINDINGS_SCHEMA }
  ),
  (rev, d) => {
    const fs = (rev && rev.findings) || []
    if (fs.length === 0) return { dim: d.key, confirmed: [], refuted: [] }
    return agent(
      `あなたは敵対的検証者。以下の findings (次元: ${d.key}) を 1 件ずつ「反証」を試みる。差分 (${ctx.diff_command}) と規範文書を自分で読み直し、finding の根拠が差分に実在し規律違反として成立するかを判定する。不確かな finding・規範文書の裏付けが取れない finding は refuted に倒す。
## findings
${JSON.stringify(fs, null, 2)}` + GUARDRAILS,
      { label: 'verify:' + d.key, phase: 'Verify', schema: VERDICTS_SCHEMA }
    ).then(v => {
      const verdicts = (v && v.verdicts) || []
      const confirmed = []
      const refuted = []
      fs.forEach((f, i) => {
        const verdict = verdicts.find(x => x.index === i)
        if (verdict && verdict.verdict === 'confirmed') confirmed.push(Object.assign({}, f, { dim: d.key, verify_reason: verdict.reason }))
        else refuted.push(Object.assign({}, f, { dim: d.key, verify_reason: verdict ? verdict.reason : 'verdict なし (安全側で棄却)' }))
      })
      return { dim: d.key, confirmed, refuted }
    })
  }
)

const order = { high: 0, medium: 1, low: 2 }
const dims = reviewed.filter(Boolean)
const confirmed = dims.flatMap(r => r.confirmed).sort((a, b) => (order[a.severity] !== undefined ? order[a.severity] : 9) - (order[b.severity] !== undefined ? order[b.severity] : 9))
const refutedCount = dims.reduce((n, r) => n + r.refuted.length, 0)
log('confirmed ' + confirmed.length + ' 件 / refuted ' + refutedCount + ' 件')

return {
  target: ctx.description,
  confirmed_findings: confirmed,
  refuted_count: refutedCount,
  refuted: dims.flatMap(r => r.refuted),
  note: 'confirmed_findings は敵対的検証を通過したもののみ。汎用的なバグ探索は /code-review を併用してください。',
}
