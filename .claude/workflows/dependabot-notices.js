export const meta = {
  name: 'dependabot-notices',
  description: 'Regenerate THIRD_PARTY_NOTICES.md for open dependabot PRs and prepare local fix commits (never pushes)',
  whenToUse: 'Run when dependabot PRs fail CI on the third-party notices contract check. Pass {check:true} to also run cargo check per PR (slow). Push commands are reported for a human to run.',
  phases: [
    { title: 'Discover', detail: 'list open dependabot PRs' },
    { title: 'Fix', detail: 'per-PR worktree + notices regeneration + local commit' },
  ],
}

const GUARDRAILS = `
## SynapseGit 共通ガードレール (必ず遵守)
- リポジトリ root はカレントワーキングディレクトリ (SynapseGit checkout)。リポジトリ本体の working tree は変更しない (作業は /tmp 配下の git worktree 内のみ)。
- cargo は必ず "cargo +1.88.0" で呼び、CARGO_TARGET_DIR を自分専用のパスに分離する。cargo を内部で呼ぶ Node スクリプトには RUSTUP_TOOLCHAIN=1.88.0 を付ける。
- 外向きアクションの実行禁止: git push / git tag / gh pr merge / gh issue・pr create / GitHub 設定変更。push が必要な場合はコマンドをレポートに書いて人間に委ねる。
- 凍結領域 (編集禁止): docs/evaluation/publication-comprehension/v1/ 配下、spec/application/generic-artifact/v1/ と generic-artifact-publication/v1/、docs/releases/v*.md、CHANGELOG.md の過去バージョンセクション。
- 出力・レポートは日本語。
`

const LIST_SCHEMA = {
  type: 'object',
  required: ['prs'],
  properties: {
    prs: {
      type: 'array',
      items: {
        type: 'object',
        required: ['number', 'title', 'branch'],
        properties: {
          number: { type: 'integer' },
          title: { type: 'string' },
          branch: { type: 'string', description: 'headRefName' },
          url: { type: 'string' },
        },
      },
    },
  },
}

const RESULT_SCHEMA = {
  type: 'object',
  required: ['number', 'notices_was_stale', 'check_result', 'committed', 'worktree', 'push_command', 'notes'],
  properties: {
    number: { type: 'integer' },
    notices_was_stale: { type: 'boolean' },
    check_result: { type: 'string', enum: ['passed', 'failed', 'skipped'], description: 'cargo check の結果' },
    committed: { type: 'boolean' },
    worktree: { type: 'string' },
    push_command: { type: 'string', description: '人間が実行すべき push コマンド (自分では実行しない)。commit が無い場合は空文字' },
    notes: { type: 'string' },
  },
}

const doCheck = !!(args && args.check === true)

phase('Discover')
const list = await agent(
  'open の dependabot PR を列挙する: gh pr list --author "app/dependabot" --state open --json number,title,headRefName,url を実行し、headRefName を branch に入れて prs として返す。リポジトリは変更しない。' + GUARDRAILS,
  { label: 'discover', schema: LIST_SCHEMA, model: 'sonnet', effort: 'low' }
)
const prs = (list && list.prs) || []
if (prs.length === 0) {
  return { message: 'open の dependabot PR はありません。', processed: [], push_commands: [] }
}
log(prs.length + ' 件の dependabot PR を処理します')

phase('Fix')
const results = await pipeline(prs, (_first, pr) => agent(
  `担当: dependabot PR #${pr.number} (${pr.title}) の THIRD_PARTY_NOTICES.md 修復準備。
手順:
1. git fetch origin ${pr.branch}
2. git worktree add /tmp/synapsegit-dep-${pr.number} -b depfix/${pr.number} origin/${pr.branch}
   - パスまたはブランチが既存なら git worktree remove --force /tmp/synapsegit-dep-${pr.number} と git branch -D depfix/${pr.number} で除去してからやり直す。
   - git の index.lock 等の一時的なロックエラーは 5 秒待って 1 回だけ再試行する。
3. worktree 内で RUSTUP_TOOLCHAIN=1.88.0 node scripts/generate_third_party_notices.mjs --check を実行し、stale なら同 --write で再生成する。
${doCheck
    ? `4. worktree 内で CARGO_TARGET_DIR=/tmp/synapsegit-dep-${pr.number}-target cargo +1.88.0 check --workspace --locked を実行し、結果を check_result に記録する。`
    : '4. cargo check は今回スキップ (check_result は skipped)。CI が push 後に全量検証する。'}
5. 変更が生じた場合のみ worktree 内でコミットする。コミットメッセージ: "chore: regenerate THIRD_PARTY_NOTICES.md"
6. push は絶対にしない。push_command には人間が実行すべき "git push origin depfix/${pr.number}:${pr.branch}" を入れる。
7. worktree は削除せず残す (人間が push 前に確認できるように)。
注意: dependabot が後から force-push すると depfix コミットは上書きされるため、push は速やかに行うべき旨を notes に含める。
` + GUARDRAILS,
  { label: 'fix:#' + pr.number, phase: 'Fix', schema: RESULT_SCHEMA, model: 'sonnet' }
))

const done = results.filter(Boolean)
const failed = done.filter(r => r.check_result === 'failed')
if (failed.length) log(failed.length + ' 件で cargo check が失敗 — レポート参照')

return {
  processed: done,
  push_commands: done.filter(r => r.committed && r.push_command).map(r => r.push_command),
  note: 'push は実行していません。push_commands を確認のうえ人間が実行してください。push 後、各 PR の CI green を確認してから squash merge してください。notices 以外の理由で CI が落ちる場合は個別に調査が必要です。',
}
