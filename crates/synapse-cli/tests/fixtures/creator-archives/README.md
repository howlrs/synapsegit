# Creator archive fixtures

These directory archives are restored by `crates/synapse-cli/tests/cli.rs` to
check that the current CLI reads Creator sessions recorded by earlier builds
(Issue #132). Each archive holds one deferred session made from the same three
synthetic files, a synthetic rationale, and, for v0.11.0, a synthetic
generation note. They contain no real creator data.

| Directory | Written by | Session | Implementation OID |
|---|---|---|---|
| `v0.1.0` | `synapse` from the published `synapsegit-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` (archive SHA-256 `15edef03fa9c4204f5326aec4ab33e3834f998fb4671d9e93a3af7353c4f83b3`) | `release-fixture` | `blob:sg-oid-v1:sha256:cce835384026b51df3029211baab744540f2f3d12f12c52261ebb6d45a30eaa5` |
| `v0.11.0` | `synapse` from the published `synapsegit-v0.11.0-x86_64-unknown-linux-gnu.tar.gz` (archive SHA-256 `35d7231be64f4171040de02c402aaed66f667391056206fafa22c90b961a7a69`) | `release-fixture` | `blob:sg-oid-v1:sha256:4490a6a014bbe51b87927cd5be47d5b4aa882ecf1234b6afd1c981db8a398ca8` |
| `unreleased-source-build` | a debug build of v0.13.1 source with one comment line appended to `crates/synapse-observation/Cargo.toml` | `unreleased-fixture` | `blob:sg-oid-v1:sha256:d75f818f273203d939df7c41e66897093d7076888a8df56a491332a815cfccd2` |

The first two archives exercise the oldest and newest releases whose sessions
v0.13.1 rejected. The third stands for any build whose Observation bundle
matches no published release, such as a newer release opened by an older
binary. Its session is otherwise consistent, so the report is refused with
`creator_implementation_unrecognized` rather than `creator_report_invalid`.

Do not edit these bytes. `manifest.sha256` and each object checksum bind them;
regenerate an archive with the named binary instead.
