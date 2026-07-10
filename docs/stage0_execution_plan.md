# SynapseGit Core Stage 0 execution plan

Status: ready to start

Target: four-week protocol and vertical-slice spike

## Outcome

Stage 0の終了条件は、UIの完成でもAI機能の多さでもない。次の一文を実データで成立させることである。

> CreatorとCreative AIが同じ制作履歴を利用しながら、AIはproposalだけを作り、人が公式判断を持ち、固定点Observationを含む履歴を別環境へ同じOIDで復元できる。

## Workstream A: protocol freeze

### Week 1

- `spec/core/v0.1`のOID preimage、strict JSON、timestamp、fixed-point、set/sequenceをRustへ実装する。
- JS verifierとはコードを共有せず、17 structured fixtureとBlobのOIDを一致させる。
- duplicate key、invalid UTF-8、BOM、lone surrogate、float/exponent/`-0`をparse前に拒否する。
- `record.schema.json`で具象recordをdispatchし、`operations.md`のsemantic error codeを固定する。
- RustとJSの相互fixtureが一致した時点で`sg-oid-v1`をfreeze候補にする。

OID freeze前なら破壊的修正を許す。freeze後は同じprefixの意味を変更せず、新profileへversionを上げる。

## Workstream B: Rust Core vertical slice

### Week 1–2

最初のCLI経路を一本だけ完成させる。

```text
put-blob / put-record
  -> build-tree
  -> commit
  -> update-ref --expected
  -> fsck
  -> export
  -> empty-store restore
```

実装境界は[`runtime_architecture.md`](./runtime_architecture.md)に従う。

- filesystem CASを正本にする。
- SQLite transactionでRefとreflogをCAS更新する。
- projection tableは全削除してCASから再生成できるようにする。
- object書込みはtemporary file、flush、atomic rename、closure検証、Ref更新の順にする。
- upload quota、object count/depth上限、巨大JSON/zip bomb等のresource limitを最初から置く。

### CLI acceptance

- 同じBlob/Recordの再投入は同じOIDを返す。
- OIDとbody不一致を拒否する。
- stale `expected_head`でRefを動かさない。
- processを各書込み境界で強制終了しても、公開済みRefのclosureが壊れない。
- exportを空directoryへrestoreし、Commit DAGとavailability stateが一致する。

## Workstream C: fixed-point Observation pilot

### Week 2–3

二つの小さなdatasetだけを作る。

1. **Painting control**: 平面キャンバスまたは壁画を固定stationから反復撮影する。
2. **Building validation**: 小規模な一壁面・一区画を近似固定視点で撮影する。

各datasetに最低限含める条件:

- 対象無変更で照明だけ変更
- 対象無変更でcamera位置を許容範囲内・範囲外へ変更
- 既知領域への小変更
- 遮蔽、反射、blur、露出不良
- CaptureProfileの`imported`, `repeatable`, `calibrated`比較
- Plan、Previous Observation、Current Observationの三者比較

Python adapterはOIDを決めず、入力OID、adapter/version/configuration digest、結果BlobをRust Coreへ返す。出力は`comparable/partial/incomparable`とreason code、`changed/unchanged/ambiguous/unobservable` maskを必須にする。

### Observation acceptance

- 対象無変更＋照明差を物理変更として断定しない。
- registration失敗、遮蔽過多、欠測を「変化なし」に変換しない。
- base→target方向を交換したAnalysisが別の意味として残る。
- RAW、preview、normalized image、maskが別OIDで追跡できる。
- 既知変更領域に対する見逃し・誤検出と、条件別の限界を報告できる。

## Workstream D: Creator / Creative AI value slice

### Week 3–4

最初は外部modelの品質競争をしない。fixtureまたは単純adapterでもよいので、権限と履歴の縦切りを完成させる。

```text
Decision Commit
  -> ContextPack + Policy + DelegationGrant
  -> AI Activity
  -> proposal/{agent}/{run}
  -> Diff + Claim
  -> human adopt / modify / reject
  -> DecisionFeedback
  -> next ContextPack
```

AIの有効能力はActor、Grant、Policy、runtime capabilityの積集合とする。AIは`decision/*`, `release/*`, policy、外部egress、erasure、物理作用を直接変更できない。base Refが進んだ出力は`stale_base`として残し、自動rebaseしない。

### Creator benefit hypotheses

| 仮説 | Pilot metric |
|---|---|
| 記録が制作を中断しない | Captureの能動入力中央値20秒以内 |
| 節目を残す負担が小さい | Commitの能動入力中央値30秒以内 |
| 判断根拠を再発見できる | 1か月後に重要変更を本人が2分以内に説明 |
| 引継ぎと報告が速くなる | report/handoff作成時間を現行手順と比較 |
| 却下案も再利用できる | proposal再利用件数と再探索時間 |
| archiveがservice外で生きる | 空store restore成功率100% |

各annotation要求には即時の見返りを付ける。例として領域選択からDiff report、音声理由からDecision rationale、CaptureProfile入力から撮影ガイドを即座に返す。Commit件数やannotation量をcreator評価や料金指標にしない。

### Creative AI benefit and safety hypotheses

| 仮説 | Pilot metric |
|---|---|
| chatより正確なproject contextを受け取れる | AI Artifactからbase/input/Policy OIDへ到達100% |
| 却下理由を次回に利用できる | reviewed DecisionFeedbackを次Contextへ含めた割合 |
| 複数AIが履歴を壊さず探索できる | proposal Ref外へのunauthorized write 0件 |
| staleな案を正史へ混ぜない | base mismatch検出100% |
| creator dataを勝手に学習へ出さない | opt-inなしのexternal training/egress 0件 |
| 人の決定権を保つ | decision/release RefのAI直接更新0件 |

AI採用率は成功指標にしない。却下、保留、探索、批評にも価値があるためである。

## SQLite / SurrealDB decision spike

Stage 0ではどちらかへ履歴を固定しない。同じfilesystem CASから両ProjectionStoreを構築し、`runtime_architecture.md`記載の8 queryで比較する。

SurrealDBを既定へ昇格する条件:

- OID、Commit、exportの正本がDBから独立している。
- 全projectionを空から再構築できる。
- concurrent CAS試験で履歴消失がない。
- SQLiteより横断queryの実装または性能に明確な利益がある。
- version migration失敗時もCASとRefsから復旧できる。

条件を満たさなくても、分析・可視化用の任意adapterとして残せる。

## Explicit non-goals

- 建物全体のBIM自動照合
- 3D/point cloud/3GSの本格diff
- AIによる作者・貢献率・工程完了の自動認定
- public social network、marketplace、token
- cross-repository mergeとfederation
- 「永久保存」「作者証明」「現実の完全な正史」という販売表現
- creator dataの広告利用、無断model training、創造性score

## Exit gate

次の全項目を満たした時だけStage 1 Core kernelへ進む。

- RustとJSが全golden OID、canonical length、canonical SHA-256で一致する。
- 具象schema、semantic validation、stable error codeが実行される。
- local CAS、SQLite Ref CAS、reflog、fsck、export/restoreの縦切りが動く。
- present/tombstoned/missing closureを復元できる。
- Painting control datasetで観測条件差と既知変更を区別して報告できる。
- Creative AI flowがproposal-onlyとHuman Gateを守る。
- Creator benefit metricを採取でき、記録負担が目標から大きく外れていない。
- SurrealDBは測定結果に基づきdefault / optional / deferのいずれかへ明示決定する。

Stage 0で性能やUIが未完成でもよい。OIDの意味、履歴の復元性、人とAIの決定境界が曖昧なままStage 1へ進まない。
