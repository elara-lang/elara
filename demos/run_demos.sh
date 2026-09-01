#!/usr/bin/env bash
# End-to-end demo runner: source + AOT .elac parity, C99 FFI host, native library.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
unset ELARA_PATH
export ELARA_SYS_INI=

if [[ ! -x bin/elara || ! -x bin/elarac || ! -f bin/libelara.a ]]; then
  make build
fi

shopt -s nullglob
demos=(demos/0[1-7]_*.ela)
if ((${#demos[@]} < 7)); then
  echo "expected demos 01..07, found ${#demos[@]}" >&2
  exit 1
fi

for f in "${demos[@]}"; do
  echo "==> $f (source)"
  out_src="$(./bin/elara "$f")"

  # Keep .elac beside the source so relative import search dirs resolve.
  bc="${f%.ela}.elac"
  echo "==> $f (AOT -> $bc)"
  ./bin/elarac -o "$bc" "$f"
  out_bin="$(./bin/elara "$bc")"
  rm -f "$bc"

  if [[ "$out_src" != "$out_bin" ]]; then
    echo "stdout mismatch for $f" >&2
    diff -u <(printf '%s\n' "$out_src") <(printf '%s\n' "$out_bin") >&2 || true
    exit 1
  fi
done

echo "==> demos/08_c_ffi_sandbox_host.c"
CC="${CC:-cc}"
"$CC" -std=c99 -Wall -Wextra -Werror \
  -Ielara/include \
  demos/08_c_ffi_sandbox_host.c \
  bin/libelara.a \
  -lm -lpthread -ldl \
  -o /tmp/elara_ffi_demo
/tmp/elara_ffi_demo
rm -f /tmp/elara_ffi_demo

echo "==> demos/09_native_library.c"
so_dir="$(mktemp -d)"
"$CC" -std=c99 -Wall -Wextra -Werror -shared -fPIC \
  -Ielara/include \
  demos/09_native_library.c \
  -o "$so_dir/native_demo.so"
echo "==> demos/09_native_library.ela"
./bin/elara --lib "$so_dir" demos/09_native_library.ela
rm -rf "$so_dir"

echo "All 9 Elara demos passed (source + AOT .elac + C99 FFI + native library)"
