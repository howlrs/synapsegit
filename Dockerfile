# syntax=docker/dockerfile:1.7

FROM rust:1.95.0-slim-bookworm@sha256:d7482085ff5b415f84dba5647ae71606650bdef00db7aeb69f4b3d170c3e4082 AS builder

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY spec/core/v0.1/schemas ./spec/core/v0.1/schemas

RUN cargo build --locked --release --package synapse-cli --bin synapse

FROM debian:bookworm-slim@sha256:60eac759739651111db372c07be67863818726f754804b8707c90979bda511df AS runtime

LABEL org.opencontainers.image.title="SynapseGit CLI smoke job" \
      org.opencontainers.image.description="Non-production Cloud Run Job smoke test for the SynapseGit Stage 0 CLI" \
      org.opencontainers.image.source="https://github.com/howlrs/synapsegit"

COPY --from=builder --chown=0:0 /src/target/release/synapse /usr/local/bin/synapse
COPY --chown=0:0 deploy/gcp/smoke/smoke.sh /usr/local/bin/synapse-smoke
RUN chmod 0555 /usr/local/bin/synapse /usr/local/bin/synapse-smoke
RUN mkdir -p /opt/synapse/smoke-fixtures
COPY --chown=0:0 \
    deploy/gcp/smoke/fixtures/original.txt \
    deploy/gcp/smoke/fixtures/current.txt \
    deploy/gcp/smoke/fixtures/ai-output.txt \
    /opt/synapse/smoke-fixtures/
RUN chmod 0444 /opt/synapse/smoke-fixtures/*.txt

ENV HOME=/tmp
WORKDIR /tmp
USER 65532:65532

ENTRYPOINT ["/usr/local/bin/synapse-smoke"]
