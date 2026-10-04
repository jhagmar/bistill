# Toolchain image for the GitHub required checks. Source is bind-mounted at
# run; this file only pins the runner.
#
# Ubuntu 24.04 matches ubuntu-latest. Rust 1.85.0 is the MSRV and the
# toolchain .github/workflows/ci.yml selects. CodeQL bundle 2.27.0 matches
# github/codeql-action v4.38.2. reuse 6.2.0 matches fsfe/reuse-action v6
# (image fsfe/reuse:6). cargo-deny 0.20.2 matches the CI install.

FROM ubuntu:24.04 AS ci

ARG RUST_VERSION=1.85.0
ARG CARGO_DENY=0.20.2
ARG REUSE=6.2.0

ENV DEBIAN_FRONTEND=noninteractive
ENV RUSTUP_HOME=/opt/rustup
ENV CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ENV CARGO_TERM_COLOR=always
ENV CARGO_INCREMENTAL=0
ENV RUSTUP_TOOLCHAIN=1.85.0

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        curl \
        file \
        git \
        libclang-dev \
        libssl-dev \
        pkg-config \
        python3 \
        python3-pip \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
        -y \
        --default-toolchain "${RUST_VERSION}" \
        --profile minimal \
        --component rustfmt,clippy,llvm-tools-preview \
        --no-modify-path

RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported dpkg architecture"; exit 1 ;; \
    esac; \
    curl --proto '=https' --tlsv1.2 -fsSL \
        "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY}/cargo-deny-${CARGO_DENY}-${target}.tar.gz" \
        | tar -xz -C /tmp; \
    install -m 755 "/tmp/cargo-deny-${CARGO_DENY}-${target}/cargo-deny" /usr/local/bin/cargo-deny; \
    rm -rf "/tmp/cargo-deny-${CARGO_DENY}-${target}"

RUN pip3 install --break-system-packages --no-cache-dir "reuse[charset-normalizer]==${REUSE}"

WORKDIR /src
CMD ["/src/ci/entrypoint.sh"]

FROM ci AS codeql

ARG CODEQL_BUNDLE=2.27.0

RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) bundle=codeql-bundle-linux64.tar.gz ;; \
      arm64) bundle=codeql-bundle-linux-arm64.tar.gz ;; \
      *) echo "unsupported dpkg architecture"; exit 1 ;; \
    esac; \
    curl --proto '=https' --tlsv1.2 -fsSL \
        "https://github.com/github/codeql-action/releases/download/codeql-bundle-v${CODEQL_BUNDLE}/${bundle}" \
        | tar -xz -C /opt

ENV PATH=/opt/codeql:$PATH
CMD ["/src/ci/codeql.sh"]
