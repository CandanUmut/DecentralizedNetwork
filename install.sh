#!/usr/bin/env bash
# Build and install timecoin-node.
#   git clone https://github.com/CandanUmut/DecentralizedNetwork && cd DecentralizedNetwork && ./install.sh
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is not installed. Get it from https://rustup.rs :"
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  exit 1
fi

echo "== building (first build takes a few minutes)"
cargo build --release

BIN_DIR="${HOME}/.local/bin"
mkdir -p "$BIN_DIR"
install -m 755 target/release/timecoin-node "$BIN_DIR/timecoin-node"
echo "== installed to $BIN_DIR/timecoin-node"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "NOTE: add $BIN_DIR to your PATH, e.g.:  export PATH=\"\$PATH:$BIN_DIR\"" ;;
esac

cat <<'EOF'

Quickstart:
  timecoin-node run --data-dir ~/.timecoin        # start your node
  open http://127.0.0.1:3000                      # phone-friendly dashboard
  timecoin-node join-file > my-community.json     # invite a friend
Friend joins with:
  timecoin-node run --data-dir ~/.timecoin --config my-community.json
EOF
