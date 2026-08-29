#!/bin/sh
# Install the bundled `necoder` terminal launcher into /usr/local/bin.
set -eu

usage() {
    cat <<'EOF'
Usage: install-cli-mac.sh [--force] [--prefix <directory>]

Installs the `necoder` command into /usr/local/bin by default.
Use --prefix "$HOME/.local/bin" to install without administrator privileges.
EOF
}

force=0
destination_dir=/usr/local/bin
while [ "$#" -gt 0 ]; do
    case "$1" in
        --force) force=1 ;;
        --prefix)
            shift
            if [ "$#" -eq 0 ]; then
                printf '%s\n' '--prefix にはディレクトリが必要です。' >&2
                exit 2
            fi
            destination_dir=$1
            ;;
        -h|--help) usage; exit 0 ;;
        *) printf '不明なオプション: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
if [ -x "$script_dir/bin/necoder" ]; then
    source_launcher=$script_dir/bin/necoder
elif [ -x "$script_dir/necoder-cli" ]; then
    # Repository checkout convenience.
    source_launcher=$script_dir/necoder-cli
else
    printf '%s\n' 'necoder CLIランチャーが見つかりません。アプリを再インストールしてください。' >&2
    exit 1
fi

destination=$destination_dir/necoder
if [ -d "$destination" ] && [ ! -L "$destination" ]; then
    printf '%s はディレクトリなので置き換えません。別の --prefix を指定してください。\n' "$destination" >&2
    exit 1
fi
if [ -e "$destination" ] || [ -L "$destination" ]; then
    current=$(readlink "$destination" 2>/dev/null || true)
    if [ "$current" = "$source_launcher" ]; then
        printf 'すでにインストール済みです: %s\n' "$destination"
        exit 0
    fi
    if [ "$force" -ne 1 ]; then
        printf '%s はすでに存在します。置き換える場合は --force を付けてください。\n' "$destination" >&2
        exit 1
    fi
fi

install_link() {
    mkdir -p "$destination_dir"
    ln -sfn "$source_launcher" "$destination"
}

if { [ -d "$destination_dir" ] && [ -w "$destination_dir" ]; } || { [ ! -e "$destination_dir" ] && [ -w "$(dirname -- "$destination_dir")" ]; }; then
    install_link
else
    printf '管理者権限を使って %s にインストールします。\n' "$destination_dir"
    sudo mkdir -p "$destination_dir"
    sudo ln -sfn "$source_launcher" "$destination"
fi

printf 'インストール完了: %s\n' "$destination"
printf '使い方: necoder .\n'
