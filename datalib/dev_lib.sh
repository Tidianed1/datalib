#!/usr/bin/env bash
# Sourced by the dev launchers (serve_dev.sh, dev.sh) for what they
# decide alike: which library a run opens, whether to open a browser at
# it, and a free port.

# The library a run opens: the argument, else the one the desktop app
# opens first, `Libraries/Default` in the app's data directory (Tauri's
# `app_data_dir` for the identifier in tauri/tauri.conf.json). A leading
# `~` is expanded here because a quoted argument reaches us with it
# intact.
dev_library_root() {
  local arg="${1:-}"
  case "$arg" in
    "")    echo "$(dev_app_data_dir)/Libraries/Default" ;;
    "~")   echo "$HOME" ;;
    "~/"*) echo "$HOME/${arg#\~/}" ;;
    *)     echo "$arg" ;;
  esac
}

dev_app_data_dir() {
  case "$(uname -s)" in
    Darwin) echo "$HOME/Library/Application Support/com.imbue.datalib" ;;
    *)      echo "${XDG_DATA_HOME:-$HOME/.local/share}/com.imbue.datalib" ;;
  esac
}

# Opens the OS browser only for a person at a terminal: an agent's shell
# or a preview pane pipes stdout, and a tab popping open there steals the
# focus from whatever the person is doing. `DATALIB_NO_OPEN=1` keeps the
# URL on the terminal even when it is one. The launchers pass the binary
# `--no-open` and call this instead, because only they know when the URL
# answers, and dev.sh's URL is Vite's, not the binary's.
dev_open_browser() {
  local url="$1"
  if [[ -n "${DATALIB_NO_OPEN:-}" || ! -t 1 ]]; then
    echo "open $url in your browser"
    return
  fi
  case "$(uname -s)" in
    Darwin) open "$url" ;;
    Linux)  xdg-open "$url" >/dev/null 2>&1 || true ;;
    *)      echo "open $url in your browser" ;;
  esac
}

# Bind :0, read the port back, close. The gap before the real listener
# binds is a race; losing it costs a restart, which is fine for a local
# run and not for the test suite (datalib/ui/playwright.config.ts takes
# each port from the server's own `--url-file` instead).
dev_free_port() {
  python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'
}
