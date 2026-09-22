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

## Run

One binary (`conr`) for everything:

```bash
cargo build --release
# binary: target/release/conr  (optional: ln -s "$PWD/target/release/conr" ~/.local/bin/conr)

conr                  # tip: client / server / -h
conr -h               # full help
conr client           # interactive TUI
conr server start     # background server (pid + log under ~/.local/share/conr-server/run/)
conr server status
conr server stop
conr server run       # foreground server (logs to stderr)
```

Server environment:

```bash
export DATABASE_URL=postgres://localhost/conr
# optional fixed JWT secret (base64 of 32 bytes); generated ephemerally if unset
# export CONR_SIGNING_KEY=$(openssl rand -base64 32)
export CONR_PORT=8080
# optional: where uploaded images are stored (default: ~/.local/share/conr-server/media)
# export CONR_MEDIA_DIR=/var/lib/conr/media

conr server start
```

Config is created at `~/.config/conr/config.toml`:

```toml
server_url = "http://127.0.0.1:8080"

[theme]
background = "#10190f"
foreground = "#decab2"
accent     = "#98b1cd"
error      = "#b7aa87"
success    = "#acc7a9"
dim        = "#616560"
follow_system = true
```

Theme hex values are applied to the TUI chrome on launch. Surfaces and selection colors are derived from those six keys. With `follow_system = true` (default), Omarchy's active theme (`~/.local/state/omarchy/current/theme/colors.toml`) is read on every launch and overrides these hex values — set `follow_system = false` to pin a custom palette.

Data (cache, backups, logs): `~/.local/share/conr/`. Server run files: `~/.local/share/conr-server/run/`.

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
  - Composer styles in place as pairs close; `Ctrl+B`/`U`/`T` wrap; `{` color autocomplete
- Quote posts (`Shift+R`) with a pending-quote strip and `@mention` autocomplete in the quote composer
- Command palette (`Ctrl+P`, also `Ctrl+K`) — fuzzy-find commands
- Images: upload JPEG/PNG/WebP/GIF (up to 4 per post, 20 MiB each); paste from clipboard (`Ctrl+V`), paste a path, or `Ctrl+O`; open with `o`
- WebSocket live updates
- Offline SQLite cache + in-memory view cache (pages switch instantly, refresh in background)
- Cursor pagination on the feed and profile (j / G load older posts)

### Social (Phase B + C)

- Likes (`Space`), replies (`r`), threads (`Enter`)
- Edit own posts (`e`), tombstone delete (`d`)
- Follow / unfollow (`f` on feed or profile)
- Profiles (`p`), bio edit
- Blocks (`b` on profile)
- Search posts / users / tags (`/`)
- Notifications (`i`) — likes, replies, follows, mentions
- Direct messages (`m`) with unread badge
- Light rate limit (60 posts / hour)

### Agent CLI (JSON)

Same `conr` binary — not a TUI. Actions print JSON on stdout; `next` / `previous` / `replies` / `up` print one post per call as `@user123: Hi everybody!`. Errors go to stderr.

```bash
conr register --handle alice              # prints token once; stores it
conr login --handle alice --identity TOK  # or CONR_IDENTITY / keychain

# sequential walk (state in ~/.local/share/conr/cli-nav.json)
conr next                                 # first/next post  → @user: text
conr previous                             # (alias: prev)
conr replies                              # enter replies to current post
conr up                                   # leave replies, reprint parent

# act on the current post from the walk
conr like                                 # toggle like (or --id ID)
conr repost "check this out"              # quote-repost with commentary

# still JSON
conr feed [--following] [--cursor CUR]
conr post "hello" [--reply ID] [--quote ID]
conr thread ID
conr search "query"
conr profile [HANDLE]
conr notifications
conr dm list
conr dm show CONV_ID
conr dm @user123 send hi there
conr watch                                # WS events as JSONL, reconnects

# global: --server URL (CONR_SERVER), --handle (CONR_HANDLE), --identity (CONR_IDENTITY)
```

Auth reuses the same identity token store as the TUI (`~/.local/share/conr/tokens/` + keyring).

### Keys

| Key | Action |
| --- | --- |
| `j` / `k` | Scroll (loads older posts near the bottom) |
| `g` / `G` | Top / bottom of the current list |
| `n` | New post |
| `r` | Reply |
| `e` | Edit own post / bio on profile |
| `Shift+R` | Quote selected post |
| `Enter` | Open thread / result |
| `Space` | Like / unlike |
| `o` | Open images on selected post |
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
| `Ctrl+P` | Command palette (also `Ctrl+K`) |
| `Ctrl+S` | Submit compose / DM |
| `Ctrl+V` | Paste image from clipboard (compose) |
| `Ctrl+O` | Attach image by path (compose) |
| `Ctrl+X` | Remove last attached image |

## License

MIT — see [LICENSE](./LICENSE).
