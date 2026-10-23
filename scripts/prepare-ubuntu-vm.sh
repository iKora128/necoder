#!/usr/bin/env bash
# UTM へ取り込むディスクと cloud-init CD を用意する。VM の登録・起動は別工程。
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
directory="$repo_root/target/linux-vm"
mkdir -p "$directory"
chmod 700 "$directory"

for filename in SHA256SUMS noble-server-cloudimg-arm64.img; do
    if [[ ! -f "$directory/$filename" ]]; then
        curl --fail --location --retry 3 \
            "https://cloud-images.ubuntu.com/noble/current/$filename" \
            -o "$directory/$filename.partial"
        mv "$directory/$filename.partial" "$directory/$filename"
    fi
done
python3 - "$directory" <<'PY'
import hashlib
import sys
from pathlib import Path
directory = Path(sys.argv[1])
image = directory / 'noble-server-cloudimg-arm64.img'
expected = next(line.split()[0] for line in (directory / 'SHA256SUMS').read_text().splitlines()
                if line.endswith(' *' + image.name))
digest = hashlib.sha256()
with image.open('rb') as stream:
    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
        digest.update(chunk)
if digest.hexdigest() != expected:
    raise SystemExit('Ubuntu image checksum mismatch; do not import this image')
print('Ubuntu image verified:', expected)
PY

if [[ ! -f "$directory/ssh-key" ]]; then
    ssh-keygen -q -t ed25519 -N '' -C necoder-ubuntu-validation -f "$directory/ssh-key"
fi
# UTM に取り込んだ稼働中ディスクではなく、取り込み元の雛形だけを作る。
# 既存ファイルがあれば、再実行で作り直さない。
docker run --rm \
    --mount "type=bind,source=$directory,target=/vm" \
    --mount "type=bind,source=$repo_root/scripts/linux,target=/setup,readonly" \
    ubuntu:24.04 bash -euo pipefail -c '
        apt-get update > /vm/vm-tools-install.log 2>&1
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
            qemu-utils cloud-image-utils python3 >> /vm/vm-tools-install.log 2>&1
        if [[ ! -f /vm/ubuntu-desktop.qcow2 ]]; then
            qemu-img convert -O qcow2 /vm/noble-server-cloudimg-arm64.img /vm/ubuntu-desktop.qcow2
            qemu-img resize /vm/ubuntu-desktop.qcow2 64G
        fi
        if [[ ! -f /vm/seed.iso ]]; then
            python3 /setup/prepare-vm-cloud-init.py /vm
            cloud-localds /vm/seed.iso /vm/user-data /vm/meta-data
        fi
        qemu-img info /vm/ubuntu-desktop.qcow2
    '
echo "VM disk: $directory/ubuntu-desktop.qcow2"
echo "Cloud-init CD: $directory/seed.iso"
echo "SSH key and local login password are in $directory (do not commit or share)."
