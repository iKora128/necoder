#!/bin/sh
set -eu

mode=test
if [ "${1:-}" = "--gui" ]; then
    mode=gui
    shift
fi

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture_dir="$repo_root/tests/fixtures/remote-ssh"
scratch_dir=$(mktemp -d "${TMPDIR:-/tmp}/shirushi-remote-ssh.XXXXXX")
compose_project="shirushi-remote-ssh-$$"

export SHIRUSHI_REMOTE_TEST_PUBLIC_KEY="$scratch_dir/id_ed25519.pub"

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
Host shirushi-docker
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

if [ "${SHIRUSHI_REMOTE_SERVER_BINARY:-}" ]; then
    server_binary=$SHIRUSHI_REMOTE_SERVER_BINARY
else
    server_binary=''
    for candidate in \
        "$HOME/.local/share/shirushi/remote/artifacts/$target/shirushi-remote-server" \
        "$repo_root/target/$target/release/shirushi-remote-server" \
        "$repo_root/target/$target/debug/shirushi-remote-server"
    do
        if [ -f "$candidate" ]; then
            server_binary=$candidate
            break
        fi
    done
fi

if [ ! -f "$server_binary" ]; then
    cat >&2 <<EOF
Linux remote-server artifact not found for $target.
Build it first, then rerun this script:

  cargo zigbuild -p host --bin shirushi-remote-server --release --target $target

Or set SHIRUSHI_REMOTE_SERVER_BINARY to an existing $target binary.
EOF
    exit 1
fi

echo "==> SSH host: shirushi-docker (127.0.0.1:$ssh_port, $target)"
cd "$repo_root"
if [ "$mode" = gui ]; then
    echo "==> Opening Shirushi for the SSH picker demo"
    echo "==> In Shirushi: + -> Remote/SSH -> shirushi-docker"
    echo "==> Then browse to work/sample and open it as the project"
    echo "==> The SSH container will be removed when Shirushi exits"
    SHIRUSHI_SSH_CONFIG="$ssh_config" \
    SHIRUSHI_REMOTE_SERVER_BINARY="$server_binary" \
        cargo run -p shirushi -- "$@"
else
    echo "==> Running the real SSH end-to-end suite"
    SHIRUSHI_SSH_CONFIG="$ssh_config" \
    SHIRUSHI_REMOTE_TEST_URI="ssh://shirushi-docker/home/dev/work/sample" \
    SHIRUSHI_REMOTE_SERVER_BINARY="$server_binary" \
        cargo test -p host --test remote_ssh_live -- --nocapture --test-threads=1 "$@"
fi
