#!/bin/sh
set -eu

key_file=${AUTHORIZED_KEY_FILE:-/run/necoder/id_ed25519.pub}
if [ ! -s "$key_file" ]; then
    echo "authorized key is missing: $key_file" >&2
    exit 1
fi

install -d -m 700 -o dev -g dev /home/dev/.ssh
install -m 600 -o dev -g dev "$key_file" /home/dev/.ssh/authorized_keys

project=/home/dev/work/sample
install -d -m 755 -o dev -g dev "$project/src" "$project/docs"

cat > "$project/Cargo.toml" <<'EOF'
[package]
name = "remote-sample"
version = "0.1.0"
edition = "2021"
EOF

cat > "$project/src/lib.rs" <<'EOF'
/// TODO: replace this sample function with real project code.
pub fn add(left: u64, right: u64) -> u64 {
    left + right
}
EOF

cat > "$project/src/main.rs" <<'EOF'
fn main() {
    // TODO: add a useful command-line interface.
    println!("remote ssh sample");
}
EOF

cat > "$project/README.md" <<'EOF'
# Remote SSH sample

TODO: document the sample project.
EOF

cat > "$project/docs/notes.md" <<'EOF'
# Notes

The live test edits this file through a second SSH connection.
EOF

chown -R dev:dev /home/dev/work
runuser -u dev -- git -C "$project" init -q
runuser -u dev -- git -C "$project" config user.name "necoder Test"
runuser -u dev -- git -C "$project" config user.email "necoder-test@example.invalid"
runuser -u dev -- git -C "$project" add .
runuser -u dev -- git -C "$project" commit -qm "Initial fixture"

ssh-keygen -A
exec /usr/sbin/sshd -D -e
