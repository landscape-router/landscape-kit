ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa
ARG DEBIAN_IMAGE=debian:12.11-slim@sha256:b1a741487078b369e78119849663d7f1a5341ef2768798f7b7406c4240f86aef

FROM ${RUST_IMAGE} AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rustfmt.toml rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --locked --profile dist --features test-support -p lkit-cli --bin lkit \
    && cargo build --locked --release -p lkit-publish --bin lkit-publish \
    && cargo build --locked --release -p lkit-test-fixture \
       --bin lkit-fixture-release --bin landscape-webserver --bin lkit-test-systemctl
RUN mkdir -p /output/bin \
    && cp target/dist/lkit /output/bin/ \
    && cp target/release/lkit-publish \
       target/release/lkit-fixture-release target/release/landscape-webserver \
       target/release/lkit-test-systemctl /output/bin/

FROM ${DEBIAN_IMAGE}
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       ca-certificates curl iproute2 procps python3 util-linux \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /output/bin/ /usr/local/bin/
COPY scripts/docker-e2e/run-service.sh scripts/docker-e2e/run-scenarios.sh scripts/docker-e2e/rustfs-proxy.py scripts/lib/rustfs-test.sh /usr/local/lib/lkit-e2e/
RUN chmod 0755 /usr/local/lib/lkit-e2e/run-service.sh /usr/local/lib/lkit-e2e/run-scenarios.sh /usr/local/lib/lkit-e2e/rustfs-test.sh
CMD ["/usr/local/lib/lkit-e2e/run-service.sh"]
