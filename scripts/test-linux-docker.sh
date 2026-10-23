#!/usr/bin/env bash
# Mac の target とユーザー設定を共有せず、Ubuntu 上で Linux Desktop 本体を検証する。
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
mode="${1:-all}"
case "$mode" in
    check|build|test|all|shell) ;;
    -h|--help)
        cat <<'EOF'
Usage: bash scripts/test-linux-docker.sh [check|build|test|all|shell]
  check : cargo check --locked --workspace --all-targets
  build : cargo build --locked -p necoder --bin necoder
  test  : cargo test --locked --workspace (no Desktop session)
  all   : check, build, test (default)
  shell : disposable interactive Ubuntu shell

Environment:
  NECODER_LINUX_PLATFORM    linux/arm64 or linux/amd64 (Docker engine architecture)
  NECODER_LINUX_JOBS        Cargo build jobs (default: 4)
  NECODER_LINUX_TEST_THREADS  Test threads (default: 2)

Logs: target/linux-port/ on the host. Sources are mounted read-only.
Cargo downloads and Linux target are kept in dedicated Docker volumes.
GUI startup, IME and GPU behavior must be checked separately in a Desktop VM.
EOF
        exit 0
        ;;
    *) echo "Unknown mode: $mode (use --help)" >&2; exit 2 ;;
esac

command -v docker >/dev/null || { echo "Docker is required." >&2; exit 1; }
docker_arch="$(docker info --format '{{.Architecture}}')"
case "$docker_arch" in
    aarch64|arm64) default_platform=linux/arm64 ;;
    x86_64|amd64) default_platform=linux/amd64 ;;
    *) echo "Unsupported Docker architecture: $docker_arch" >&2; exit 1 ;;
esac
platform="${NECODER_LINUX_PLATFORM:-$default_platform}"
case "$platform" in
    linux/arm64|linux/amd64) ;;
    *) echo "Unsupported platform: $platform" >&2; exit 2 ;;
esac
architecture="${platform#linux/}"
rust_version="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$repo_root/rust-toolchain.toml")"
[[ -n "$rust_version" ]] || { echo "Missing Rust toolchain version." >&2; exit 1; }
image="necoder-linux-build:ubuntu24.04-$architecture-rust$rust_version"
cache_prefix="necoder-linux-ubuntu24-$architecture"
log_dir="$repo_root/target/linux-port"
mkdir -p "$log_dir"
log_file="$log_dir/$(date -u +%Y%m%dT%H%M%SZ)-$architecture-$mode.log"

run_validation() {
    echo "Platform: $platform; Rust: $rust_version; mode: $mode"
    echo "Revision: $(git -C "$repo_root" rev-parse HEAD) (including working-tree changes)"
    docker build --platform "$platform" \
        --build-arg "RUST_VERSION=$rust_version" \
        --tag "$image" "$repo_root/scripts/linux"

    local terminal=(--init)
    if [[ "$mode" == shell ]]; then
        terminal+=(-it)
    fi
    docker run --rm "${terminal[@]}" --platform "$platform" \
        --mount "type=bind,source=$repo_root,target=/workspace,readonly" \
        --mount "type=volume,source=$cache_prefix-target,target=/build" \
        --mount "type=volume,source=$cache_prefix-registry,target=/opt/cargo/registry" \
        --mount "type=volume,source=$cache_prefix-git,target=/opt/cargo/git" \
        --env CARGO_TARGET_DIR=/build \
        --env "CARGO_BUILD_JOBS=${NECODER_LINUX_JOBS:-4}" \
        --env "RUST_TEST_THREADS=${NECODER_LINUX_TEST_THREADS:-2}" \
        "$image" bash -euo pipefail -c '
            rustc --version
            uname -m
            case "$1" in
                check) cargo check --locked --workspace --all-targets ;;
                build) cargo build --locked -p necoder --bin necoder ;;
                test) cargo test --locked --workspace ;;
                all)
                    cargo check --locked --workspace --all-targets
                    cargo build --locked -p necoder --bin necoder
                    cargo test --locked --workspace
                    ;;
                shell) exec bash ;;
            esac
        ' bash "$mode"
}

if [[ "$mode" == shell ]]; then
    run_validation
else
    echo "Log: $log_file"
    run_validation 2>&1 | tee "$log_file"
fi
