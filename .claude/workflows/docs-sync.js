export const meta = {
  name: 'docs-sync',
  description: 'Detect and fix documentation drift (stale versions, EN/JA divergence, broken code blocks) in the working tree, without committing',
  whenToUse: 'Run when repo-health reports documentation drift, or after a release. Requires a clean working tree; fixes stay uncommitted for review.',
  phases: [
    { title: 'Preflight', detail: 'clean-tree check' },
    { title: 'Find', detail: '3 parallel drift finders' },
    { title: 'Fix', detail: 'apply fixes to working tree' },
    { title: 'Verify', detail: 'doc verifiers + adversarial diff review' },
  ],
}

const GUARDRAILS = `
## SynapseGit 共通ガードレール (必ず遵守)
- リポジトリ root はカレントワーキングディレクトリ (SynapseGit checkout)。
- cargo は必ず "cargo +1.88.0" で呼ぶ。cargo を内部で呼ぶ Node スクリプトには RUSTUP_TOOLCHAIN=1.88.0 を付ける。
- 凍結領域 (編集禁止): docs/evaluation/publication-comprehension/v1/ 配下、spec/application/generic-artifact/v1/ と spec/application/generic-artifact-publication/v1/、docs/releases/v*.md、CHANGELOG.md の過去バージョンセクション、公開済み v* タグ。
- 外向きアクションの実行禁止: git push / git tag / gh issue create / gh pr create / gh pr merge / GitHub 設定変更 / リリース公開 / 外部サービスへの送信。
- 能力主張の言語規律: README 等の抑制的な boundary 記述 (byte identity のみ、AI モデル呼び出しなし、127.0.0.1 限定、作者性・真実の証明はしない等) は意図的な設計。誇張表現 (「真正性証明」「AI 検証」等) を導入しない。
- 英日ペア (README.md/README.ja.md、docs/tutorial/README.md/README.ja.md) は必ず両側を同時に修正し、片側だけの変更で乖離を拡大しない。
- 出力・レポートは日本語。根拠は「ファイル:行」で示す。
`

const PRE_SCHEMA = {
  type: 'object',
  required: ['clean', 'branch', 'notes'],
  properties: {
    clean: { type: 'boolean', description: 'git status --porcelain が空なら true' },
    branch: { type: 'string' },
    notes: { type: 'string', description: 'origin/main との関係 (ahead/behind) など' },
  },
}

const FIXLIST_SCHEMA = {
  type: 'object',
  required: ['fixes'],
  properties: {
    fixes: {
      type: 'array',
      items: {
        type: 'object',
        required: ['file', 'issue', 'action'],
        properties: {
          file: { type: 'string' },
          issue: { type: 'string', description: '何が乖離・破損しているか (根拠の行番号つき)' },
          action: { type: 'string', description: '適用すべき具体的修正' },
          pair_file: { type: 'string', description: '英日ペアの相方も直す必要がある場合のパス (不要なら空文字)' },
        },
      },
    },
  },
}

const APPLY_SCHEMA = {
  type: 'object',
  required: ['applied', 'skipped', 'diffstat'],
  properties: {
    applied: { type: 'array', items: { type: 'object', required: ['file', 'change'], properties: { file: { type: 'string' }, change: { type: 'string' } } } },
    skipped: { type: 'array', items: { type: 'object', required: ['file', 'reason'], properties: { file: { type: 'string' }, reason: { type: 'string' } } } },
    diffstat: { type: 'string', description: 'git diff --stat の出力' },
  },
}

const REVIEW_SCHEMA = {
  type: 'object',
  required: ['ok', 'problems'],
  properties: {
    ok: { type: 'boolean' },
    problems: { type: 'array', items: { type: 'string' } },
  },
}

const FINDER_COMMON = 'あなたは SynapseGit の文書乖離検出担当 (read-only、リポジトリは変更しない)。修正対象として確度の高いものだけを fixes に列挙する。判断が割れるもの・設計判断を要するものは fixes に入れず、issue の末尾に「要人間判断」と書いた項目として返す。\n' + GUARDRAILS

phase('Preflight')
const pre = await agent(
  'SynapseGit リポジトリの状態確認。git fetch origin を実行し、git status --porcelain (空=clean)、現在ブランチ、origin/main との ahead/behind を調べて返す。リポジトリは変更しない。' + GUARDRAILS,
  { label: 'preflight', schema: PRE_SCHEMA, model: 'sonnet', effort: 'low' }
)
if (!pre || !pre.clean) {
  return { aborted: true, reason: 'working tree が clean ではありません。docs-sync は自分の変更だけが diff に現れるよう clean tree を前提とします。', state: pre }
}

phase('Find')
const finders = await parallel([
  () => agent(FINDER_COMMON + `
担当: バージョン残存と鮮度。最新リリースタグ (git tag --sort=-v:refname の先頭) を基準に、旧バージョン文字列が残る Markdown を grep で洗い出し (凍結領域と履歴説明として妥当な言及は除外)、SECURITY.md の Supported versions 表の版数不整合も確認して、修正 fixes を返す。`, { label: 'find:versions', schema: FIXLIST_SCHEMA, model: 'sonnet' }),
  () => agent(FINDER_COMMON + `
担当: 英日パリティ。README.md/README.ja.md と docs/tutorial/README.md/README.ja.md の各ペアで、見出し構造・段落・boundary 記述の欠落や乖離を検出し、欠落側に文を補う fixes を返す (翻訳文面の具体案を action に含める。トーンは既存の抑制的文体に合わせる)。`, { label: 'find:enja', schema: FIXLIST_SCHEMA, model: 'sonnet' }),
  () => agent(FINDER_COMMON + `
担当: コードブロック破損。README.md, README.ja.md, docs/quickstart.md, docs/install.md, docs/tutorial/README*.md, docs/usage_guide.md, docs/cli_reference.md の bash/sh ブロックを抽出し、bash -n (/tmp の一時ファイル経由) とリテラル \\" 混入 (過去実績: docs/quickstart.md セクション2) を検査し、壊れたブロックを元の意図どおりコピペ可能な形に直す fixes を返す。`, { label: 'find:codeblocks', schema: FIXLIST_SCHEMA, model: 'sonnet' }),
])

const fixes = finders.filter(Boolean).flatMap(r => r.fixes || [])
if (fixes.length === 0) {
  return { fixes: [], message: '修正対象の文書乖離は見つかりませんでした。' }
}
log(fixes.length + ' 件の修正候補を検出。適用します')

phase('Fix')
const applied = await agent(
  'あなたは SynapseGit の文書修正担当。以下の fixes リストの項目「だけ」を working tree に適用する (コミットはしない)。issue に「要人間判断」とあるものは適用せず skipped に入れる。凍結領域に触れる項目も skipped。英日ペア指定 (pair_file) がある修正は必ず両側を同時に直す。修正後に git diff --stat を取得して返す。\n'
  + GUARDRAILS
  + '\n## fixes\n' + JSON.stringify(fixes, null, 2),
  { label: 'apply-fixes', schema: APPLY_SCHEMA, model: 'sonnet' }
)

phase('Verify')
const [verify, review] = await parallel([
  () => agent(
    'SynapseGit の文書検証を実行して結果を報告する (リポジトリは変更しない): node scripts/verify_docs.mjs / git diff --check / 可能なら node scripts/verify_mermaid.mjs (ネットワーク不可なら skip と明記) / git diff --stat。fail があれば出力を引用する。' + GUARDRAILS,
    { label: 'verify:scripts', model: 'sonnet', effort: 'low' }
  ),
  () => agent(
    'あなたは敵対的レビュアー。git diff (unstaged) を読み、SynapseGit の文書修正が規律に違反していないか検査する: (1) 凍結領域 (docs/evaluation/publication-comprehension/v1/, spec/application/*/v1, docs/releases/, CHANGELOG 過去セクション) への変更が混入していないか — git diff --name-only で確認 (2) 英日ペアの片側だけの変更で乖離が拡大していないか (3) 抑制的な boundary 記述のトーンを崩す誇張・能力主張の変化がないか (4) 意図外の変更 (無関係な行の書き換え) がないか。問題ゼロなら ok=true。' + GUARDRAILS,
    { label: 'review:adversarial', schema: REVIEW_SCHEMA }
  ),
])

return {
  fixes_found: fixes.length,
  applied: applied ? applied.applied : [],
  skipped: applied ? applied.skipped : [],
  diffstat: applied ? applied.diffstat : '',
  verification: verify,
  adversarial_review: review,
  next_steps: '変更は working tree に未コミットで残っています。内容を確認し、1 slice = 1 PR の規律に従い agent/docs-sync-<topic> ブランチでコミットして PR を作成してください (slice-pr ワークフローも利用可)。',
}
