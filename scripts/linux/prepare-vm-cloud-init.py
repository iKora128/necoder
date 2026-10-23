#!/usr/bin/env python3
"""Ubuntu 検証 VM の初期設定を作る。Ubuntu コンテナ内から実行する。"""
import crypt
import json
import secrets
import sys
from pathlib import Path

directory = Path(sys.argv[1])
password_path = directory / "login-password.txt"
if not password_path.exists():
    password_path.write_text(secrets.token_urlsafe(24) + "\n")
password_path.chmod(0o600)
password_hash = crypt.crypt(password_path.read_text().strip(), crypt.mksalt(crypt.METHOD_SHA512))
public_key = (directory / "ssh-key.pub").read_text().strip()

config = {
    "hostname": "necoder-ubuntu",
    "manage_etc_hosts": True,
    "ssh_pwauth": False,
    "disable_root": True,
    "users": [{
        "name": "necoder",
        "gecos": "necoder Linux validation",
        "groups": ["adm", "sudo", "video", "render"],
        "sudo": "ALL=(ALL) NOPASSWD:ALL",
        "shell": "/bin/bash",
        "lock_passwd": False,
        "passwd": password_hash,
        "ssh_authorized_keys": [public_key],
    }],
    "package_update": True,
    "packages": ["qemu-guest-agent", "openssh-server"],
    "write_files": [{
        "path": "/usr/local/sbin/necoder-desktop-setup",
        "permissions": "0755",
        "content": """#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get install -y --no-install-recommends \\
    ubuntu-desktop-minimal gnome-terminal xorg dbus-x11 spice-vdagent \\
    mesa-vulkan-drivers mesa-utils vulkan-tools \\
    ibus-mozc language-pack-ja fonts-noto-cjk \\
    libfontconfig1 libxkbcommon-x11-0 libssl3 libasound2t64
cat > /etc/gdm3/custom.conf <<'GDM'
[daemon]
AutomaticLoginEnable=true
AutomaticLogin=necoder
[security]
[xdmcp]
[chooser]
[debug]
GDM
systemctl enable qemu-guest-agent
systemctl set-default graphical.target
systemctl restart gdm3
touch /var/lib/necoder-desktop-ready
""",
    }],
    "runcmd": [["bash", "-c", "/usr/local/sbin/necoder-desktop-setup > /var/log/necoder-desktop-setup.log 2>&1"]],
}
(directory / "user-data").write_text("#cloud-config\n" + json.dumps(config, indent=2) + "\n")
(directory / "user-data").chmod(0o600)
(directory / "meta-data").write_text("instance-id: necoder-ubuntu-24-arm64\nlocal-hostname: necoder-ubuntu\n")
