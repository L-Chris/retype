#!/bin/bash
set -euo pipefail
[[ "$(uname -s)" == Darwin ]] || exit 1
destination="$HOME/Library/Input Methods/retype.app"
echo 'First select another input source and remove retype from Keyboard > Text Input.'
read -r -p 'Remove the retype input method app? [y/N] ' answer
[[ "$answer" == y || "$answer" == Y ]] || exit 0
if [[ -d "$destination" ]]; then
    identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$destination/Contents/Info.plist")"
    [[ "$identifier" == io.github.retype.inputmethod.Retype ]] || exit 1
    pkill -u "$(id -u)" -x retype || true
    rm -rf "$destination"
fi
echo 'Personal settings, learning and dictionaries were kept in ~/Library/Application Support/retype.'
