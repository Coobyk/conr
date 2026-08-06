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

## Features

### Auth & identity

- Register / login (handle + 128-char token)
- Token reveal, welcome animation, re-enter unlock
- Optional passphrase-encrypted local backup
- Token reminders

### Timeline

- Global chronological feed + **Following** tab (`Tab`)
- Compose posts (512 chars), `#hashtags`, `@mentions`
- Post markup: `**bold**`, `__underline__`, `~~strike~~`, `{red}color{/}`
- WebSocket live updates
- Offline SQLite cache

### Social (Phase B + C)

- Likes (`Space`), replies (`r`), threads (`Enter`)
- Edit own posts (`e`), tombstone delete (`d`)
- Follow / unfollow (`f` on feed or profile)
- Profiles (`p`), bio edit
- Blocks (`b` on profile)
- Search posts / users / tags (`/`)
- Notifications (`i`) — likes, replies, follows, mentions
- Direct messages (`m`)
- Light rate limit (60 posts / hour)

### Keys

| Key | Action |
| --- | --- |
| `j` / `k` | Scroll |
| `n` | New post |
| `r` | Reply |
| `e` | Edit own post / bio on profile |
| `Enter` | Open thread / result |
| `Space` | Like / unlike |
| `d` | Delete own post |
| `Tab` | For you ↔ Following |
| `/` | Search |
| `i` | Notifications |
| `m` | DMs |
| `p` | Profile of selected post's author |
| `Shift+p` | Your profile |
| `f` | Follow author |
| `u` | Refresh |
| `Esc` | Back |
| `?` | Help |
| `q` | Quit |
| `Shift+L` | Logout (keeps saved tokens) |
| `Ctrl+S` | Submit compose / DM |

## License

MIT — see [LICENSE](./LICENSE).
