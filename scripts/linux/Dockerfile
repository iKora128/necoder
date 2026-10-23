FROM ubuntu:24.04

ARG RUST_VERSION
ENV CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:$PATH

COPY install-deps.sh /tmp/install-deps.sh
RUN bash /tmp/install-deps.sh && rm -rf /var/lib/apt/lists/* /tmp/install-deps.sh
RUN test -n "$RUST_VERSION" \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --profile minimal --default-toolchain "$RUST_VERSION" \
        --component rustfmt,clippy \
    && rm /tmp/rustup.sh

# read-only の作業ツリーでも実履歴を読むテストを実行できるようにする。
RUN git config --global --add safe.directory /workspace
WORKDIR /workspace
CMD ["bash"]
