# 星月夜の上塗り: SynapseGit 公開事例

[English](./README.md) | [事例の公開リポジトリ](https://github.com/howlrs/synapsegit-starry-night/tree/45db60e3497f5392a9fb93135698021d23e42059) | [公開bundle](https://github.com/howlrs/synapsegit-starry-night/tree/45db60e3497f5392a9fb93135698021d23e42059/synapsegit/bundle) | [SynapseGit チュートリアル](../../tutorial/README.ja.md)

制作の節目で、何を見て、どんな次の一手を提案したのか。実物のアクリル画の写真と
工程地図を、Original・Current・AI outputに割り当てる事例です。
画像を用意する工程と、人が見比べて判断する工程を分けて記録します。

SynapseGit開発者の **howlrs** は、IKEAで購入したゴッホ《星月夜》レプリカへ、白・青・黒の
アクリル絵の具を重ね、明度5段階の絵に描き替えています。Claude Codeを使うワークフローが
明度見本と工程地図を供給しました。ただしこの事例はmodelの実行を検証しておらず、特定の
modelへの帰属もしていません。地図はcaller-suppliedなAI outputです。

## ひとつの具体的な見比べ

`step2-lightblue` は明るい青の工程です。下の3画像は、塗る前の状態、この工程の直前に
あった状態、この工程で提案された内容を、別々の役割として示します。

| Original: 塗る前のレプリカ | Current: 工程1・白の後 | AI output: 工程2の地図 |
|---|---|---|
| [![塗る前の星月夜レプリカ](./assets/original.jpg)](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/docs/images/20261009_step1_before.jpg) | [![白の工程後の星月夜レプリカ](./assets/current.jpg)](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/docs/images/20261009_step1_after.jpg) | [![明るい青の工程を示すAI output地図](./assets/proposal.png)](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/output/steps_blue/step2_lightblue.png) |
| 基準となる写真 | 工程2直前の観測状態 | 明るい青を塗る場所の提案 |

写真は公開済みの表示用コピー（1200×900）で、実際に登録した写真とは異なるbytesです。
工程地図は公開済みの提案ファイルをそのままコピーしています。
このページの画像のbyte checksumは[assets/SHA256SUMS](./assets/SHA256SUMS)で確認できますが、
SynapseGit Core objectやOIDを検証するものではありません。実際のinput hashは固定した
[workflow](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/docs/synapsegit-workflow.md)を参照してください。各画像から公開済みの固定sourceへ移動できます。

工程2と3は続けて塗っており、その間の写真はありません。そのため工程3のCurrentは工程2
開始前と同じ写真です。これは後の状態を正確に撮影したものではなく、その限界を明示した近似です。

## 3画像と判断を分ける理由

| 役割 | この事例で入れるもの | 見る人が確認できること |
|---|---|---|
| Original | 塗る前のレプリカ写真 | どの状態を基準にしたか |
| Current | 白の工程後の写真 | 提案前に何を観測したか |
| AI output | 明度・工程の地図 | 次に何を提案したか |
| Human Decision | 工程2の提案を採用 | 制作者が3画像を見比べ、会話で伝えた採用判断を記録した |

制作者が塗った写真をAI outputとは呼びません。塗り終えた写真は、次の提案にとっての
Currentになれます。画面外で進む制作でも、この区別があれば後からレビューできます。

## 2026-10-09時点の状態

明度5段階の計画、白、明るい青、中間の青、青の5件には、Human `adopt`判断が記録されています。
固定した公開bundleには、これらの完了sessionのpublic projectionがあります。採用は提案bytesを
そのまま選んだ記録です。地図どおりに塗ったこと、outputを変更したこと、物理的に絵が変わったことは示しません。
実際の判断は制作者が3画像を確認して会話で伝え、エージェントがCLIで記録しました。
公開bundleは`synapse-present 1.0.0`の`preview`で整合性を確認しています。

制作者は開発者本人です。独立した利用者によるPilot評価や、顧客の導入成果としては扱いません。

SynapseGitが確かめられるのは、記録されたfileのbyte identityです。物理的に絵が変わったこと、
作者、modelが地図を作ったことは確かめません。この制限は上のstatusとセットで読んでください。

## 同じ形で試す

1. まず[チュートリアル](../../tutorial/README.ja.md)で、完了まで進むsyntheticな3画像の例を試します。
2. 手元でOriginalの基準画像、Currentの観測画像、提案outputの3枚を用意します。privateなOriginalはアップロードしません。
3. [Creator操作ガイド](../../creator_workflow.md)に沿って提案を記録し、見比べた後にHuman Decisionを残します。

評価時のinstallは、このcheckout自身の[Installation guide](../../install.md)を参照してください。
この事例はv1.0.0で記録し、このcheckoutはv1.1.0を文書化しています。
v1.xの旧版読み取りの約束は[互換性方針](../../compatibility.md)を参照してください。

利用や評価の質問は、[Support](../../../SUPPORT.md)の窓口へ、非公開の画像を添付せずに相談できます。
商用・production利用には別途書面での許可が必要です。[License](../../../LICENSE)を確認してください。

## 固定したsource

このページの事実は、事例リポジトリのcommit [`45db60e`](https://github.com/howlrs/synapsegit-starry-night/tree/45db60e3497f5392a9fb93135698021d23e42059)に固定しています。[README](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/README.md)、[記録方針](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/RECORDING.md)、[workflow](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/docs/synapsegit-workflow.md)、[進捗](https://github.com/howlrs/synapsegit-starry-night/blob/45db60e3497f5392a9fb93135698021d23e42059/progress.md)、[公開bundle](https://github.com/howlrs/synapsegit-starry-night/tree/45db60e3497f5392a9fb93135698021d23e42059/synapsegit/bundle)です。
