export const meta = {
  name: 'slice-pr',
  description: 'Implement one slice (1 slice = 1 PR) in an isolated worktree: plan, critique, implement, CI-parity verify, multi-lens review, PR body draft. Never pushes',
  whenToUse: 'Pass a goal description or an issue number as args (e.g. "docs/quickstart.md セクション2の \\" 混入を修正する" or "#17 の sub-issue X"). Produces a verified local branch agent/<slug> plus a PR body draft; push and PR creation stay manual.',
  phases: [
    { title: 'Plan', detail: 'plan + adversarial critique' },
    { title: 'Implement', detail: 'worktree implementation with targeted tests' },
    { title: 'Verify', detail: 'full CI-parity suite' },
    { title: 'Review', detail: '3-lens adversarial review + one fix round' },
    { title: 'Finalize', detail: 'PR body draft and handoff' },
  ],
}

const GUARDRAILS = `
## SynapseGit 共通ガードレール (必ず遵守)
- リポジトリ root はカレントワーキングディレクトリ (SynapseGit checkout)。リポジトリ本体の working tree は変更しない — 実装は指定された /tmp 配下の git worktree 内のみで行う。
- cargo は必ず "cargo +1.88.0" で呼ぶ (CI は RUST_TOOLCHAIN=1.88.0 固定)。cargo を内部で呼ぶ Node スクリプトには RUSTUP_TOOLCHAIN=1.88.0 を付ける。CARGO_TARGET_DIR は指定された専用パスを使う。
- 凍結領域 (編集禁止): docs/evaluation/publication-comprehension/v1/ 配下、spec/application/generic-artifact/v1/ と generic-artifact-publication/v1/、docs/releases/v*.md、CHANGELOG.md の過去バージョンセクション、公開済み v* タグ。
- 外向きアクションの実行禁止: git push / git tag / gh issue create / gh pr create / gh pr merge / GitHub 設定変更 / リリース公開。worktree 内でのローカルコミットは可。
- 依存 (Cargo.toml / Cargo.lock) を変更したら同一変更内で RUSTUP_TOOLCHAIN=1.88.0 node scripts/generate_third_party_notices.mjs --write を実行する。
- capability に影響する変更は docs/README.md capability table / docs/project_status.md / README.md / README.ja.md を同一 slice で更新。英日ペア文書は必ず両側同時に更新。
- エラー契約: machine-readable error code が安定契約。public error enum への #[non_exhaustive] 新規付与禁止。variant 追加はソース互換破壊として PR 本文に明示。
- 能力主張の言語規律: 抑制的な boundary 記述を崩す誇張を導入しない。
- 出力・レポートは日本語。
`

const PLAN_SCHEMA = {
  type: 'object',
  required: ['slug', 'summary', 'checklists', 'steps', 'files', 'tests', 'risks'],
  properties: {
    slug: { type: 'string', description: 'ブランチ名 agent/<slug> に使う kebab-case の短い識別子' },
    summary: { type: 'string', description: 'この slice で何を達成するか (2-4文)' },
    checklists: { type: 'array', items: { type: 'string' }, description: 'CONTRIBUTING.md の変更別 checklist のうち該当する系統と、その義務項目' },
    steps: { type: 'array', items: { type: 'string' }, description: '実装手順' },
    files: { type: 'array', items: { type: 'string' }, description: '変更予定ファイル' },
    tests: { type: 'array', items: { type: 'string' }, description: '追加・更新するテストと、実行する検証コマンド' },
    docs_updates: { type: 'array', items: { type: 'string' }, description: '連動して更新すべき文書' },
    risks: { type: 'array', items: { type: 'string' } },
  },
}

const CRITIQUE_SCHEMA = {
  type: 'object',
  required: ['blockers', 'improvements'],
  properties: {
    blockers: { type: 'array', items: { type: 'string' }, description: '計画のまま進めてはならない致命的問題 (凍結領域侵害、スコープ過大で 1 slice に収まらない、規律違反)' },
    improvements: { type: 'array', items: { type: 'string' }, description: '採用すべき改善提案' },
  },
}

const IMPL_SCHEMA = {
  type: 'object',
  required: ['commits', 'diffstat', 'notes'],
  properties: {
    commits: { type: 'array', items: { type: 'string' } },
    diffstat: { type: 'string' },
    notes: { type: 'string', description: '計画からの逸脱・未解決点があれば正直に書く' },
  },
}

const VERIFY_SCHEMA = {
  type: 'object',
  required: ['all_passed', 'steps'],
  properties: {
    all_passed: { type: 'boolean' },
    steps: { type: 'array', items: { type: 'object', required: ['cmd', 'status'], properties: { cmd: { type: 'string' }, status: { type: 'string', enum: ['passed', 'failed', 'skipped'] }, note: { type: 'string' } } } },
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
          claim: { type: 'string' },
          evidence: { type: 'string' },
        },
      },
    },
  },
}

if (!args || String(args).trim() === '') {
  throw new Error('args にゴール記述または Issue 番号を渡してください。例: Workflow({name: "slice-pr", args: "docs/quickstart.md の壊れた bash ブロックを修正"})')
}
const goal = String(args).trim()

phase('Plan')
const plan = await agent(
  `あなたは SynapseGit の slice 計画担当 (read-only。リポジトリは変更しない)。
ゴール: ${goal}
- ゴールが Issue 番号 (#N / N) を含むなら gh issue view <N> --comments で内容を読む。
- 関連コード・spec・docs を読み、1 slice = 1 PR に収まる最小で完結した計画を立てる。収まらない場合は最初の 1 slice に絞り、残りは notes に分割案として書く。
- CONTRIBUTING.md の変更別 checklist (OID/canonicalization, Record schema/local semantic rule, ObjectStore/Ref/archive, CLI, Generic artifact application contract, Observation adapter, Documentation) から該当系統を特定し、その義務項目 (spec/fixture/docs の同時更新義務) を checklists に列挙する。
- 検証コマンドは cargo +1.88.0 の pin を含めて具体的に書く。` + GUARDRAILS,
  { label: 'plan', schema: PLAN_SCHEMA }
)
if (!plan) throw new Error('計画の生成に失敗しました')
const slug = (plan.slug || 'slice').toLowerCase().replace(/[^a-z0-9-]/g, '-').replace(/^-+|-+$/g, '') || 'slice'
const worktree = '/tmp/synapsegit-slice-' + slug
const branch = 'agent/' + slug
const targetDir = '/tmp/synapsegit-slice-' + slug + '-target'

const critique = await agent(
  `あなたは敵対的な計画レビュアー (read-only)。以下の SynapseGit slice 計画を批判的に検査する: (1) 凍結領域・規律違反はないか (2) 1 slice = 1 PR として大きすぎ/小さすぎないか (3) checklist の義務 (spec/fixture/docs 連動) の見落とし (4) テスト計画が実装の自己申告に依存していないか (5) より単純な代替はないか。致命的問題のみ blockers、それ以外は improvements に。
## ゴール
${goal}
## 計画
${JSON.stringify(plan, null, 2)}` + GUARDRAILS,
  { label: 'critique', schema: CRITIQUE_SCHEMA }
)
if (critique && critique.blockers && critique.blockers.length > 0) {
  return {
    aborted: true,
    reason: '計画に blocker があるため実装に進みませんでした。計画を修正して再実行してください。',
    goal, plan, blockers: critique.blockers, improvements: critique.improvements,
  }
}
log('計画確定: ' + branch + ' — 実装へ')

phase('Implement')
const impl = await agent(
  `あなたは SynapseGit の実装担当。以下の計画を実装する。
## 準備
- git fetch origin
- git worktree add ${worktree} -b ${branch} origin/main (パス/ブランチが既存なら worktree remove --force と branch -D で除去してやり直す)
- 以後の作業はすべて ${worktree} 内。cargo は CARGO_TARGET_DIR=${targetDir} を付ける。
## 実装
- 可能な限りテストファースト (先に失敗するテストを書き、実装で通す)。
- 計画の checklists / docs_updates の義務を満たす。improvements も妥当なら取り込む。
- 変更は論理単位でローカルコミットする (push はしない)。コミットメッセージは既存の慣行 (feat:/fix:/docs:/chore: プレフィックス) に合わせる。
- 完了後: git -C ${worktree} diff origin/main --stat と git -C ${worktree} log --oneline origin/main.. を取得して返す。
- 計画どおりにいかなかった点・未解決点は notes に正直に書く (自己申告の粉飾は後段レビューで検出される)。
## ゴール
${goal}
## 計画
${JSON.stringify(plan, null, 2)}
## 批判レビューからの改善提案
${JSON.stringify((critique && critique.improvements) || [], null, 2)}` + GUARDRAILS,
  { label: 'implement', schema: IMPL_SCHEMA, model: 'sonnet' }
)
if (!impl) throw new Error('実装エージェントが結果を返しませんでした')

const VERIFY_PROMPT = `あなたは検証担当。${worktree} 内で CI 同等検証を順に実行し、各ステップの結果を返す。fail しても最後まで全ステップ実行する。cargo 系は CARGO_TARGET_DIR=${targetDir} を付ける。
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
14. node scripts/verify_mermaid.mjs (ネットワーク不可なら MERMAID_CLI=<ローカル mmdc> を試し、それも不可なら skipped)
15. node scripts/manage_github_security.mjs --validate
16. git diff --check (worktree 内)` + GUARDRAILS

phase('Verify')
const verify = await agent(VERIFY_PROMPT, { label: 'verify:ci-parity', schema: VERIFY_SCHEMA, model: 'sonnet', effort: 'low' })

phase('Review')
const LENSES = [
  { key: 'correctness', focus: '正しさとテスト: ロジック誤り、境界条件、エラー処理、テスト不足、実装者の自己申告 (notes) と実際の差分の乖離。' },
  { key: 'contracts', focus: '契約と不変条件: 凍結領域への変更混入、OID・canonical bytes・schema・fixture 契約、依存方向 (local-http は local-service 経由のみ等)、error code 契約と #[non_exhaustive] 禁止、依存変更時の THIRD_PARTY_NOTICES 再生成。' },
  { key: 'docs', focus: 'ドキュメント連動: capability 4 文書連動、英日ペア同時更新、openapi.json と architecture 文書の整合、CONTRIBUTING checklist 義務の充足、boundary 記述のトーン維持。' },
]
const reviews = await parallel(LENSES.map(l => () =>
  agent(
    `あなたは敵対的レビュアー。${worktree} の差分 (git -C ${worktree} diff origin/main) を、次のレンズだけで検査して findings を返す (該当なしなら空配列)。実装者の申告: ${JSON.stringify(impl.notes || '')}
## レンズ: ${l.key}
${l.focus}
## ゴール
${goal}` + GUARDRAILS,
    { label: 'review:' + l.key, phase: 'Review', schema: FINDINGS_SCHEMA }
  ).then(r => ((r && r.findings) || []).map(f => Object.assign({ lens: l.key }, f)))
))
let findings = reviews.filter(Boolean).flat()
const serious = findings.filter(f => f.severity === 'high' || f.severity === 'medium')

let fixRound = null
let reverify = null
if (serious.length > 0) {
  log(serious.length + ' 件の要修正 finding — 修正ラウンドを実施')
  fixRound = await agent(
    `あなたは修正担当。${worktree} 内で以下のレビュー findings を修正し、ローカルコミットする (push しない)。修正できない・修正すべきでないと判断した finding は理由つきで notes に書く。修正後の diffstat と追加コミットを返す。cargo は CARGO_TARGET_DIR=${targetDir}。
## findings
${JSON.stringify(serious, null, 2)}` + GUARDRAILS,
    { label: 'fix-round', schema: IMPL_SCHEMA, model: 'sonnet' }
  )
  reverify = await agent(VERIFY_PROMPT, { label: 'reverify:ci-parity', schema: VERIFY_SCHEMA, model: 'sonnet', effort: 'low' })
}

phase('Finalize')
const finalVerify = reverify || verify
const prBody = await agent(
  `あなたは PR 起案担当 (read-only、リポジトリと worktree は変更しない)。.github/pull_request_template.md を読み、そのテンプレートに完全準拠した PR 本文を組み立てて返す。
- 対象: ${worktree} の差分 (git -C ${worktree} diff origin/main --stat / git -C ${worktree} log --oneline origin/main..)。
- 「実行した検証コマンド」には実際の検証結果を反映する: ${JSON.stringify((finalVerify && finalVerify.steps || []).map(s => s.cmd + ' => ' + s.status))}
- 不変条件への影響・migration・security/privacy 項目は差分から誠実に判断して記入。LICENSE Section 5 (contribution grant) の同意確認項目もテンプレートどおり含める。
- 公開向け文章なので絶対パス・環境固有情報を含めない。
- PR タイトル案 (conventional prefix つき) も先頭に含める。
## ゴール
${goal}` + GUARDRAILS,
  { label: 'pr-body', model: 'sonnet' }
)

return {
  goal,
  branch,
  worktree,
  plan_summary: plan.summary,
  implementation: impl,
  fix_round: fixRound,
  verification: finalVerify,
  all_passed: !!(finalVerify && finalVerify.all_passed),
  remaining_findings: findings.filter(f => f.severity === 'low'),
  pr_body: prBody,
  next_steps: [
    '1. 内容確認: git -C ' + worktree + ' log --oneline origin/main.. && git -C ' + worktree + ' diff origin/main',
    '2. push (人間が実行): git -C ' + worktree + ' push -u origin ' + branch,
    '3. PR 作成 (人間が実行): gh pr create --head ' + branch + ' --title <タイトル> --body-file <pr_body を保存したファイル>',
    '4. CI green 確認 → squash merge → squash commit の tree がレビュー済み head と一致することを git rev-parse <merge>^{tree} で検証 → merge commit に対する main CI 成功を確認 (確立プロトコル)',
    '5. 後片付け: git worktree remove ' + worktree,
  ],
}
