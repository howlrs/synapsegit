# SynapseGit GitHub配布ガイド

Audience: maintainer、release担当、公開文書を更新するcontributor
Status: Stage 0運用runbook
Applies to: v0.3.x
Last verified: 2026-07-18

この文書は、SynapseGitを「GitHub上で見つける」「現在の用途を判断する」「安全に試す」までの
公開導線とrelease手順を定義する。protocolの規範仕様ではない。

## 配布の対象

現在のprimary audienceは、Linux CLIを扱えるtechnical creator、creative provenance／
human-in-the-loop AIを評価する研究者・tool builder、Rust developerである。画家、建築家、
施工・修復担当、デザイナーは将来の対象だが、capture、visual diff、restart-durable review、
production serviceが必要な一般導入にはまだ適さない。v0.3.0のlocalhost UIはboundedな
三file import、same-process Human review、read-only diagnostics、確認付きbackground `fsck`に
限ってwrite-capable／maintenance-capableである。

公開文面では、将来の利用構想とv0.3で実行できる能力を同じものとして表示しない。

## 公開surface

| Surface | 役割 | 正本 |
|---|---|---|
| GitHub About | 検索結果で用途を伝える | この文書のmetadata節 |
| Root README | 60秒で対象・価値・試し方・限界を判断する | [`README.md`](../README.md) |
| 日本語README | 日本語利用者の同等入口 | [`README.ja.md`](../README.ja.md) |
| GitHub Release | 固定versionのbinary、checksum、release notes | `docs/releases/vX.Y.Z.md`とtag workflow |
| Local PublicationBundle | 作者外の人／AIが読むderived JSON、Markdown、static HTML | `synapse-publication`のcanonical projectionとlocal generator |
| Documentation index | 評価・実装・運用資料を探す | [`docs/README.md`](./README.md) |
| Security / Support | 非公開報告と通常問い合わせを分離する | [`SECURITY.md`](../SECURITY.md)、[`SUPPORT.md`](../SUPPORT.md) |
| Issues / Pull requests | 再現可能なfeedbackと変更を受ける | `.github` templates |

Stage 0ではcrates.io、GHCR、Homebrew、OS package repositoryを配布channelにしない。現行
Dockerfileはprivate GCP Core CLI smoke専用であり、end-user imageとして紹介しない。今回の
`synapse-present`追加でもDocker imageは変更せず、publication bundleやremote publishの配布経路にしない。

## GitHub About metadata

推奨description:

> Git-like, local-first Rust CLI and viewer for creative-work provenance: files, observations, AI proposals, evidence, and human decisions.

推奨topics:

```text
rust
cli
local-first
content-addressable-storage
data-provenance
creative-tools
human-in-the-loop
digital-preservation
json-schema
sqlite
```

`git`、`image-diff`、`cloud-service`は、互換性または未実装機能を誤認させるため現時点では
付けない。Topicsは機能追加時に増やすのではなく、公開利用者が実際に辿れる用途に合わせる。

外部project siteができるまではWebsite欄を空のままにする。prereleaseはGitHubの
`/releases/latest`対象外なので、汎用release導線には
`https://github.com/howlrs/synapsegit/releases`を使い、install commandにはversion固定URLを使う。

## Social Preview

repository内の候補画像は[`docs/assets/social-preview.png`](./assets/social-preview.png)とする。
GitHub SettingsのSocial previewへ明示的にuploadしない限り、repositoryへ置くだけでは反映されない。

公開前に次を確認する。

- 1280 × 640相当の2:1 landscapeで、1 MB未満
- `SynapseGit`、短い価値提案、`Stage 0 preview`以外の細かな文を詰め込まない
- 実装済みUIのように見える架空画面を使わない
- mobile share cardでも名称が読める
- dark/light backgroundの両方で主要文字が読める

## Release channelと対応platform

| Channel | Support | Notes |
|---|---|---|
| Linux x86_64 GNU archive | Preview support | Ubuntu 22.04 build、glibc 2.34+ |
| Tagged source build | Best-effort preview | Rust 1.88+、対応Unix-like host |
| Windows | Unsupported | atomic archive publication path未対応 |
| macOS / Linux ARM64 prebuilt | Not published | release pipelineで未検証 |
| Public cloud / container service | Not published | architectureまたはprivate smokeのみ |

新しいplatformは、buildが通るだけで配布対象にしない。tag workflowでtest、binary smoke、archive
展開後smokeを実行でき、security boundaryと制限をrelease notesへ記述してから追加する。

## Release asset構成

v0.3.0 archiveは`synapse`、`synapse-local`、`synapse-present`の三binaryを含む。
公開済みv0.2.0 archiveは`synapse`と`synapse-local`の二binaryだけを含み、後から内容を変更しない。
公開済みv0.1.0 archiveは`SECURITY.md`と`CHANGELOG.md`追加前に作られたため、binary二つと
release notesの`README.md`だけを含む。

```text
synapsegit-vX.Y.Z-TARGET/
  synapse
  synapse-local
  synapse-present
  README.md
  SECURITY.md
  CHANGELOG.md
  LICENSE
  THIRD_PARTY_NOTICES.md
```

Releaseにはarchive、全archiveを列挙した`SHA256SUMS`、tag-pinned release notesを置く。
更新後のworkflowで作るrelease archiveにはGitHub artifact attestationを生成する。checksumは
同じRelease上のbyteとの一致、attestationはGitHub Actions buildとの来歴を確認するもので、
softwareが安全であることやownerの法的意思を代替しない。

## Release gate

tagをpushする前に、次を満たす。

1. root `LICENSE`、Cargo `license-file` metadata、README、archiveの条件が一致し、license verifierを通る。
2. 全crate version、`docs/releases/vX.Y.Z.md`、`CHANGELOG.md`を更新する。
3. root READMEと日本語READMEのversion、platform、boundaryを更新する。
4. `docs/project_status.md`とcapability tableを更新する。
5. 次の検証をclean checkoutで実行する。

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
node scripts/verify_core_fixtures.mjs
node scripts/verify_local_api.mjs
node scripts/verify_license.mjs
node scripts/generate_third_party_notices.mjs --check
node scripts/verify_docs.mjs
node scripts/verify_mermaid.mjs
node scripts/manage_github_security.mjs --validate
git diff --check
```

6. release tagはversion commitを指すannotated tagとして作る。署名運用を導入した後はsigned tagを必須にする。
7. tag workflowがdraft prereleaseを作り、asset upload、checksum、attestation、公開まで成功したことを確認する。
8. 別directoryへassetをdownloadし、checksum、attestation、三binaryの`--version`／`--help`、3-file Pilot、
   read-only local publication bundleのexport／previewを確認する。

## 公開後check

- Release URLをsign-out状態で開ける
- READMEのversion固定download URLが200を返す
- archive内READMEとonline release notesのboundaryが一致する
- `SHA256SUMS`が全archiveを過不足なく列挙する
- GitHub Actionsのtag runが対象commitをbuildしている
- About description/topicsとSocial Previewが反映される
- Security Advisoriesに`Report a vulnerability`が表示される
- Community ProfileでREADME、CONTRIBUTING、Issue template、PR templateを検出する
- `docs/usage_guide.md`やpresentation guideにprivate repository等の古い状態が残っていない

## License policy

Copyright holderはhowlrsとK-Terashimaである。SynapseGitには独自の
[`SynapseGit Source-Available License 1.0`](../LICENSE)を適用し、OSI承認のopen-source
licenseとして表示しない。

許可する範囲は、GitHubでの閲覧とFork、Fork内のsource改変、GitHub-hosted CI、upstreamへの
Pull Request、および非商用評価またはFork／PR準備のための管理下環境でのclone、build、実行、
testである。commercial／production／hosted利用、GitHub Fork以外でのsource・binary再配布、
Release／Package／container／mirrorの公開には別途書面の許可を必要とする。正確な定義と条件は
root `LICENSE`を正本とする。元のarchiveへ`LICENSE`を収録していないv0.1.0も適用対象である。

独自licenseに架空のSPDX identifierを割り当てない。Cargoは
`[workspace.package] license-file = "LICENSE"`と各crateのinheritanceを使う。GitHubの
license detectorが`Other`または未検出と表示しても、OSI license名へ置き換えない。

license変更時は少なくとも次を同じPull Requestで更新する。

- root `LICENSE`と日本語概要
- `Cargo.lock`から生成した`THIRD_PARTY_NOTICES.md`
- Cargo `license-file` metadataと全crateのinheritance
- release archiveの`LICENSE`
- README、install guide、release notes、contribution条件
- `scripts/verify_license.mjs`の期待値

## 関連資料

- [Installation](./install.md)
- [Project status](./project_status.md)
- [Release notes](./releases/v0.3.0.md)
- [Security model](./security_model.md)
- [Contributing](../CONTRIBUTING.md)
- [Documentation index](./README.md)
