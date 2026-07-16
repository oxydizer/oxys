#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 ARTIFACT.OXYS CATEGORY/PF" >&2
    exit 2
fi
if [ "$(id -u)" -ne 0 ]; then
    echo "run this only as root inside a disposable Gentoo test system" >&2
    exit 2
fi

artifact=$1
package=$2
category=${package%%/*}
pf=${package#*/}
atom=$category/${pf%-[0-9]*}
oxys_bin=${OXYS_BIN:-oxys}

for command in "$oxys_bin" qcheck qlist emerge sha256sum; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "required command not found: $command" >&2
        exit 2
    }
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
export NOCOLOR=true

"$oxys_bin" package verify "$artifact"
"$oxys_bin" package install "$artifact" --root /
qcheck "$package"
qlist -ICv "$atom" | grep -Fx "$package" >/dev/null

receipt=/var/lib/oxys/installed/$category/$pf.toml
sha256sum "$receipt" >"$work/receipt.before"
"$oxys_bin" package install "$artifact" --root /
sha256sum -c "$work/receipt.before"

emerge --pretend -uDN @world >"$work/world.txt"
if grep -Eq "^\[[^]]+\][[:space:]]+$category/$pf([[:space:]:]|$)" "$work/world.txt"; then
    echo "$package was selected by emerge -uDN @world" >&2
    exit 1
fi

emerge --depclean --pretend >"$work/depclean.txt"
if grep -Eq "(^|[[:space:]])$category/$pf([[:space:]:]|$)" "$work/depclean.txt"; then
    echo "$package was selected by emerge --depclean" >&2
    exit 1
fi

"$oxys_bin" remove "$package" --root /
if qlist -ICv "$atom" | grep -Fx "$package" >/dev/null; then
    echo "$package is still visible to Portage after removal" >&2
    exit 1
fi

echo "all .oxys Portage acceptance checks passed for $package"
