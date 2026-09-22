#!/usr/bin/env bash
# Share a local conr-server over Cloudflare Tunnel (quick tunnel or named).
#
# Quick (temporary https://*.trycloudflare.com URL, no Cloudflare login):
#   ./scripts/host-cloudflare.sh
#
# Named tunnel (stable hostname; requires: cloudflared tunnel login):
#   ./scripts/host-cloudflare.sh --named conr --hostname conr.example.com
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PORT="${CONR_PORT:-8080}"
DATABASE_URL="${DATABASE_URL:-postgres://${USER:-postgres}@localhost/conr}"
MODE="quick"
TUNNEL_NAME="conr"
HOSTNAME=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --named) MODE="named"; TUNNEL_NAME="${2:-conr}"; shift 2 || shift ;;
    --hostname) HOSTNAME="${2:-}"; shift 2 ;;
    --port) PORT="${2:-8080}"; shift 2 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

if ! command -v cloudflared >/dev/null; then
  echo "cloudflared not found. Install: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/" >&2
  exit 1
fi

# Stable JWT key so sessions survive restarts while you share the instance.
if [[ -z "${CONR_SIGNING_KEY:-}" ]]; then
  KEY_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/conr/signing.key"
  mkdir -p "$(dirname "$KEY_FILE")"
  if [[ ! -f "$KEY_FILE" ]]; then
    openssl rand -base64 32 | tr -d '\n' >"$KEY_FILE"
    chmod 600 "$KEY_FILE"
    echo "wrote new CONR_SIGNING_KEY to $KEY_FILE"
  fi
  export CONR_SIGNING_KEY
  CONR_SIGNING_KEY="$(cat "$KEY_FILE")"
  export CONR_SIGNING_KEY
fi

export DATABASE_URL
export CONR_PORT="$PORT"

# Start server if nothing is listening on the port.
if ! curl -sf "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
  echo "starting conr server on :${PORT} …"
  cargo build -q
  cargo run -q -- server run >/tmp/conr-server-tunnel.log 2>&1 &
  echo $! >/tmp/conr-server-tunnel.pid
  for _ in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
      break
    fi
    sleep 0.25
  done
  if ! curl -sf "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
    echo "server failed to start; see /tmp/conr-server-tunnel.log" >&2
    exit 1
  fi
  echo "conr-server up (pid $(cat /tmp/conr-server-tunnel.pid))"
else
  echo "conr-server already healthy on :${PORT}"
fi

ORIGIN="http://127.0.0.1:${PORT}"

if [[ "$MODE" == "quick" ]]; then
  echo ""
  echo "Starting Cloudflare quick tunnel → ${ORIGIN}"
  echo "A public https://*.trycloudflare.com URL will print below."
  echo "Friends set ~/.config/conr/config.toml:"
  echo '  server_url = "https://THAT-URL.trycloudflare.com"'
  echo ""
  echo "Press Ctrl+C to stop the tunnel (server keeps running if it was already up)."
  echo ""
  exec cloudflared tunnel --url "$ORIGIN" --no-autoupdate
fi

# Named tunnel path
if [[ -z "$HOSTNAME" ]]; then
  echo "--named requires --hostname conr.yourdomain.com" >&2
  exit 1
fi

if [[ ! -f "${HOME}/.cloudflared/cert.pem" ]]; then
  echo "No Cloudflare cert. Run once:  cloudflared tunnel login" >&2
  exit 1
fi

if ! cloudflared tunnel list 2>/dev/null | grep -qw "$TUNNEL_NAME"; then
  echo "creating tunnel ${TUNNEL_NAME} …"
  cloudflared tunnel create "$TUNNEL_NAME"
fi

TID="$(cloudflared tunnel list | awk -v n="$TUNNEL_NAME" '$2==n {print $1; exit}')"
CFG="${HOME}/.cloudflared/conr-config.yml"
cat >"$CFG" <<EOF
tunnel: ${TID}
credentials-file: ${HOME}/.cloudflared/${TID}.json

ingress:
  - hostname: ${HOSTNAME}
    service: ${ORIGIN}
  - service: http_status:404
EOF

echo "routing DNS ${HOSTNAME} → tunnel ${TUNNEL_NAME} (${TID})"
cloudflared tunnel route dns "$TUNNEL_NAME" "$HOSTNAME" || true

echo ""
echo "Public URL: https://${HOSTNAME}"
echo "Friends set ~/.config/conr/config.toml:"
echo "  server_url = \"https://${HOSTNAME}\""
echo ""
exec cloudflared tunnel --config "$CFG" run "$TUNNEL_NAME"
