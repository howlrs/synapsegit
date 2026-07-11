# SynapseGit Core 使用ガイド

Status: **Core v0.1 / Stage 0 draft**

このガイドは、SynapseGit Coreの想定利用者、Pilotでの使い方、現在このリポジトリで実行できる範囲をまとめる。現時点では完成済みの制作アプリやcapture clientを提供していない。利用フローの図は構想とPilot仮説を含む。

対象はSynapseGit Coreのみであり、Chrono-Engine、歴史的人物の思考再現、自動利益分配はこのガイドとCore v0.1の対象外である。

## branch: main 関連資料

- [想定利用者別シナリオ（PPTX・日本語）](https://github.com/howlrs/synapsegit/blob/main/docs/presentations/synapsegit_user_scenarios_ja.pptx)
- [PPTXの利用・再生成手順](https://github.com/howlrs/synapsegit/blob/main/docs/presentations/README.md)
- [リポジトリREADME](https://github.com/howlrs/synapsegit/blob/main/README.md)
- [Core構想](https://github.com/howlrs/synapsegit/blob/main/docs/core_concept.md)
- [Stage 0 execution plan](https://github.com/howlrs/synapsegit/blob/main/docs/stage0_execution_plan.md)
- [Runtime architecture](https://github.com/howlrs/synapsegit/blob/main/docs/runtime_architecture.md)
- [Core Protocol v0.1](https://github.com/howlrs/synapsegit/blob/main/spec/core/v0.1/README.md)

## 想定利用者と最初に返す価値

| 利用者 | 最初の利用場面 | その場で返すもの |
|---|---|---|
| 画家・壁画家 | 制作session前後を同じ視点から記録する | 前後比較、差分候補、制作process pack |
| 建築家 | Plan、直前現況、現在現況を照合する | 設計変更理由、採用・是正・保留の判断card |
| 施工・修復担当 | Hold Pointや不可逆な処置の前後を残す | 進捗・処置報告、EvidenceGap、引き渡し資料 |
| デザイナー | 複数tool・参考資料・AI案の採否をつなぐ | Proposalの比較、却下理由、可搬なContextPack |
| 制作チーム・後任 | DecisionまたはReleaseから重要変更を辿る | 根拠、制約、未解決事項、未採用案への到達 |

Coreは既存の制作ソフト、BIM/CAD、ペイントツールを置き換えない。それらを横断して、物理対象、成果物、観測、判断を接続する。

## Pilotでの基本的な使い方

### 1. 一つの対象を選ぶ

最初から建物全体や制作活動全体を対象にしない。キャンバス、壁画、小規模な壁面、内装の一区画など、時間を通して追跡する一つの`Subject`を選ぶ。

### 2. 記録する節目を決める

常時監視ではなく、次のような意味のある節目を選ぶ。

- 制作sessionの開始・終了
- 案の承認、設計変更、検査
- 解体、被覆、防水、封止、ワニス等のHold Point
- 修復処置の前後
- 公開、引き渡し、基準版の固定

緊急対応や記録不能な範囲は、作業を止める理由にせず、後追い記録と`EvidenceGap`を許す。

### 3. Capture Profileを選ぶ

| Profile | 最低限残す条件 | 利用できる比較 |
|---|---|---|
| `Imported` | 画像と取得経路 | 参考記録、限定的な外観比較 |
| `Repeatable` | station、viewpoint、許容位置誤差 | 同一視点系列の候補差分 |
| `Calibrated` | marker、scale、色・照明等の校正 | 定義された精度内の寸法・色比較 |

精密な主張が不要な通常Captureへ、校正作業を一律に要求しない。一方、条件が不足するObservationから精密な色・寸法変化を確定表示しない。

### 4. 撮る・取り込む

通常Captureでは長文を要求せず、画像、対象、時刻、状態chip、任意の一言または音声を基本にする。Pilot UX目標は能動入力中央値20秒以内であり、現時点の実績値ではない。

### 5. 三者を比較する

```text
Plan（実現したかった状態）
  ↕ 計画との適合
Previous Observation（直前に観測した状態）
  ↕ 時間変化
Current Observation（現在観測した状態）
```

画像差分は`Analysis`であり、物理変化の確定事実ではない。registration失敗、遮蔽、blur、露出不良、欠測を「変化なし」へ置き換えない。

### 6. 人が意味を確定する

差分候補を、実変化、照明、影、遮蔽物、濡れ・乾燥、不明などへ分類する。採用理由、未解決事項、次に守る制約を必要最小限だけ確認し、Decision Commitとして節目を残す。Pilot UX目標は通常Commitの能動入力中央値30秒以内である。

### 7. 報告・引き継ぎ・archiveへ返す

選択した履歴から、進捗、制作process、処置記録、As-recorded、引き継ぎ資料を構成する。将来のCoreでは、open archiveを空storeへrestoreし、同じOIDとavailabilityを検証できることを受入条件とする。

## Creative AIを使う場合

```text
ContextPack + Policy + DelegationGrant
  → AI Proposal Branch
  → Artifact + Diff + Claim
  → Human Review Gate
  → adopt / modify / reject
  → DecisionFeedback
```

- AIは`proposal/*`だけを作り、`decision/*`と`release/*`を直接進めない。
- 採用、公開、引き渡し、Policy変更、削除、外部送信、物理作用は人の承認を必要とする。
- `generated_by AI`, `selected_by human`, `modified_by human`, `approved_by human`を分離する。
- DecisionFeedbackは既定でproject-localとし、明示opt-inなしに外部model学習へ使用しない。
- AIの採用率だけを成功指標にしない。却下、保留、探索、批評も履歴として価値を持つ。

## 現在このリポジトリで実行できること

リポジトリrootで次を実行する。

```bash
node scripts/verify_core_fixtures.mjs
cargo test --workspace --locked
```

現在のRust実装はstrict JSON、resource limits、canonical bytes、Blob／structured OIDを独立実装し、17 structured fixtureとBlobをJavaScript goldenへ照合する。構造化OIDのAPIはschema／semantic validation前であることを示すため`*_unchecked`としている。

## まだ実行できないこと

- production用のschema／semantic validation入口
- filesystem CAS、SQLite Ref CAS、reflog
- 実際のcapture client、画像registration、compare UI
- archive export／empty-store restoreの実ファイルround trip
- AI proposalからHuman Gateまでのruntime authorization

これらは[Stage 0 execution plan — branch: main](https://github.com/howlrs/synapsegit/blob/main/docs/stage0_execution_plan.md)のexit gateに従って実装する。

## 表示・評価でしないこと

- 「作者を証明」「現実を完全記録」「契約適合を自動証明」と表示しない。
- 差分量、Capture数、Commit数、夜間活動から創造性、努力、勤務時間、生産性を評価しない。
- hidden screen recording、keystroke、常時音声・映像、顔識別を利用しない。
- 「改ざん不能」「永久保存」と販売しない。hashで確認できるのは、含まれる記録のbyte同一性である。
- 写真中心の記録を無条件に`As-built`と呼ばず、`As-recorded`または確認範囲付きの表現を使う。

## リンク運用

このガイドとPPTX内の公開導線は、配布資料から同じ版へ戻れるようGitHubの`main`ブランチを明示したURLを使用する。新規ファイルへのリンクは、変更がcommit・pushされて`main`へ反映されるまで有効にならない。

リポジトリはprivateであるため、反映後もリンクの閲覧にはGitHub上のリポジトリ権限が必要である。権限を持たない外部利用者へは、PPTXまたはPDFを別の許可済み経路で配布する。
