# lkit 换源多发行版 E2E 的构建器：只产出生产二进制，供各发行版容器挂载。
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa

FROM ${RUST_IMAGE} AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --locked --release -p lkit-cli --bin lkit \
    && mkdir -p /output \
    && cp target/release/lkit /output/lkit
