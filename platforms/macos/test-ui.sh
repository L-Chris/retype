#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export MACOSX_DEPLOYMENT_TARGET=13.0
target="$(rustc -vV | sed -n 's/^host: //p')"
cargo build --locked --release --target "$target" -p retype-macos
mkdir -p target/macos-ui-validation
library="${CARGO_TARGET_DIR:-$PWD/target}/$target/release/libretype_macos.a"
xcrun clang -DRT_UI_SMOKE -mmacosx-version-min=13.0 -fobjc-arc -fblocks -O1 -Wall -Wextra -Werror -Wno-deprecated-declarations \
    -I platforms/macos/include -I platforms/macos/src platforms/macos/src/*.m platforms/macos/tests/UISmoke.m "$library" \
    -framework Cocoa -framework ImageIO -framework Carbon -framework InputMethodKit -framework QuartzCore -framework UniformTypeIdentifiers \
    -framework Security -framework SystemConfiguration -framework AVFoundation -framework AudioToolbox -framework ApplicationServices \
    -lc++ -liconv -lresolv -o target/macos-ui-validation/smoke
RETYPE_TEST_ROOT="$PWD/target/macos-ui-validation/data" target/macos-ui-validation/smoke
xcrun clang -DRT_UI_SMOKE -mmacosx-version-min=13.0 -fobjc-arc -fblocks -O1 -Wall -Wextra -Werror -Wno-deprecated-declarations \
    -I platforms/macos/include -I platforms/macos/src platforms/macos/src/*.m platforms/macos/tests/TerminationSmoke.m "$library" \
    -framework Cocoa -framework ImageIO -framework Carbon -framework InputMethodKit -framework QuartzCore -framework UniformTypeIdentifiers \
    -framework Security -framework SystemConfiguration -framework AVFoundation -framework AudioToolbox -framework ApplicationServices \
    -lc++ -liconv -lresolv -o target/macos-ui-validation/termination
# A tiny private legacy dictionary lets the termination test open the real Rust
# runtime without downloading resources or touching the user's input history.
python3 - <<'PY'
import hashlib
import pathlib
import struct

root = pathlib.Path("target/macos-ui-validation/data/termination/dictionaries")
root.mkdir(parents=True, exist_ok=True)
word = "阿".encode()
image = b"RTDICT01" + struct.pack("<IHBd", 1, len(word), 1, 100.0) + word + struct.pack("<H", 1)
digest = hashlib.sha256(image).hexdigest()
(root / f"legacy-base-{digest}.bin").write_bytes(image)
(root / "legacy-base.current").write_text(digest)
PY
RETYPE_TEST_ROOT="$PWD/target/macos-ui-validation/data" target/macos-ui-validation/termination
RETYPE_TEST_ROOT="$PWD/target/macos-ui-validation/data" target/macos-ui-validation/termination --fail-flush
python3 platforms/macos/tests/update-helper.py
