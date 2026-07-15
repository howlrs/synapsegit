# SynapseGit support

SynapseGit is an experimental Stage 0 project maintained on a best-effort basis.
There is no commercial support or response-time guarantee.

## Choose the right channel

| Need | Channel |
|---|---|
| Reproducible defect | [Bug report](https://github.com/howlrs/synapsegit/issues/new?template=bug_report.yml) |
| Proposed use case or capability | [Feature request](https://github.com/howlrs/synapsegit/issues/new?template=feature_request.yml) |
| Usage or evaluation question | [Question](https://github.com/howlrs/synapsegit/issues/new?template=question.yml) |
| Suspected vulnerability | [Private vulnerability report](https://github.com/howlrs/synapsegit/security/advisories/new) |

Search existing Issues and the [FAQ](./docs/faq.md) first. For commands and
stable error codes, read the [CLI reference](./docs/cli_reference.md).

## What to include

- SynapseGit version (`synapse --version` or `synapse-local --version`)
- installation route and operating system
- exact command or documented workflow
- expected and actual behavior
- minimal reproduction using synthetic files
- relevant output with paths and identifiers redacted

Do not attach real creator images, repository archives, credentials, tokens,
private paths, or cloud identifiers. A checksum or OID may still correlate with
private material; redact it when it is not required to reproduce the problem.

## Support boundary

The current supported evaluation path is the local CLI and loopback-only
application described in [README.md](./README.md). Public reverse-proxy deployment,
multi-user hosting, production cloud operation, image-difference interpretation,
and recovery from manually edited object or SQLite files are outside the
current support scope.

For development contributions, use [CONTRIBUTING.md](./CONTRIBUTING.md).
