# SynapseGit license概要（日本語）

Status: 非規範的な日本語要約
Authoritative text: [`LICENSE`](../LICENSE)
Last updated: 2026-07-15

SynapseGitの権利者は **howlrs** と **K-Terashima** です。適用条件の正式な正本は英語の
`SynapseGit Source-Available License 1.0`であり、この文書は理解を助けるための要約です。
食い違いがある場合はroot [`LICENSE`](../LICENSE)を優先します。
この条件は2026-07-15から提供され、元のarchiveに`LICENSE`を収録していないv0.1.0も対象です。

## 許可されること

- GitHub上でsourceを閲覧する
- GitHubのFork機能でpublic Forkを作る
- そのForkの開発・維持または非商用評価のため、管理下の環境でclone、build、実行、test、改変する
- そのForkのbuildとtestにGitHub-hosted automationを利用する
- 改変をForkで公開し、upstreamへPull Requestを送る
- 個人、教育、学術研究、または非productionの社内評価を行う

評価利用は非商用・非productionに限られます。licenseとcopyright表示を保持し、重要な改変を
行った場合はmodified versionであることを示してください。営利組織も、許可されたFork／PRへの
協力または条件を満たす社内評価に限って利用できます。

## 別途許諾が必要なこと

- production利用、商用利用、収益を得る活動
- 販売、貸与、sublicenseその他の商業的利用
- GitHub Fork／Pull Request以外でsourceやbinaryを第三者へ再配布する
- GitHub Release、Package、container image、downloadable artifact、mirrorとして公開する
- hosted／managed／SaaSとして提供する
- SynapseGitまたは権利者の名称、logo、trademarkを利用する

上記を希望する場合は、権利者から書面による別許諾を得る必要があります。

## Contribution

別の書面合意がない限り、公式repositoryへ意図的に提出するcontributorは、提出する権利を
持つことを表明し、howlrsとK-Terashimaへ共同で、commercial利用とrelicenseを含む全目的で
contributionを利用できる、永続的・取消不能・世界的・無償・非独占・譲渡可能・sublicense可能な
copyright licenseを付与します。transfer、sublicense、relicenseには両権利者の書面合意が必要です。
これはcopyrightの譲渡ではなく、contributorは自分のcontributionのcopyrightを保持します。
この広い許諾は、Forkで改変を公開しただけでは発生せず、公式repositoryへの取り込みを意図して
提出した場合に発生します。Pull Requestの提出は採用を保証しません。

## Licenseの性質

これはsource-available licenseであり、OSI承認のopen-source licenseではありません。
Softwareは無保証で提供されます。完全な条件、termination、免責、責任制限は
[`LICENSE`](../LICENSE)を確認してください。Rust依存componentはSynapseGit独自licenseの
対象外であり、[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md)に収録した各条件が適用されます。
