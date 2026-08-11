# Cadmus Cursor Cloud base image — toolchain only (no project COPY).
# Checkout-dependent setup runs via .cursor/cloud-install.sh on each agent boot.

FROM ubuntu:26.04

ENV DEBIAN_FRONTEND=noninteractive

# Renovate-tracked version pins (same names as CI).
ENV MDBOOK_VERSION=0.5.4
ENV MDBOOK_EPUB_REV=21a1c8134134201a2d555313447c96e56e2a8996
ENV MDBOOK_MERMAID_VERSION=0.17.0
ENV NEXTEST_VERSION=0.9.140
ENV NODE_VERSION=24

ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV SQLX_OFFLINE=true
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV DISPLAY=:1
ENV NVM_DIR=/home/ubuntu/.nvm
ENV PATH="/home/ubuntu/.local/bin:/home/ubuntu/linaro-toolchain/bin:/usr/local/cargo/bin:${PATH}"

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        autoconf \
        automake \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        curl \
        gettext \
        git \
        gperf \
        jq \
        libasound2t64 \
        libatk-bridge2.0-0t64 \
        libatk1.0-0t64 \
        libbz2-dev \
        libclang-dev \
        libdjvulibre-dev \
        libfreetype6-dev \
        libgbm1 \
        libgumbo-dev \
        libharfbuzz-dev \
        libjbig2dec0-dev \
        libjpeg-dev \
        libnspr4 \
        libnss3 \
        libopenjp2-7-dev \
        libpng-dev \
        libtool \
        libxcomposite1 \
        libxdamage1 \
        libxfixes3 \
        libxkbcommon0 \
        libxrandr2 \
        libsdl2-dev \
        llvm-dev \
        meson \
        ninja-build \
        patchelf \
        pkg-config \
        python3 \
        tcl \
        unzip \
        wget \
        zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

ENV LIBCLANG_PATH=/usr/lib/llvm-18/lib

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --default-toolchain stable \
    && rustup target add arm-unknown-linux-gnueabihf \
    && rustup component add rustfmt clippy llvm-tools-preview

RUN mkdir -p /home/ubuntu/linaro-toolchain \
    && cd /home/ubuntu/linaro-toolchain \
    && wget -q https://developer.arm.com/-/cdn-downloads/permalink/legacy-linaro-gnu-toolchains/4.9-2017.01/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz \
    && tar xf gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz --strip-components=1 \
    && rm gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz

RUN mkdir -p /home/ubuntu/.cache/mdbook /home/ubuntu/.local/bin \
    && curl -fsSL "https://github.com/rust-lang/mdBook/releases/download/v${MDBOOK_VERSION}/mdbook-v${MDBOOK_VERSION}-x86_64-unknown-linux-gnu.tar.gz" \
        | tar xz -C /home/ubuntu/.cache/mdbook \
    && ln -sf /home/ubuntu/.cache/mdbook/mdbook /home/ubuntu/.local/bin/mdbook

RUN mkdir -p /home/ubuntu/.cache/mdbook-epub /home/ubuntu/.local/bin \
    && tmp="$(mktemp -d)" \
    && curl -fsSL "https://github.com/Michael-F-Bryan/mdbook-epub/archive/${MDBOOK_EPUB_REV}.tar.gz" \
        | tar xz -C "$tmp" \
    && cargo install --path "$tmp/mdbook-epub-${MDBOOK_EPUB_REV}" --locked \
        --root /home/ubuntu/.cache/mdbook-epub \
    && echo "${MDBOOK_EPUB_REV}" > /home/ubuntu/.cache/mdbook-epub/.rev \
    && ln -sf /home/ubuntu/.cache/mdbook-epub/bin/mdbook-epub /home/ubuntu/.local/bin/mdbook-epub \
    && rm -rf "$tmp"

RUN mkdir -p /home/ubuntu/.cache/mdbook-mermaid /home/ubuntu/.local/bin \
    && cargo install mdbook-mermaid --version "${MDBOOK_MERMAID_VERSION}" \
        --root /home/ubuntu/.cache/mdbook-mermaid \
    && echo "${MDBOOK_MERMAID_VERSION}" > /home/ubuntu/.cache/mdbook-mermaid/.version \
    && ln -sf /home/ubuntu/.cache/mdbook-mermaid/bin/mdbook-mermaid /home/ubuntu/.local/bin/mdbook-mermaid

RUN cargo install cargo-nextest --version "${NEXTEST_VERSION}" --locked \
    && ln -sf /usr/local/cargo/bin/cargo-nextest /home/ubuntu/.local/bin/cargo-nextest

RUN export HOME=/home/ubuntu NVM_DIR=/home/ubuntu/.nvm \
    && curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.2/install.sh | bash \
    && . /home/ubuntu/.nvm/nvm.sh \
    && nvm install "${NODE_VERSION}" \
    && nvm alias default "${NODE_VERSION}" \
    && node_path="$(nvm which default)" \
    && ln -sf "$node_path" /home/ubuntu/.local/bin/node \
    && ln -sf "$(dirname "$node_path")/npm" /home/ubuntu/.local/bin/npm \
    && ln -sf "$(dirname "$node_path")/npx" /home/ubuntu/.local/bin/npx

RUN chown -R ubuntu:ubuntu /home/ubuntu

WORKDIR /workspace
