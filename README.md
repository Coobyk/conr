# conr

Keyboard-driven, terminal-native social network. Privacy-first identity (handle + token). Chronological feed only. Rust end-to-end.

See [spec.md](./spec.md) for the full design.

## Prerequisites

- Rust 1.80+ (edition 2021)
- PostgreSQL 16+ (18 works)

## Database

```bash
# Arch / CachyOS
sudo pacman -S postgresql
sudo -u postgres initdb -D /var/lib/postgres/data   # once
sudo systemctl enable --now postgresql
sudo -u postgres createuser -s "$USER"
createdb conr
```

## Run server

```bash
export DATABASE_URL=postgres://localhost/conr
# optional fixed JWT secret (base64 of 32 bytes); generated ephemerally if unset
# export CONR_SIGNING_KEY=$(openssl rand -base64 32)
export CONR_PORT=8080

cargo run -p conr-server
```

## Run client

```bash
cargo run -p conr-client
```

Config is created at `~/.config/conr/config.toml`:

```toml
server_url = "http://127.0.0.1:8080"
```

Data (cache, backups, logs): `~/.local/share/conr/`.

## Phase A (current)

- Register / login (handle + 128-char token)
- Token reveal, welcome animation, re-enter unlock
- Optional passphrase-encrypted local backup
- Global chronological feed + compose posts (512 chars)
- WebSocket live `NewPost`
- Offline SQLite cache

## License

MIT — see [LICENSE](./LICENSE).
