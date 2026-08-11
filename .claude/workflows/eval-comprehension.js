export const meta = {
  name: 'eval-comprehension',
  description: 'Run the frozen v1 publication-comprehension protocol AI tracks with zero-context evaluator agents and score the responses',
  whenToUse: 'Executes the top-priority backlog item: AI/JSON and AI/HTML tracks of docs/evaluation/publication-comprehension/v1 (default 3 runs per case/format) with strict context isolation, then runs the scorer. Human track and accessibility checks remain manual. Args: {runs: N} to change run count.',
  phases: [
    { title: 'Setup', detail: 'read protocol, copy artifacts to opaque paths, record SHA-256' },
    { title: 'Evaluate', detail: 'zero-context evaluators (cases x formats x runs)' },
    { title: 'Score', detail: 'assemble response envelopes and run the scorer' },
  ],
}

const SETUP_SCHEMA = {
  type: 'object',
  required: ['evaluation_cases', 'private_notes'],
  properties: {
    evaluation_cases: {
      type: 'array',
      items: {
        type: 'object',
        required: ['opaque_id', 'format', 'artifact_path', 'questionnaire', 'response_format'],
        properties: {
          opaque_id: { type: 'string', description: 'ケース内容 (complete/incomplete 等) を推測させない不透明 ID (例: case-a)' },
          format: { type: 'string', enum: ['json', 'html'] },
          artifact_path: { type: 'string', description: '/tmp/synapsegit-eval/inputs/ 配下へコピーした、元のケース名を含まないパス' },
          questionnaire: { type: 'string', description: '評価者に渡す質問票全文。oracle・期待回答・実ケース名・コーパスの説明を含めない' },
          response_format: { type: 'string', description: '評価者が書き出す回答 JSON の形式説明 (キー構成)' },
        },
      },
    },
    private_notes: { type: 'string', description: 'opaque_id と実ケース/トラックの対応、原本とコピーの SHA-256 (一致確認済み)、response envelope の組立てと scorer 実行に必要な情報。評価者には渡らない' },
  },
}

const EVAL_SCHEMA = {
  type: 'object',
  required: ['written_path'],
  properties: {
    written_path: { type: 'string', description: '回答 JSON を書き出したパス' },
    note: { type: 'string', description: '1行の所感 (任意)' },
  },
}

const RUNS = (args && Number(args.runs)) || 3
if (RUNS < 1 || RUNS > 5 || !Number.isInteger(RUNS)) throw new Error('args.runs は 1〜5 の整数で指定してください')

phase('Setup')
const setup = await agent(
  `あなたは SynapseGit の凍結評価コーパス実行のセットアップ担当。
対象: docs/evaluation/publication-comprehension/v1/ (バイト不変の凍結領域 — 読み取りとコピーのみ。1 バイトも変更しない)。
手順:
1. コーパスの README / protocol.json / questionnaire 類を読み、AI トラック (JSON / HTML) の評価ケース (complete / incomplete-only の 2 ケース想定だが、コーパスの記述を正とする) を特定する。
2. mkdir -p /tmp/synapsegit-eval/inputs /tmp/synapsegit-eval/raw を実行 (既存の古い中身があれば /tmp/synapsegit-eval ごと削除してから作り直す)。
3. 各ケース x 各フォーマットの評価対象アーティファクト (例: projection.json / index.html) を、実ケース名を含まない不透明な名前 (input-a.json, input-b.html など) で /tmp/synapsegit-eval/inputs/ にコピーし、原本とコピー両方の sha256sum が一致することを確認して private_notes に記録する。
4. protocol の質問票から、評価者にそのまま渡せる questionnaire 文面と、回答 JSON の response_format を各ケースぶん組み立てる。questionnaire には oracle の内容・期待回答・実ケース名・「これは不完全ケースです」等のヒントを絶対に含めない (含めると評価が無効になる)。
5. response envelope の要求形式 (protocol が求めるメタデータ: run ID、入力 SHA-256、モデル情報等) と scorer (node scripts/score_publication_comprehension.mjs) の実行方法を private_notes にまとめる。
出力は日本語。`,
  { label: 'setup', schema: SETUP_SCHEMA, model: 'sonnet' }
)
if (!setup || !setup.evaluation_cases || setup.evaluation_cases.length === 0) {
  throw new Error('セットアップに失敗しました (evaluation_cases が空)')
}
log(setup.evaluation_cases.length + ' ケース x ' + RUNS + ' run の評価を開始')

phase('Evaluate')
const runsList = setup.evaluation_cases.flatMap(c =>
  Array.from({ length: RUNS }, (_, i) => ({
    opaque_id: c.opaque_id,
    format: c.format,
    artifact_path: c.artifact_path,
    questionnaire: c.questionnaire,
    response_format: c.response_format,
    run: i + 1,
  }))
)
const answers = await parallel(runsList.map(r => () =>
  agent(
    `あなたはゼロコンテキスト評価者です。このプロジェクトの事前知識を一切持たない外部の読者として振る舞ってください。

## 厳格な隔離規則 (違反すると評価全体が無効になります)
- 読んでよいファイルは次の 1 つだけです: ${r.artifact_path}
- 上記以外のファイルの閲覧、ディレクトリ一覧、リポジトリ内の検索 (grep/glob)、git コマンド、Web アクセスはすべて禁止です。
- このファイルから読み取れないことは「わからない/記載がない」と正直に答えてください。推測や一般知識で補完しないでください。

## 手順
1. 上記ファイルを読む。
2. 次の質問票に、ファイルの内容のみに基づいて回答する。
---
${r.questionnaire}
---
3. 回答を次の形式の JSON で /tmp/synapsegit-eval/raw/${r.opaque_id}-run${r.run}.json に書き出す:
${r.response_format}
4. 最終出力には書き出したファイルのパスと 1 行の所感のみを返す (回答本文は繰り返さない)。`,
    { label: 'eval:' + r.opaque_id + ':run' + r.run, phase: 'Evaluate', schema: EVAL_SCHEMA }
  ).then(a => (a ? Object.assign({ opaque_id: r.opaque_id, format: r.format, run: r.run }, a) : null))
))
const done = answers.filter(Boolean)
log(done.length + '/' + runsList.length + ' run 完了')

phase('Score')
const scored = await agent(
  `あなたは評価の集計担当。凍結コーパス docs/evaluation/publication-comprehension/v1/ は読んでよい (oracle 含む) が、1 バイトも変更しない。
入力: 評価者の生回答ファイル (以下)。
${JSON.stringify(done, null, 2)}
セットアップ担当の引き継ぎ (opaque_id と実ケースの対応、SHA-256、envelope 形式、scorer 実行方法):
${setup.private_notes}
手順:
1. protocol.json の response envelope 形式を読み直し、生回答を実ケースに対応づけて /tmp/synapsegit-eval/responses/ に envelope JSON として組み立てる (入力アーティファクトの SHA-256、run ID、評価モデルが Claude Code サブエージェントである旨のメタデータを含める)。
2. scorer を実行する: node scripts/score_publication_comprehension.mjs (正確な引数はスクリプト冒頭か使用方法を読んで確認)。
3. 結果を報告: ケース x フォーマットごとのスコア、run 間のばらつき、protocol の成功基準に対する pass/fail、失点した設問と傾向。
4. 明記: 本実行がカバーしたのは AI トラックのみで、Human トラック (20 名) と accessibility 検査 (axe/keyboard/zoom/320px/screen-reader) は未実施のまま残ること。scorer が exit 0 でも not_run トラックは pass ではない。
5. リポジトリは一切変更しない。結果ファイルは /tmp/synapsegit-eval/ に残す。
出力は日本語。`,
  { label: 'score', model: 'sonnet' }
)

return {
  runs_completed: done.length,
  runs_planned: runsList.length,
  raw_dir: '/tmp/synapsegit-eval/raw',
  responses_dir: '/tmp/synapsegit-eval/responses',
  report: scored,
  note: '本ワークフローは AI トラック (JSON/HTML) のみを実行します。Human トラックと accessibility 検査は手動実施が必要です。結果を記録する場合は評価成果物 (/tmp/synapsegit-eval/) を別途保全してください。',
}
