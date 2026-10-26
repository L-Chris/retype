#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
[[ "$(uname -s)" == Darwin ]] || { echo 'Build on macOS with Xcode Command Line Tools.' >&2; exit 1; }
export MACOSX_DEPLOYMENT_TARGET=13.0
root="$PWD"
output="$root/dist/macos"
version="$(sed -nE 's/^version = "([^"]+)".*$/\1/p' Cargo.toml | head -1)"
[[ -n "$version" ]] || exit 1
mkdir -p "$output"
stage="$(mktemp -d "$output/stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
bundle="$stage/retype.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
sed "s/VERSION_PLACEHOLDER/$version/g" platforms/macos/Info.plist > "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
executables=()
for arch in arm64 x86_64; do
    if [[ "$arch" == arm64 ]]; then target=aarch64-apple-darwin; else target=x86_64-apple-darwin; fi
    cargo build --locked --release --target "$target" -p retype-macos
    library="${CARGO_TARGET_DIR:-$root/target}/$target/release/libretype_macos.a"
    xcrun clang -arch "$arch" -isysroot "$sdk" -mmacosx-version-min=13.0 \
        -fobjc-arc -fblocks -O2 -Wall -Wextra -Werror -Wno-deprecated-declarations \
        -I platforms/macos/include platforms/macos/src/*.m "$library" \
        -framework Cocoa -framework ImageIO -framework Carbon -framework InputMethodKit -framework QuartzCore \
        -framework UniformTypeIdentifiers -framework Security -framework SystemConfiguration \
        -framework AVFoundation -framework AudioToolbox -framework ApplicationServices \
        -lc++ -liconv -lresolv -o "$stage/retype-$arch"
    executables+=("$stage/retype-$arch")
done
xcrun lipo -create "${executables[@]}" -output "$bundle/Contents/MacOS/retype"
cp assets/logo.png "$bundle/Contents/Resources/logo.png"
cp NOTICE.txt "$bundle/Contents/Resources/NOTICE.txt"
cp LICENSE "$bundle/Contents/Resources/LICENSE.txt"
cp platforms/macos/update.sh "$bundle/Contents/Resources/update.sh"
mkdir -p "$stage/retype.iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/logo.png --out "$stage/retype.iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" assets/logo.png --out "$stage/retype.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$stage/retype.iconset" -o "$bundle/Contents/Resources/retype.icns"
# Render a centered vector mask at its menu-bar point size, independent of the app icon.
xcrun clang -isysroot "$sdk" -mmacosx-version-min=13.0 -fobjc-arc -O2 -Wall -Wextra -Werror \
    platforms/macos/tools/MenuIcon.m -framework Foundation -framework CoreGraphics -o "$stage/menu-icon"
"$stage/menu-icon" "$bundle/Contents/Resources/menuTemplate.pdf"
# Ad-hoc signing supports local development. Public distribution needs a Developer ID and notarization.
sign_identity="${RETYPE_MACOS_SIGN_IDENTITY:-}"
if [[ -z "$sign_identity" ]]; then
    sign_identity="$(security find-identity -v -p codesigning 2>/dev/null | sed -nE 's/^[[:space:]]*[0-9]+\) ([0-9A-F]{40}) ".*"/\1/p' | head -1)"
fi
codesign --force --options runtime --entitlements platforms/macos/entitlements.plist --sign "${sign_identity:--}" "$bundle"
codesign --verify --strict --verbose=2 "$bundle"
archive="$output/retype-$version-macos-universal.zip"
package="$stage/retype-macos"
mkdir -p "$package"
cp platforms/macos/install.sh platforms/macos/uninstall.sh "$package/"
cp platforms/macos/install.command "$package/"
cp docs/macos.md "$package/README.md"
chmod +x "$package"/*.sh "$package"/*.command
archive_bundle() {
    ditto "$bundle" "$package/retype.app"
    ditto -c -k --sequesterRsrc --keepParent "$package" "$archive"
}
archive_bundle
if [[ -n "${RETYPE_MACOS_NOTARY_PROFILE:-}" ]]; then
    [[ -n "${RETYPE_MACOS_SIGN_IDENTITY:-}" ]] || { echo 'Notarization requires RETYPE_MACOS_SIGN_IDENTITY.' >&2; exit 1; }
    xcrun notarytool submit "$archive" --keychain-profile "$RETYPE_MACOS_NOTARY_PROFILE" --wait
    xcrun stapler staple "$bundle"
    archive_bundle
fi
(cd "$output" && shasum -a 256 "$(basename "$archive")" > "$(basename "$archive").sha256")
echo "$archive"
