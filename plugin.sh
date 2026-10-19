#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-archify.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-archify" ;;
    *) rel="x86_64-apple-darwin/gray-archify" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-archify" ;;
    *) rel="x86_64-unknown-linux-musl/gray-archify" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-archify.exe" "$here/target/release/gray-archify" "$here/target/debug/gray-archify.exe" "$here/target/debug/gray-archify"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-archify binary not found in $here" >&2
exit 1
