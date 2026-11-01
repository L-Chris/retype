#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export MACOSX_DEPLOYMENT_TARGET=13.0
target="$(rustc -vV | sed -n 's/^host: //p')"
output="${RETYPE_LATENCY_OUTPUT:-$PWD/target/macos-input-latency/current}"
mkdir -p "$output"
if [[ -n "${RETYPE_LATENCY_LIBRARY:-}" ]]; then library="$RETYPE_LATENCY_LIBRARY"; else
    cargo build --locked --release --target "$target" -p retype-macos
    library="${CARGO_TARGET_DIR:-$PWD/target}/$target/release/libretype_macos.a"
fi
python3 - "$output" <<'PY'
import hashlib,pathlib,sys
source=pathlib.Path('data/dict/retype-dict.bin').resolve()
root=pathlib.Path(sys.argv[1])/'data'/'dictionaries'
root.mkdir(parents=True,exist_ok=True)
hash=hashlib.sha256(source.read_bytes()).hexdigest()
link=root/f'legacy-base-{hash}.bin'
if not link.exists():link.symlink_to(source)
(root/'legacy-base.current').write_text(hash)
PY
xcrun clang -DRT_UI_SMOKE -mmacosx-version-min=13.0 -fobjc-arc -fblocks -O2 -Wall -Wextra -Werror -Wno-deprecated-declarations \
    -I platforms/macos/include -I platforms/macos/src platforms/macos/src/*.m platforms/macos/tests/InputLatency.m "$library" \
    -framework Cocoa -framework ImageIO -framework Carbon -framework InputMethodKit -framework QuartzCore -framework UniformTypeIdentifiers \
    -framework Security -framework SystemConfiguration -framework AVFoundation -framework AudioToolbox -framework ApplicationServices \
    -lc++ -liconv -lresolv -o "$output/input-latency"
RETYPE_TEST_ROOT="$output/data" "$output/input-latency" "${RETYPE_LATENCY_EXPECT_FIRST:-还是有点慢}" "$output" "${RETYPE_LATENCY_ROUNDS:-40}"
