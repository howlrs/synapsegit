export const meta = {
  name: 'release-prep',
  description: 'Prepare a release branch per docs/distribution.md: version bump, release notes, CHANGELOG, doc sync, full CI-parity gate. Never tags or pushes',
  whenToUse: 'Pass the target version as args (e.g. "v0.6.0"). Produces a verified local branch agent/release-<version>; tagging and pushing stay manual per the release runbook.',
  phases: [
    { title: 'Preflight', detail: 'go/no-go checks' },
    { title: 'Prepare', detail: 'version bump + release docs in a worktree' },
    { title: 'Verify', detail: 'verify_release_version + full CI-parity gate' },
    { title: 'Review', detail: 'adversarial diff review vs release gate' },
  ],
}

const GUARDRAILS = `
## SynapseGit 共通ガードレール (必ず遵守)
- リポジトリ root はカレントワーキングディレクトリ (SynapseGit checkout)。リポジトリ本体の working tree は変更しない — 作業は指定された /tmp 配下の git worktree 内のみ。
- cargo は必ず "cargo +1.88.0" で呼ぶ (CI は RUST_TOOLCHAIN=1.88.0 固定)。cargo を内部で呼ぶ Node スクリプトには RUSTUP_TOOLCHAIN=1.88.0 を付ける。CARGO_TARGET_DIR は指定された専用パスを使う。
- 凍結領域 (編集禁止): docs/evaluation/publication-comprehension/v1/ 配下、spec/application/generic-artifact/v1/ と generic-artifact-publication/v1/、既存の docs/releases/v*.md (新バージョンのファイル追加は可)、CHANGELOG.md の過去バージョンセクション (新セクション追加と [Unreleased] の移動は可)、公開済み v* タグ。
- 外向きアクションの実行禁止: git push / git tag / gh pr create / gh pr merge / リリース公開。annotated tag の作成もしない (merge 後に人間が行う)。worktree 内のローカルコミットは可。
- 依存が変わった場合は RUSTUP_TOOLCHAIN=1.88.0 node scripts/generate_third_party_notices.mjs --write を同一変更内で実行。
- 出力・レポートは日本語。
`

const PRE_SCHEMA = {
  type: 'object',
  required: ['go', 'reasons', 'facts'],
  properties: {
    go: { type: 'boolean' },
    reasons: { type: 'array', items: { type: 'string' }, description: 'no-go の場合の理由 (go なら空)' },
    facts: { type: 'string', description: '現在版数、[Unreleased] の内容要約、main と origin/main の状態、open PR/issue' },
  },
}

const PREP_SCHEMA = {
  type: 'object',
  required: ['commits', 'diffstat', 'notes'],
  properties: {
    commits: { type: 'array', items: { type: 'string' } },
    diffstat: { type: 'string' },
    notes: { type: 'string', description: '判断に迷った点・人間の確認が必要な点' },
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

const REVIEW_SCHEMA = {
  type: 'object',
  required: ['ok', 'problems'],
  properties: {
    ok: { type: 'boolean' },
    problems: { type: 'array', items: { type: 'string' } },
  },
}

if (typeof args !== 'string' || !/^v\d+\.\d+\.\d+$/.test(args)) {
  throw new Error('args にリリースバージョンを "vX.Y.Z" 形式で渡してください。例: Workflow({name: "release-prep", args: "v0.6.0"})')
}
const version = args
const bare = version.slice(1)
const slug = bare.replace(/\./g, '-')
const worktree = '/tmp/synapsegit-release-' + slug
const branch = 'agent/release-v' + slug
const targetDir = worktree + '-target'

phase('Preflight')
const pre = await agent(
  `リリース準備 ${version} の go/no-go 判定 (read-only)。
- git fetch origin。ローカル main と origin/main の一致、working tree の clean を確認 (不一致・dirty は no-go 理由に)。
- 現在のクレート版数 (crates/*/Cargo.toml) と最新タグを確認し、${version} が単調増加であることを確認。
- CHANGELOG.md の [Unreleased] セクションが空でないことを確認 (空ならリリースする変更がない = no-go)。
- gh pr list --state open / gh issue list --state open で、リリース前に処理すべき open 項目を facts に列挙 (dependabot PR の放置は no-go にはしないが facts に明記)。
- docs/distribution.md の Release gate 節を読み、追加の前提条件があれば確認。` + GUARDRAILS,
  { label: 'preflight', schema: PRE_SCHEMA, model: 'sonnet' }
)
if (!pre || !pre.go) {
  return { aborted: true, version, reasons: (pre && pre.reasons) || ['preflight エージェントが結果を返しませんでした'], facts: pre && pre.facts }
}
log('preflight go — ' + version + ' の準備を開始')

phase('Prepare')
const prep = await agent(
  `あなたはリリース準備担当。${version} のリリース準備ブランチを作る。
## 準備
- git worktree add ${worktree} -b ${branch} origin/main (既存なら remove --force / branch -D でやり直す)。以後すべて ${worktree} 内で作業。
## 参照
- docs/distribution.md の Release gate 節が正。前回のリリース準備コミット (git log --oneline --grep "release: prepare" -1 とその diff) を形式の参考にする。
## 作業項目 (すべて同一ブランチ上)
1. 全 15 クレート (crates/*/Cargo.toml) の version を ${bare} に統一。ワークスペース内クロス依存の version 指定があればそれも更新。
2. Cargo.lock を更新: CARGO_TARGET_DIR=${targetDir} cargo +1.88.0 update --workspace を実行し、workspace メンバーの版のみ変わることを git diff で確認。
3. docs/releases/${version}.md を新規作成 (既存リリースノートの構造 — Highlights / Install と検証手順 / Compatibility and migration / Known limits — に合わせる。Stage 0 preview の抑制的トーンを維持)。
4. CHANGELOG.md: [Unreleased] の内容を [${bare}] - <今日の日付 (date +%F)> セクションへ移動し、空の [Unreleased] を残す。末尾の compare link 定義を更新。過去セクションは変更しない。
5. README.md / README.ja.md のリリース参照 (版数・Release URL・checksum 手順の版数) を更新。両言語で同時に。
6. docs/install.md / docs/project_status.md / docs/distribution.md の版数と Last verified を更新。
7. SECURITY.md の Supported versions 表を ${version} 系に更新 (過去リリースで更新漏れの実績がある要注意項目)。
8. 旧版数の参照が他の Markdown に残っていないか grep で確認し、履歴・凍結領域以外は更新 (過去実績: docs/tutorial/README*.md, deploy/local/README.md に残存)。
9. RUSTUP_TOOLCHAIN=1.88.0 node scripts/generate_third_party_notices.mjs --check を実行し、依存が変わっていれば --write で更新 (Gemini レビュー指摘による明示項目)。
10. 変更を論理単位でローカルコミット (メッセージ例: "release: prepare ${version}")。push はしない。
完了後: git -C ${worktree} diff origin/main --stat と git -C ${worktree} log --oneline origin/main.. を返す。迷った点は notes に。` + GUARDRAILS,
  { label: 'prepare', schema: PREP_SCHEMA, model: 'sonnet' }
)
if (!prep) throw new Error('リリース準備エージェントが結果を返しませんでした')

phase('Verify')
const verify = await agent(
  `あなたは検証担当。${worktree} 内でリリース検証を順に実行し、各ステップの結果を返す。fail しても全ステップ実行する。cargo 系は CARGO_TARGET_DIR=${targetDir}。
 0. bash scripts/verify_release_version.sh ${version} (引数の形式はスクリプト冒頭の usage を読んで合わせる)
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
14. node scripts/verify_mermaid.mjs (ネットワーク不可なら MERMAID_CLI を試し、それも不可なら skipped)
15. node scripts/manage_github_security.mjs --validate
16. git diff --check
17. 旧版数残存の再確認: 凍結領域・履歴以外の Markdown に ${version} より古い版数参照が残っていないか grep` + GUARDRAILS,
  { label: 'verify:release-gate', schema: VERIFY_SCHEMA, model: 'sonnet', effort: 'low' }
)

phase('Review')
const review = await agent(
  `あなたは敵対的レビュアー (read-only)。${worktree} の差分 (git -C ${worktree} diff origin/main) を docs/distribution.md の Release gate 全項目と突合し、以下を検査する:
(1) gate 項目の抜け (版数・release notes・CHANGELOG・README 英日・project_status・install・SECURITY.md Supported versions)
(2) CHANGELOG の形式 (Keep a Changelog 準拠、compare link、過去セクション不変)
(3) release notes のトーン (Stage 0 preview の抑制的 boundary 記述の維持、誇張なし)
(4) 凍結領域への変更混入 (git diff --name-only で確認)
(5) Cargo.lock の変更が workspace メンバー版数のみか
問題ゼロなら ok=true。` + GUARDRAILS,
  { label: 'review:gate', schema: REVIEW_SCHEMA }
)

return {
  version,
  branch,
  worktree,
  preflight_facts: pre.facts,
  preparation: prep,
  verification: verify,
  all_passed: !!(verify && verify.all_passed),
  gate_review: review,
  next_steps: [
    '1. 差分確認: git -C ' + worktree + ' diff origin/main',
    '2. push (人間が実行): git -C ' + worktree + ' push -u origin ' + branch,
    '3. PR 作成 → 独立レビュー → CI green → squash merge → tree 一致検証 + merge commit CI 確認 (確立プロトコル)',
    '4. merge 後、clean checkout で docs/distribution.md の Release gate フル検証を再実行',
    '5. annotated tag 作成と push (人間が実行): git tag -a ' + version + ' -m "SynapseGit ' + version + '" <merge commit> && git push origin ' + version + ' (lightweight tag は release.yml が拒否する)',
    '6. release.yml 完了後の独立検証: 別ディレクトリへ download → sha256sum --check → gh attestation verify --repo howlrs/synapsegit --deny-self-hosted-runners → 三 binary の smoke (docs/distribution.md 記載の手順)',
  ],
}
