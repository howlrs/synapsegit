# Privacy and trust summary

[日本語の完全な security and trust model](./security_model.md)

This is the user-facing English summary for the v0.11.1 local preview. It
does not replace the [Japanese security model](./security_model.md), which is
the detailed technical reference.

## What SynapseGit can and cannot establish

SynapseGit records immutable objects, checked Ref history, and byte identity
under its draft profile. That can help verify that stored bytes and selected
history still match. It does not prove authorship, truth, copyright,
permission, a physical change, capture time, or that an AI model actually ran.
The Creator Pilot records a caller-supplied file as AI-attributed; it invokes
no model.

Image comparison is a conservative byte-identity baseline. Manual browser
comparison can help a person inspect two displayable images, but it performs
no pixel registration, difference analysis, EXIF processing, or visual/physical
claim verification.

## Private repository and archive data

The local repository and ordinary Core directory archive retain the private
Creator record: generation notes, decision rationale, decision pins, original
and derived source lineage, and raw local assets. Core does not encrypt local
Blobs or archives and does not restrict access by the operating-system user.
Protect the repository and archive with your own storage encryption, access,
retention, and distribution controls before they contain restricted data.

Archive integrity checks establish package byte and graph consistency. They do
not authenticate the sender, provide confidentiality, or recall copies that
have already left your control. Use a dedicated empty target for restore. The
browser restore UI fixes the target to the open registered project and still
requires explicit confirmation; it does not make an archive safe to trust.

## Public bundle boundary

`synapse-present` derives a local read-only bundle from a stable private Ref
copy. It excludes private rationale, internal Actor IDs, repository paths, and
raw asset bytes. It does not automatically decide whether sharing is allowed,
publish remotely, upload files, enforce training-use policy, or remove copies
after distribution.

A public bundle can retain OIDs, Refs, heads, fingerprints, and your
author-supplied public text. Those values can still be correlating information.
`--public` selects bundle visibility; it is not a publishing command. Review
the generated bundle and its intended recipient before external sharing. The
[public-text workflow](./presentation_sidecar.en.md) explains the separate
author-text form and export command.

## `synapse-local` is loopback-only, not user authentication

The current image application is a single-user local preview. It binds only
to literal IPv4 `127.0.0.1`; it is not a hosted or multi-user service. Its
request boundary checks the exact Host, unsafe-request Origin and Fetch
Metadata, uses a process-local custom-header browser token, disables CORS, and
sets CSP and `nosniff` protections.

The browser token helps prevent ambient browser requests. It is not proof of
identity and does not protect against another process running as the same OS
user, a malicious browser extension, or same-origin XSS. Do not expose the
service through a proxy, bind it to a network interface, or treat it as an
access-control boundary. Public HTTP transport, TLS, durable identity and
ACLs, multi-user authorization, and media sandboxing are not implemented.

## Operating limits that matter to a first evaluation

The local service accepts three Creator input files with a maximum of 64 MiB
each and 192 MiB together. Its browser `fsck`, archive listing, archive export,
and archive restore use server-fixed bounded limits; request input cannot raise
them. A completed operation can still report an unclean repository. The UI
does not expose object IDs, paths, or issue details from such maintenance
reports.

For full boundaries, threat-model details, resource ceilings, archive behavior,
and known limitations, consult the [Japanese security model](./security_model.md)
and the [local application architecture (Japanese)](./localhost_application_architecture.md).
