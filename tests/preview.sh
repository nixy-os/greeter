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
trap 'test -z "$client" || kill "$client" 2>/dev/null || true; kill "$server" 2>/dev/null || true' EXIT
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
echo "Preview login passed; screenshots in $output"
