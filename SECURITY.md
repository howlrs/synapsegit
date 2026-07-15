# Security policy

SynapseGit is a Stage 0 preview. It is suitable for local evaluation, not for a
public, multi-user, or production deployment.

## Supported versions

| Version | Security handling |
|---|---|
| Latest v0.1.x prerelease | Best-effort investigation and fixes |
| Older prereleases | Upgrade may be required before a fix is provided |
| Unreleased `main` builds | Not a supported distribution |

There is no guaranteed response or remediation SLA. The maintainer will use
the private advisory thread to communicate triage status as capacity permits.

## Report a vulnerability privately

Use
[GitHub private vulnerability reporting](https://github.com/howlrs/synapsegit/security/advisories/new).
Do not open a public Issue with exploit details, sensitive paths, private data,
tokens, or unreleased findings.

Include as much of the following as is safe:

- affected SynapseGit version or commit;
- operating system and installation route;
- affected command, library boundary, or localhost route;
- impact and realistic attack preconditions;
- minimal reproduction steps or proof of concept;
- whether the issue is already public; and
- any suggested mitigation or coordinated-disclosure constraints.

Use synthetic data. Do not upload a real creator repository, private images,
credentials, cloud identifiers, or personal information.

## In scope

- object parsing, validation, canonicalization, OID, graph, Ref, and archive
  integrity failures;
- path traversal, unsafe file publication, restore, or local data exposure;
- violations of the documented loopback Host, Origin, token, or image-serving
  boundary in `synapse-local`;
- authorization or one-shot state violations in the implemented local AI and
  Human Decision application routes; and
- release workflow or artifact-integrity weaknesses that affect distributed
  binaries.

## Important current boundaries

- `synapse-local` must remain on `127.0.0.1`. Reverse-proxy or public exposure
  is unsupported.
- The local OS user and filesystem permissions are trusted. The browser token
  is process-local and is not multi-user authentication.
- The creator Pilot uses fixed local identities and caller-supplied candidate
  output. It does not authenticate a person or prove that a model generated a
  file.
- OIDs and checksums verify bytes under their stated profile. They do not prove
  authorship, truth, permission, confidentiality, or sender identity.
- Malicious-media decode isolation, HTTP/JWT/MFA, durable authorization,
  public-cloud tenancy, and a signed archive profile are not implemented.

Unsupported deployment reports are still useful when they reveal a defect in
an implemented boundary, but the project cannot treat a public deployment as a
supported configuration.

## Disclosure

Please allow time for private investigation and a fix before public disclosure.
When a report is confirmed, the maintainer may prepare a private fix, release
notes, and a GitHub Security Advisory, and will credit the reporter if requested
and appropriate.

For non-sensitive bugs and usage questions, follow [SUPPORT.md](./SUPPORT.md).
For the complete trust model, read
[docs/security_model.md](./docs/security_model.md).
