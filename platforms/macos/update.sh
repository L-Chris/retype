#!/bin/bash
set -euo pipefail
source="${1:?Missing update bundle}"
destination="${2:?Missing installed bundle}"
pid="${3:?Missing input method PID}"
expected="$HOME/Library/Input Methods/retype.app"
[[ "$destination" == "$expected" && "$source" == "$HOME/Library/Application Support/retype/updates/"* ]] || exit 1
[[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 ]] || exit 1
umask 077
mkdir -p "$HOME/Library/Application Support/retype/logs"
log="$HOME/Library/Application Support/retype/logs/update-install.log"
if [[ -f "$log" && $(stat -f '%z' "$log") -ge 262144 ]]; then mv -f "$log" "$log.1"; fi
exec >> "$log" 2>&1
state="$HOME/Library/Application Support/retype/updates/install-state.json"
step=validating
staging=''
replaced=0
write_state() {
    printf '{"stage":"%s","step":"%s","exitCode":%d,"pid":%d,"time":%s}\n' "$1" "$step" "$2" "$$" "$(date +%s)" > "$state.tmp"
    mv -f "$state.tmp" "$state"
}
finish() {
    result=$?
    trap - EXIT
    if [[ "$result" -ne 0 ]]; then
        printf '%s Update failed at %s (exit %s)\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$step" "$result"
        if [[ "$replaced" == 1 && -d "$staging/previous.app" ]]; then
            [[ ! -e "$destination" ]] || mv "$destination" "$staging/failed.app" || true
            if [[ ! -e "$destination" ]] && mv "$staging/previous.app" "$destination"; then
                "$destination/Contents/MacOS/retype" --register || true
            else
                printf 'Previous app preserved at %s/previous.app\n' "$staging"
                staging=''
            fi
        fi
        write_state failed "$result" || true
        if [[ -d "$destination" ]] && ! kill -0 "$pid" 2>/dev/null; then open -a "$destination" || true; fi
    fi
    [[ -z "$staging" ]] || rm -rf "$staging"
    exit "$result"
}
trap finish EXIT
printf '%s Update started\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
write_state running 0
identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source/Contents/Info.plist")"
[[ "$identifier" == io.github.retype.inputmethod.Retype ]] || exit 1
codesign --verify --strict "$source"
old_team="$(codesign -dv --verbose=4 "$destination" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
new_team="$(codesign -dv --verbose=4 "$source" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
[[ "$old_team" == "$new_team" ]] || exit 1
step=waiting_for_exit
write_state running 0
for attempt in {1..300}; do
    if ! kill -0 "$pid" 2>/dev/null; then break; fi
    sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then exit 1; fi
staging="$(mktemp -d "$HOME/Library/Input Methods/.retype-update.XXXXXX")"
step=installing
write_state running 0
ditto "$source" "$staging/retype.app"
codesign --verify --strict "$staging/retype.app"
mv "$destination" "$staging/previous.app"
replaced=1
mv "$staging/retype.app" "$destination"
"$destination/Contents/MacOS/retype" --register
# Refresh the cached input-source icon after replacing the bundle.
pkill -u "$(id -u)" -x TextInputMenuAgent || true
open -a "$destination"
step=complete
write_state complete 0
printf '%s Update complete\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
