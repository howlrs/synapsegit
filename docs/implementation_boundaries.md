# 実装の境界とlibraryだけの機能

対象: Rust workspaceを組み込む・評価する実装者、tool builder、評価者。制作者が配布binaryを使うために
読む必要はありません。

root [README](../README.ja.md)は、配布する3 binary（`synapse`、`synapse-local`、`synapse-present`）で
制作者が何をできるかを説明します。このページは、それらのbinaryの技術的な境界と、workspace libraryだけに
ある機能を扱います。READMEから意味を変えずに移しました。

「実装済み」は、このrepositoryのtestで検証される範囲を意味します。ここに書くlibrary／schema surfaceは、
transport統合のtest完了や配布binaryの機能を意味しません。どちらもreal-user認証、network transport、
production運用、一般利用者向けapplicationの完成を意味しません。

## Generic regular-file artifact library

source／workspace libraryには、sibling applicationがgenericな
regular-file reviewを実装するための評価用building blockもあります。
`synapse-artifact`はregular-file manifest全体を検証し、Refを
進めずにnested site Treeへdeterministicに変換します。trusted workflowはprofile-owned repositoryを
初期化し、canonical Decisionのexact headごとにactive Proposalを最大一つpublishします。
completed Decisionで選ばれたsiteは次のProposalのverified accepted baseとなり、attemptごとにfreshな
deterministic Ref／immutable identityを持つため、過去のProposal historyは保持されます。

same-process pending authorityは引き続きnon-serializableかつone-shotです。
`decide_artifact_proposal`には、embedding hostがreviewerをauthenticateしserver-owned project ACLを
確認した後にだけ発行するopaque／expiringな`ArtifactDecisionApproval`も必要です。approvalはexactな
actor／session、security epoch、Proposal／expected Decision head、disposition、rationaleの有無とbytesへ
束縛され、Decision object／Ref mutationより前にburnされます。browser fieldや`ReviewId`から復元しません。
v1 workflowが扱うのはcaller-supplied AI-attributed bytesだけで、verified executionを表現できません。
SynapseGit自身はmodelを呼びません。

固定された
[`synapsegit.generic-artifact` v1 contract](../spec/application/generic-artifact/v1/README.md)の
opaque `ReviewId`はlookup locatorであり、authorityではありません。別SQLite journalと明示的な
orchestration境界は、Proposal CAS前にprivate intentを登録し、exact publication確認後だけpublic-safeな
locatorを確定します。Decision CAS前にはexact intentを保存し、live Ref／reflog reconciliationとboundedな
selected-site checkoutの後だけterminal outcomeをcommitします。exact retryはidempotentです。restart後は
trusted configとjournal factsをimmutable object／一貫したlive Ref stateへ照合してfresh application authorityを
構築します。credential、admitted handle、approval、registration、permitはserialize／restoreせず、reviewerは
再authenticationと新しいapprovalが必要です。final publicationは引き続き`HumanDecisionRuntime`のfull
validationとCASを通ります。

このbindingはuntamperedなtrusted local configurationとjournal storageを前提としており、元のProposalが
特定のprocess runtime capability intersectionを通過したことのcryptographic evidenceではありません。
Core Ref／reflogとjournalのSQLite transactionは別なので、cross-database atomicityを主張せず、crash windowを
bounded reconciliationで解決します。Rust trusted workflow valueはgetter-onlyなprocess valueで、browserから
authorityとして渡すtransport DTOではありません。

これらのcapabilityはsource／workspace libraryに収録しています。配布する
3 binary（`synapse`、`synapse-local`、`synapse-present`）はこれらをHTTP、CLI、browser UIから
提供しません。background serviceによる自動resume、model invocation、generic browser editor、
durable identity／ACL storage、multi-process linearizability、production利用、配布許可も提供しません。
配布するCreator flowとlocalhost UIは引き続き画像専用で、そのpending review
authorityはsame-processかつrestart後にresumeできません。

`synapse-local` binaryにはbrowser import／review、専用diagnostics、bounded browser
`fsck`が含まれます。review authorityとmaintenance job stateはprocess-localで、restart後に
再開できません。

`synapse-present`は既存CASを変更せず、checkpoint済みで最大
512 MiBのRef SQLiteをprivate temporary copyへ取り込み、copy時とcopy後sourceのSHA-256一致を要求します。
SQLiteにはsource databaseを直接openさせません。sidecarまたはcopy中に変化するsourceは
`read_only_source_busy`で拒否します。
最大100 creator sessionsからGitHub-readyなlocal viewを生成できますが、GitHubへのupload／publish／
通信は行いません。private rationale、internal Actor ID、
repository path、raw assetは除外し、raw asset renderingは未実装です。public noteは別の
author-supplied textとして扱います。詳しくは[CLI reference](./cli_reference.md#synapse-present-companion-cli)を参照してください。

さらにsource／workspace libraryには、versioned generic-artifact projection／
local bundle APIも収録しています。このAPIは配布binary、HTTP、CLI、browser UIからは提供しません。
complete projectionは上記bounded Decision checkoutからのみ構築し、pending／incomplete projectionは
repository／authority identifierを含みません。canonical JSON、escaped Markdown、script-free HTML、
manifest、checksums、local Synapse／GitHub layoutをGit／network accessなしで生成します。remote
Synapse／GitHub adapter、Git import／provenance、identity mapping、GitHub App、hosted serviceは今後の
実装作業です。[#17](https://github.com/howlrs/synapsegit/issues/17)の設計範囲は完了しましたが、これらのremote surfaceは
提供していない。

[generic publication profile](../spec/application/generic-artifact-publication/v1/README.md)と
[integration roadmap](./generic_artifact_publication_roadmap.md)も参照してください。
