# Release read-compatibility fixtures

Each directory holds a repository and a directory archive written by the
published `synapse` binary of that release. The current CLI must keep reading
them (Issue #165): `every_published_release_repository_and_archive_stays_readable`
in `crates/synapse-cli/tests/cli.rs` runs `fsck`, `creator-report`, and
`creator-list` on a copy of each repository, restores each archive, and
requires the restored report to equal the repository's report.

Every fixture records one deferred session, `release-fixture`, from the same
synthetic files, Subject label, creator name, and rationale. Releases that
support `--generation-note-file` also record a synthetic generation note.
They contain no real creator data.

Git does not store empty directories, so the test recreates each repository's
empty `cas/tmp` directory after copying it. Do not edit these bytes; the
archives are checksum-bound and the repositories are the release's own output.

After each release, add its fixture before the next version bump:

```bash
gh release download v0.0.0 --repo howlrs/synapsegit \
  --pattern 'synapsegit-v0.0.0-x86_64-unknown-linux-gnu.tar.gz' --pattern SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
bash scripts/generate_release_fixture.sh v0.0.0 synapsegit-v0.0.0-x86_64-unknown-linux-gnu.tar.gz
```

Replace `v0.0.0` with the release tag, then add its row below.
`scripts/verify_release_fixtures.mjs` requires a fixture and a row for every
annotated release tag below the current `synapse-cli` version.

| Release | Release archive SHA-256 | Generation note |
|---|---|---|
| `v0.1.0` | `15edef03fa9c4204f5326aec4ab33e3834f998fb4671d9e93a3af7353c4f83b3` | no |
| `v0.2.0` | `2a4332b3770429e30c43d38668270ba7a17b87964e471f2ac65e49987691a4fd` | no |
| `v0.3.0` | `13644d6640044f05076a51a1220779595329fd6841fc9049042df4065467442f` | no |
| `v0.4.0` | `7a221fa81f5603522deff0a210e4a3e161b79233a3083cfa63b0cbc9c332b16b` | no |
| `v0.5.0` | `9b9ba5b2cc3a1ec10c1bafc7c2405577095b527744914e93d8e2f32c04d566ee` | no |
| `v0.5.1` | `dea490377c1d192da990d9c895ccd2956bfdfcc91f5476bc8b494fb4ef7d3a20` | no |
| `v0.6.0` | `5ab6f3c1236d02844f3985588c5560f07092e0355e72e542c7704158fe7efd6e` | no |
| `v0.7.0` | `740745544631d97b8d27e8586dcefdbb2997b15e65955eb49b85b86c5ee0bb3e` | no |
| `v0.8.0` | `5cc94c0fc11dc24e9371fef0ec096726d2acf90d4ff8fe14063b826eda9b7490` | no |
| `v0.8.1` | `a90cfee3ee9b00b12f627dd39c6557cef13f9576696a422cf30a6220a2a20557` | no |
| `v0.9.0` | `ad3f98b19bb1725fd35e6cfff3278f302812ef22fad508cb8a446418c403f0c6` | no |
| `v0.10.0` | `809754518381612ed19ca3c219d8f518556e4d3beba5c15b39ff4ccbddb6da17` | no |
| `v0.11.0` | `35d7231be64f4171040de02c402aaed66f667391056206fafa22c90b961a7a69` | yes |
| `v0.11.1` | `e14db026b849b4fdc21a78fbb07cf98f8b8f31ca33748df6bf8c2b2cdc4ccce7` | yes |
| `v0.12.0` | `9ca579daacc56fcd901b3c36c0dead5940680177ac97c3d91812150543f1faac` | yes |
| `v0.13.0` | `4a752381702b10d7525345b0826ebeb4ce82a7f5d69cccd93088395f556dda91` | yes |
| `v0.13.1` | `1c26af1d712e933465233720fcb26f87d09fb7356ee85b231e679f38f8e2c50a` | yes |
| `v1.0.0-rc.1` | `b8b90ef1b9d34490f04f230e0d54cf3c50f834737d473ca83a9d9db64bef7b93` | yes |
| `v1.0.0` | `060840bd93300ed2741cbb9ff8b3150852b37e953f8afd1c0c5e413a000216db` | yes |
| `v1.0.1` | `8130b5358d45bbcd793bc1757355908edd2b570ee22b730e4a00a147e94d76aa` | yes |
| `v1.1.0` | `7b2b53f9b69a70ad185d206dead88b0235e0d14483edba2aa1dae5e7b1bc3387` | yes |
