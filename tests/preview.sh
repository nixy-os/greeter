#!/usr/bin/env bash
set -euo pipefail
# Run only inside an isolated X server; never inject input into a real session.
# GTK rendering/keyboard smoke test. Wayland and PAM are covered by the OS VM test.
binary=${1:?usage: preview.sh /path/nixy-greeter /output/directory}
output=${2:?output directory required}
mkdir -p "$output"
Xvfb -displayfd 3 -screen 0 1280x900x24 3>"$output/display" >"$output/xvfb.log" 2>&1 &
server=$!
client=
peer=
trap 'test -z "$client" || kill "$client" 2>/dev/null || true; test -z "$peer" || kill "$peer" 2>/dev/null || true; kill "$server" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
  test ! -s "$output/display" || break
  sleep 0.05
done
export DISPLAY=":$(<"$output/display")"
export GDK_BACKEND=x11
export GTK_THEME=Adwaita
export GSK_RENDERER=cairo
export XDG_CONFIG_HOME="$output/config"
mkdir -p "$XDG_CONFIG_HOME"
unset WAYLAND_DISPLAY GREETD_SOCK
"$binary" --demo >"$output/client.log" 2>&1 &
client=$!
window=$(timeout 15 xdotool search --sync --name 'Nixy login' | head -n 1)
xdotool windowfocus --sync "$window"
sleep 1
import -window "$window" "$output/login.png"
xdotool type 'demo-user'
xdotool key Return
xdotool type 'demo-password'
sleep 0.3
import -window "$window" "$output/masked-password.png"
# In the fixed 900x650 preview, inspect the password interior without its border.
# Thirteen separate small glyphs must have at least four blank pixels between them.
magick "$output/masked-password.png" -crop 230x24+389+421 +repage -threshold 40% \
  -define connected-components:verbose=true -connected-components 8 null: |
  awk '$NF == "srgb(255,255,255)" {
    split($2, box, /x|\+/)
    if (box[1] <= 6 && box[2] <= 6) print box[3], box[1]
  }' | sort -n | awk '
    NR > 1 && $1 - end < 4 { bad = 1 }
    { end = $1 + $2 }
    END { exit (NR != 13 || bad) }
  '
xdotool key Return
sleep 0.5
import -window "$window" "$output/after-login.png"
tesseract "$output/after-login.png" stdout 2>/dev/null | grep -qi 'Demo complete'
# Reset focuses username; the normal form has only two fields before power controls.
xdotool key Tab Tab Return
sleep 0.3
import -window "$window" "$output/reboot-confirm.png"
tesseract "$output/reboot-confirm.png" stdout 2>/dev/null | grep -qi 'Press Reboot again'
xdotool key Tab Return
sleep 0.3
import -window "$window" "$output/shutdown-confirm.png"
tesseract "$output/shutdown-confirm.png" stdout 2>/dev/null | grep -qi 'Press Shutdown again'
xdotool key Return
sleep 0.3
import -window "$window" "$output/power-demo.png"
tesseract "$output/power-demo.png" stdout 2>/dev/null | grep -qi 'no power action'
if grep -Ei '(Gtk|GLib).*(CRITICAL|WARNING)|panic' "$output/client.log"; then
  exit 1
fi
kill "$client"
wait "$client" || true
client=

# Hold real IPC replies so the disabled entry can be inspected while pending.
pending=$(mktemp -d "$output/pending.XXXXXX")
python3 "$(dirname "$0")/pending-greetd.py" "$pending" >"$pending/server.log" 2>&1 &
peer=$!
wait_for() {
  for _ in $(seq 1 200); do
    if test -f "$pending/$1"; then return; fi
    sleep 0.05
  done
  echo "Timed out waiting for $1" >&2
  return 1
}
wait_for server-ready
GREETD_SOCK="$pending/greetd.sock" "$binary" --session /unused-session --systemctl /unused-systemctl >"$pending/client.log" 2>&1 &
client=$!
window=$(timeout 15 xdotool search --sync --name 'Nixy login' | head -n 1)
xdotool windowfocus --sync "$window"
sleep 1
xdotool type 'demo-user'
xdotool key Return
xdotool type 'demo-password'
sleep 0.3
import -window "$window" "$pending/before-submit.png"
xdotool key Return
wait_for create-received
sleep 0.2
import -window "$window" "$pending/pending-create.png"
touch "$pending/release-create"
wait_for password-received
import -window "$window" "$pending/pending-password.png"
for shot in pending-create pending-password; do
  tesseract "$pending/$shot.png" stdout 2>/dev/null | grep -qi 'Logging in'
  # Compare the dots themselves (excluding the caret and border), pixel for pixel.
  magick "$pending/before-submit.png" -crop 123x18+389+424 +repage "$pending/expected.png"
  magick "$pending/$shot.png" -crop 123x18+389+424 +repage "$pending/actual.png"
  magick compare -metric AE "$pending/expected.png" "$pending/actual.png" null:
done
touch "$pending/release-password"
wait_for cancel-received
sleep 0.2
import -window "$window" "$pending/failure-pending-cancel.png"
test "$(magick "$pending/failure-pending-cancel.png" -crop 123x18+389+424 +repage -threshold 40% -format '%[fx:maxima]' info:)" = 0
touch "$pending/release-cancel"
wait_for server-passed
wait "$peer"
peer=
sleep 0.2
import -window "$window" "$pending/reset.png"
tesseract "$pending/reset.png" stdout 2>/dev/null | grep -qi 'Login failed'
if grep -Ei '(Gtk|GLib).*(CRITICAL|WARNING)|panic' "$pending/client.log"; then
  exit 1
fi
echo "Preview login passed; screenshots in $output"
