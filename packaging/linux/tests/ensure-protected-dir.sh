#!/bin/sh
set -eu

# Regression test for ensure_protected_dir: require_protected_dir assigns to
# the global `path` variable, so the caller must keep using its own "$1" for
# the destination. A missing child must be created without touching the
# existing parent's mode.

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)

run_case() {
    installer=$1
    tmp=$(mktemp -d)
    parent="$tmp/parent"
    child="$parent/child"
    mkdir -m 0750 "$parent"
    expected_child=$child

    # Simulate the real function's global assignment.
    require_protected_dir() {
        path=$1
    }

    install() {
        target=""
        mode=""
        while [ $# -gt 0 ]; do
            case "$1" in
                -d) ;;
                -m) shift; mode=$1 ;;
                -o|-g) shift ;;
                *) target=$1 ;;
            esac
            shift
        done
        if [ "$target" != "$expected_child" ]; then
            echo "$installer: install targeted '$target', expected '$expected_child'" >&2
            return 1
        fi
        mkdir -p "$target"
        [ -z "$mode" ] || chmod "$mode" "$target"
    }

    sudo() { "$@"; }

    eval "$(awk '/^ensure_protected_dir\(\) \{/,/^}/' "$installer")"
    ensure_protected_dir "$child"

    [ -d "$child" ] || {
        echo "$installer: missing child directory was not created" >&2
        return 1
    }
    mode=$(stat -c '%a' "$parent")
    [ "$mode" = "750" ] || {
        echo "$installer: parent mode changed from 750 to $mode" >&2
        return 1
    }
    rm -rf "$tmp"
    echo "ok: $installer"
}

run_case "$repo/packaging/linux/install.sh"
run_case "$repo/packaging/linux/install-helper.sh"
