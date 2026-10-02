#!/usr/bin/env bash
# TPM and CLI share one reversible reconciler.
set -euo pipefail
CURRENT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
exec python3 "${CURRENT_DIR}/lib/tmux.py" sync "$@"
