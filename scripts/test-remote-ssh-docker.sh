#!/bin/sh
set -eu

mode=test
if [ "${1:-}" = "--gui" ]; then
    mode=gui
    shift
fi

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture_dir="$repo_root/tests/fixtures/remote-ssh"
scratch_dir=$(mktemp -d "${TMPDIR:-/tmp}/necoder-remote-ssh.XXXXXX")
compose_project="necoder-remote-ssh-$$"

export NECODER_REMOTE_TEST_PUBLIC_KEY="$scratch_dir/id_ed25519.pub"

compose() {
    docker compose --project-name "$compose_project" --file "$fixture_dir/compose.yml" "$@"
}

cleanup() {
    compose down --volumes --remove-orphans >/dev/null 2>&1 || true
    rm -rf -- "$scratch_dir"
}
trap cleanup EXIT INT TERM

ssh-keygen -q -t ed25519 -N '' -f "$scratch_dir/id_ed25519"

echo "==> Building and starting the isolated SSH host"
compose up --detach --build --wait

published=$(compose port ssh 22 | tail -n 1)
ssh_port=${published##*:}
case "$ssh_port" in
    ''|*[!0-9]*)
        echo "could not determine the published SSH port: $published" >&2
        exit 1
        ;;
esac

known_hosts="$scratch_dir/known_hosts"
attempt=0
while ! ssh-keyscan -p "$ssh_port" 127.0.0.1 > "$known_hosts" 2>/dev/null; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 20 ]; then
        echo "SSH host did not become ready" >&2
        compose logs ssh >&2
        exit 1
    fi
    sleep 1
done

ssh_config="$scratch_dir/ssh_config"
cat > "$ssh_config" <<EOF
Host necoder-docker
    HostName 127.0.0.1
    Port $ssh_port
    User dev
    IdentityFile $scratch_dir/id_ed25519
    IdentitiesOnly yes
    UserKnownHostsFile $known_hosts
    StrictHostKeyChecking yes
    BatchMode yes
    LogLevel ERROR
EOF

remote_arch=$(compose exec -T ssh uname -m | tr -d '\r\n')
case "$remote_arch" in
    aarch64|arm64) target=aarch64-unknown-linux-musl ;;
    x86_64|amd64) target=x86_64-unknown-linux-musl ;;
    *)
        echo "unsupported Docker architecture: $remote_arch" >&2
        exit 1
        ;;
esac

if [ "${NECODER_REMOTE_SERVER_BINARY:-}" ]; then
    server_binary=$NECODER_REMOTE_SERVER_BINARY
else
    server_binary=''
    for candidate in \
        "$HOME/.local/share/necoder/remote/artifacts/$target/necoder-remote-server" \
        "$repo_root/target/$target/release/necoder-remote-server" \
        "$repo_root/target/$target/debug/necoder-remote-server"
    do
        if [ -f "$candidate" ]; then
            server_binary=$candidate
            break
        fi
    done
fi

# 同じ workspace version の途中で wire protocol / remote CLI が変わることもある。target 名だけで
# 拾った古い artifact を使うと、client が配備後の version 検査で落ちるので source より古ければ再生成。
if [ -f "$server_binary" ] && { [ "$repo_root/crates/host/src/host.rs" -nt "$server_binary" ] || [ "$repo_root/crates/host/Cargo.toml" -nt "$server_binary" ]; }; then
    echo "==> Cached $target artifact is older than host sources; rebuilding"
    server_binary=''
fi

# No artifact yet: build one in Docker. The Mac usually has no Linux cross toolchain,
# and the full workspace cannot be reused here because it depends on gpui from the zed git
# repository - a large clone that necoder-remote-server does not need. So assemble a tiny
# standalone workspace with just the two crates the server is made of.
if [ ! -f "$server_binary" ]; then
    echo "==> No $target artifact found; building necoder-remote-server in Docker"
    rust_channel=$(grep -m1 '^channel' "$repo_root/rust-toolchain.toml" | sed 's/.*"\(.*\)".*/\1/')
    # The client checks "<version> protocol=<n>" for an exact match, so the standalone
    # manifest must carry the same package version as the workspace.
    workspace_version=$(grep -m1 '^version = ' "$repo_root/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')
    build_dir="$scratch_dir/remote-server-build"
    mkdir -p "$build_dir/crates"
    cp -R "$repo_root/crates/host" "$build_dir/crates/host"
    cp -R "$repo_root/crates/paths" "$build_dir/crates/paths"
    rm -rf "$build_dir/crates/host/tests"
    # Version requirements are deliberately loose (semver-compatible with the workspace) so
    # they cannot drift. Adding a dependency to crates/host means adding it here too.
    cat > "$build_dir/Cargo.toml" <<EOF
[workspace]
resolver = "2"
members = ["crates/host", "crates/paths"]

[workspace.package]
version = "$workspace_version"
edition = "2021"
license = "AGPL-3.0-or-later"

[workspace.dependencies]
paths = { path = "crates/paths" }
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ignore = "0.4"
regex = "1"
url = "2"

[profile.release]
strip = true
EOF
    docker run --rm \
        --volume "$build_dir:/work" \
        --volume necoder-remote-server-cargo:/usr/local/cargo/registry \
        --workdir /work \
        "rust:$rust_channel-alpine" \
        sh -c 'apk add --no-cache musl-dev >/dev/null && cargo build -p host --bin necoder-remote-server --release'
    server_binary="$build_dir/target/release/necoder-remote-server"
fi

if [ ! -f "$server_binary" ]; then
    cat >&2 <<EOF
Linux remote-server artifact not found for $target, and the Docker build did not produce one.
Build it yourself, then rerun this script:

  cargo zigbuild -p host --bin necoder-remote-server --release --target $target

Or set NECODER_REMOTE_SERVER_BINARY to an existing $target binary.
EOF
    exit 1
fi

echo "==> SSH host: necoder-docker (127.0.0.1:$ssh_port, $target)"
cd "$repo_root"
if [ "$mode" = gui ]; then
    echo "==> Opening necoder for the SSH picker demo"
    echo "==> In necoder: + -> Remote/SSH -> necoder-docker"
    echo "==> Then browse to work/sample and open it as the project"
    echo "==> The SSH container will be removed when necoder exits"
    NECODER_SSH_CONFIG="$ssh_config" \
    NECODER_REMOTE_SERVER_BINARY="$server_binary" \
        cargo run -p necoder -- "$@"
else
    echo "==> Running the real SSH end-to-end suite"
    # 疎通を「黙って」止める手段を渡す（container を凍結 = TCP は張ったまま応答だけ消える）。
    # laptop の sleep 復帰直後と同じ形で、ControlMaster kill（EOF が届く）では再現できない。
    compose_command="docker compose --project-name $compose_project --file $fixture_dir/compose.yml"
    NECODER_SSH_CONFIG="$ssh_config" \
    NECODER_REMOTE_TEST_URI="ssh://necoder-docker/home/dev/work/sample" \
    NECODER_REMOTE_SERVER_BINARY="$server_binary" \
    NECODER_REMOTE_TEST_FREEZE="$compose_command pause ssh" \
    NECODER_REMOTE_TEST_UNFREEZE="$compose_command unpause ssh" \
        cargo test -p host --test remote_ssh_live -- --nocapture --test-threads=1 "$@"
fi
