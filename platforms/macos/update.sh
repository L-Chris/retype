#!/bin/bash
set -euo pipefail
source="${1:?Missing update bundle}"
destination="${2:?Missing installed bundle}"
pid="${3:?Missing input method PID}"
expected="$HOME/Library/Input Methods/retype.app"
[[ "$destination" == "$expected" && "$source" == "$HOME/Library/Application Support/retype/updates/"* ]] || exit 1
[[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 ]] || exit 1
identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source/Contents/Info.plist")"
[[ "$identifier" == io.github.retype.inputmethod.Retype ]] || exit 1
codesign --verify --strict "$source"
old_team="$(codesign -dv --verbose=4 "$destination" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
new_team="$(codesign -dv --verbose=4 "$source" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
[[ "$old_team" == "$new_team" ]] || exit 1
for attempt in {1..100}; do
    if ! kill -0 "$pid" 2>/dev/null; then break; fi
    sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then exit 1; fi
staging="$(mktemp -d "$HOME/Library/Input Methods/.retype-update.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
ditto "$source" "$staging/retype.app"
codesign --verify --strict "$staging/retype.app"
mv "$destination" "$staging/previous.app"
if ! mv "$staging/retype.app" "$destination"; then mv "$staging/previous.app" "$destination"; exit 1; fi
"$destination/Contents/MacOS/retype" --register
open -a "$destination"
