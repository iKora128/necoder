#!/usr/bin/env bash
# Ubuntu 24.04 用。Docker / Linux CI / Desktop VM で同じ依存を使う。
set -euo pipefail

if [[ "$(id -u)" != 0 ]]; then
    echo "Run with sudo: sudo bash scripts/linux/install-deps.sh" >&2
    exit 1
fi

apt-get update
apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang cmake curl file git jq \
    libasound2-dev libegl1-mesa-dev libfontconfig1-dev libglib2.0-dev \
    libssl-dev libstdc++-14-dev libvulkan1 libwayland-dev \
    libx11-xcb-dev libxcb1-dev libxkbcommon-x11-dev libzstd-dev \
    lld mesa-vulkan-drivers openssh-client pkg-config python3 ripgrep
