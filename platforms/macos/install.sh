#!/bin/bash
set -euo pipefail
[[ "$(uname -s)" == Darwin ]] || { echo 'Run this installer on macOS.' >&2; exit 1; }
bundle="${1:?Usage: bash install.sh /path/to/retype.app}"
[[ -d "$bundle/Contents" ]] || { echo 'Input method app not found.' >&2; exit 1; }
identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$bundle/Contents/Info.plist")"
[[ "$identifier" == io.github.retype.inputmethod.Retype ]] || { echo 'Unexpected bundle identifier.' >&2; exit 1; }
codesign --verify --strict "$bundle"
destination="$HOME/Library/Input Methods/retype.app"
if [[ "$bundle" == "$destination" ]]; then echo 'Already installed at the destination.'; exit 0; fi
mkdir -p "$HOME/Library/Input Methods"
# Quit only our own per-user server; never kill client applications or other input methods.
pkill -u "$(id -u)" -x retype || true
staging="$(mktemp -d "$HOME/Library/Input Methods/.retype-install.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
ditto "$bundle" "$staging/retype.app"
codesign --verify --strict "$staging/retype.app"
if [[ -e "$destination" ]]; then mv "$destination" "$staging/previous.app"; fi
if ! mv "$staging/retype.app" "$destination"; then
    [[ ! -e "$staging/previous.app" ]] || mv "$staging/previous.app" "$destination"
    exit 1
fi
"$destination/Contents/MacOS/retype" --register
open -a "$destination" --args --settings
open 'x-apple.systempreferences:com.apple.Keyboard-Settings.extension'
echo 'Add retype in Keyboard > Text Input > Edit > +. If absent, log out and back in once.'
