#!/usr/bin/env bash
set -euo pipefail
here="$(cd -- "$(dirname -- "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 "$here/kledisc" "$HOME/.local/bin/kledisc"
install -Dm644 "$here/kledisc.png" "$data/icons/hicolor/256x256/apps/kledisc.png"
mkdir -p "$data/applications"
# Use an absolute path so the launcher also works without ~/.local/bin in PATH.
python3 - "$here/kledisc.desktop" "$data/applications/kledisc.desktop" "$HOME/.local/bin/kledisc" <<'PY'
import pathlib, sys
source, target, binary = sys.argv[1:]
escaped = binary.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
pathlib.Path(target).write_text(pathlib.Path(source).read_text().replace('Exec=kledisc', 'Exec="' + escaped + '"'))
PY
printf 'kledisc instalado no menu de aplicativos.\n'
