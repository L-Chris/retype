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
    -framework Cocoa -framework Carbon -framework InputMethodKit -framework QuartzCore -framework UniformTypeIdentifiers \
    -framework Security -framework SystemConfiguration -framework AVFoundation -framework AudioToolbox -framework ApplicationServices \
    -lc++ -liconv -lresolv -o target/macos-ui-validation/smoke
RETYPE_TEST_ROOT="$PWD/target/macos-ui-validation/data" target/macos-ui-validation/smoke
xcrun clang -DRT_UI_SMOKE -mmacosx-version-min=13.0 -fobjc-arc -fblocks -O1 -Wall -Wextra -Werror -Wno-deprecated-declarations \
    -I platforms/macos/include -I platforms/macos/src platforms/macos/src/*.m platforms/macos/tests/TerminationSmoke.m "$library" \
    -framework Cocoa -framework Carbon -framework InputMethodKit -framework QuartzCore -framework UniformTypeIdentifiers \
    -framework Security -framework SystemConfiguration -framework AVFoundation -framework AudioToolbox -framework ApplicationServices \
    -lc++ -liconv -lresolv -o target/macos-ui-validation/termination
RETYPE_TEST_ROOT="$PWD/target/macos-ui-validation/data" target/macos-ui-validation/termination
RETYPE_TEST_ROOT="$PWD/target/macos-ui-validation/data" target/macos-ui-validation/termination --fail-flush
python3 platforms/macos/tests/update-helper.py
