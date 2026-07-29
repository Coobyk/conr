# conr — Technical Specification v0.3

**July 2026 · Rust-native · Privacy-first · Terminal-native Social Network**

Personal toy for a few close friends on a single public instance. Prefer shipping **auth + chronological posts** before threads, likes polish, or discovery.

---

## 1. Executive Summary & Philosophy

conr is a keyboard-driven, terminal-native social network built entirely in Rust.

**1. Minimalism.** No algorithm, no analytics, no dark patterns. Posts are text. TUI only. Chronological feed only.

**2. Privacy-first identity.** Handle + random token. Server stores only `BLAKE3(token)`. No server-side recovery, no email/phone, no real-name collection.

**3. Rust end to end.** Axum server, Ratatui client, sqlx + PostgreSQL. No JS build chain required.

**Audience:** operator + close friends. Registration is open (anyone who can reach the server).

Primary terminal target: **Ghostty**, with graceful fallbacks.

---

## 2. Product Decisions (Locked)

| Topic | Decision |
| --- | --- |
| Token format | **128 chars**, alphabet `A–Z a–z 0–9` only. No symbols, no dashes. Display as **8×16** centered block |
| Token generation | **Server generates** (see §5 rationale) |
| Login input | Strip everything except `[A-Za-z0-9]` before use/send |
| Registration UX | Force save acknowledgment → show token → welcome animation → **re-enter token to unlock** → allow re-view token once → later reminders |
| Token reminders | Client-only: **next day**, **after 1 week**, then **monthly** (“do you still have your token?”) |
| Backup | Optional encrypted local file; key from **user-chosen recovery passphrase** (not legal name / not card digits — see §5). Never sent to server |
| Handles | **Case-insensitive**; stored and displayed **lowercase**. Fixed in v1 (no renames) |
| Feed | Global chronological top-level posts |
| Post length | **512** characters max; **reject empty / whitespace-only** on server (and client) |
| Edit posts | No |
| Delete posts | **Not in first ship.** Later: **tombstone** one row, keep children, show `[Deleted]` |
| Replies | **Planned multi-level, unbounded depth.** Not required for first ship — get posting working first |
| Thread realtime | Later; reopen/refetch is fine initially |
| Likes | Keep in near-term scope (still desired); implement after basic posting if needed |
| Follows | **Skip** for now |
| Search / FTS | **Skip** for now |
| Tags table | **Skip** for now (hashtags may appear as plain text in content) |
| Blocks / notifications | Skip |
| Spam / analytics | Skip |
| Paths | **XDG only**: `~/.config/conr/`, `~/.local/share/conr/` |
| Server URL | Client `~/.config/conr/config.toml` |
| Registration | Open to anyone |
| Hosting | Single public instance |
| TLS | Terminate in front of the app (Caddy/nginx + Let’s Encrypt); app listens HTTP on localhost — see §10 |

---

## 3. Technology Stack

| Layer | Technology | Notes |
| --- | --- | --- |
| Client UI | Ratatui + crossterm | Immediate-mode TUI |
| Client async | Tokio 1.x | |
| Client keychain | keyring 3.x | libsecret / Keychain / WinCred |
| Client crypto | ring + AEAD (e.g. chacha20poly1305) + Argon2id | Local token backup only |
| Client cache | rusqlite (bundled) | Offline feed cache |
| Client input | tui-textarea | Composer |
| Server | Axum 0.8 | Single binary |
| DB | PostgreSQL 16 + sqlx 0.8 | Compile-time checked queries |
| Auth hash | blake3 | Token hashing |
| Auth session | JWT (Ed25519 via ring) + refresh tokens | |
| IDs | xid | Sortable public IDs |
| Serde | serde_json | HTTP + WebSocket |

Deferred: nucleo (until command palette needs it), viuer, FTS-heavy deps, spam HTTP client.

---

## 4. Database Schema (PostgreSQL 16)

Migrations via `sqlx::migrate!`. Prefer `sqlx::query!` where practical.

### 4.1 Users

```sql
CREATE TABLE users (
    id          TEXT PRIMARY KEY,
    handle      TEXT UNIQUE NOT NULL,  -- always lowercase
    token_hash  BYTEA NOT NULL UNIQUE,
    bio         TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

- `handle`: normalize with `trim` + lowercase before insert/lookup. Suggested pattern: `^[a-z0-9_]{3,32}$`.
- `token_hash`: `BLAKE3` of the UTF-8 token string. Raw token never stored.

### 4.2 Posts (first ship)

```sql
CREATE TABLE posts (
    id          TEXT PRIMARY KEY,
    author_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    parent_id   TEXT REFERENCES posts(id) ON DELETE RESTRICT,
    content     TEXT NOT NULL CHECK (char_length(content) <= 512),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at  TIMESTAMPTZ  -- null = live; set = tombstone (unused until delete ships)
);

CREATE INDEX idx_posts_time ON posts (created_at DESC);
CREATE INDEX idx_posts_author_time ON posts (author_id, created_at DESC);
CREATE INDEX idx_posts_parent ON posts (parent_id, created_at ASC)
    WHERE parent_id IS NOT NULL;
```

**First ship behavior:**

- Only create/list **top-level** posts (`parent_id IS NULL`).
- Reject empty / whitespace-only `content` in the handler (CHECK alone is not enough for whitespace).
- `parent_id` column exists early so multi-level replies can land without a painful migration; API may ignore/reject non-null `parent_id` until replies ship.

**Later — multi-level replies:**

- Any existing post may be a parent; **no depth cap** (interesting threads may spiral).
- Optional `root_id` denormalization if thread queries need it.

**Later — delete (tombstone):**

- Set `deleted_at = now()`, clear or replace `content` server-side as needed.
- **Do not** delete the row; **keep children**.
- API/UI render author/content as `[Deleted]` (or similar).
- `ON DELETE RESTRICT` on `parent_id` avoids accidental hard-delete of parents with children; hard delete of users still cascades (or revisit when delete ships).

### 4.3 Likes (near-term; after posting works)

```sql
CREATE TABLE likes (
    post_id    TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (post_id, user_id)
);
```

### 4.4 Refresh Tokens

```sql
CREATE TABLE refresh_tokens (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Explicitly deferred tables

`follows`, `tags`, `blocks`, `notifications`, spam columns, media.

---

## 5. User Identity & Authentication

### 5.1 Why server-side token generation

| Approach | Pros | Cons |
| --- | --- | --- |
| **Server generates (chosen)** | Guaranteed CSPRNG entropy; simple API; open-source client cannot ship a weak generator by accident | Raw token appears once in the HTTPS response |
| Client generates, sends hash | Token never on wire at register | Login still sends token today; custom clients can pick weak tokens unless server enforces length/alphabet only (cannot enforce true entropy) |

**“Open source clients invent custom tokens”** is not an exploit against other users. A custom client can only choose the secret for **its own** account. The failure mode is someone choosing `aaaa…`; with **server generation** that does not happen.

**“Server sending the token is unreliable”** is mitigated by: (1) TLS in production so the path is encrypted, (2) show-once UX + re-enter confirm so the client proves it received the token, (3) keychain + optional encrypted backup so the network is not the long-term store.

**Chosen: server generates** the 128-char token, returns it **once** in the register response, stores only `token_hash`.

### 5.2 Token format

- Alphabet: `[A-Za-z0-9]` (62 symbols).
- Length: 128.
- Entropy ≈ 763 bits (overkill; intentional).
- Hash: `BLAKE3(token.as_bytes())`.

### 5.3 Token display

Centered monospace block: **16 chars × 8 lines**, no internal dashes:

```
┌─ save this token — you will not see it again ──┐
│                                                │
│          aB3kLm9xQ2wE8rT1                      │
│          uY7nP0sD4fG6hJ9k                      │
│          ... 8 lines total ...                 │
│                                                │
│   Save in a password manager or offline.       │
│   [c] copy all   [v] view again later step     │
└────────────────────────────────────────────────┘
```

Clipboard copy may place all 128 characters as one string.

### 5.4 Registration & first-run flow (client)

1. **Choose handle** → client lowercases → `POST /auth/register`.
2. Server creates user, returns `{ token, access_token, refresh_token, user }` (**only time** token is issued).
3. **Save prompt** — explicit copy that they must store the token in a password manager / safe place; continue only after acknowledgment.
4. **Show token block** (8×16). Optional copy-to-clipboard.
5. **Welcome animation** — short, tasteful “welcome to conr” sequence (keep lightweight; no network dependency).
6. **Unlock step** — user must paste/type the token again. Client strips non-`[A-Za-z0-9]`, verifies length 128 and equality with the issued token (local check), then stores in keychain and proceeds. If mismatch: error, offer **show token again** (humans are dumb).
7. Optional: create encrypted local backup (passphrase prompt).
8. Session already valid from register response; enter feed.

Do **not** skip step 6. The re-paste is the proof they can recover access without us.

### 5.5 Token reminders (client-only)

Store timestamps in local client state (`~/.local/share/conr/state.toml` or sqlite):

| When | Prompt |
| --- | --- |
| ~24h after register | “Do you still have your conr token saved?” |
| ~7 days | Same, stronger wording |
| Every ~30 days thereafter | Light check-in |

Actions: dismiss / “show me where it should be” (keychain/backup status) / “I need to view… wait I can’t, server won’t re-send” → emphasize backup/password manager. Server **cannot** resend the token.

### 5.6 Login

1. Normalize handle to lowercase.
2. Strip token input to `[A-Za-z0-9]` only.
3. `POST /auth/login` with `{ handle, token }`.
4. Server: lookup by handle, `BLAKE3(token)`, constant-time compare to `token_hash`.
5. Issue access JWT (~1h) + refresh token (~14d, rotate on use).

### 5.7 Client storage (XDG)

| What | Path / place |
| --- | --- |
| Config | `~/.config/conr/config.toml` (`server_url`, theme, spam threshold N/A, …) |
| Keychain | service `"conr"`, username = handle |
| Keychain fallback file | `~/.local/share/conr/token` mode `0600` + TUI warning |
| Encrypted backup | `~/.local/share/conr/backup/token` |
| Offline cache | `~/.local/share/conr/cache.db` |
| Client state (reminders) | `~/.local/share/conr/state.toml` |

### 5.8 Encrypted backup (no GDPR landmines)

**Do not** ask for legal name, credit-card digits, national IDs, or other real-world identity. That is personal data you do not want to touch — and if it ever hit your server you would be in GDPR territory you are trying to avoid.

**Do** ask for a **user-chosen recovery passphrase** (any phrase they will remember). Rules:

- Used **only on the client** to derive a key (Argon2id) and encrypt the token backup (AEAD).
- **Never uploaded** to the server.
- TUI copy: “This passphrase never leaves your machine. We cannot reset it.”
- Weak option of “handle-only” key derivation is **rejected** — too easy if the backup file leaks and the handle is public.

Restore: handle + passphrase → decrypt backup → login.

### 5.9 Account recovery (server)

None. No admin token reset in v1. If they lose token + backup + passphrase, account is gone.

### 5.10 GDPR posture (practical, not legal advice)

- Collect **as little as possible**: handle, token hash, posts, timestamps, refresh token hashes.
- No email, phone, legal name, payment data, analytics SDKs.
- Friends instance + minimal data is simpler; still not a substitute for proper legal review if you open to the public EU-wide later.

---

## 6. Architecture (Monorepo)

```
conr/
  Cargo.toml
  conr-shared/
    src/
      lib.rs
      models.rs
      events.rs
      auth.rs          # alphabet, hash helpers, validation
  conr-server/
    src/
      main.rs
      router.rs
      handlers/
        auth.rs
        posts.rs
        likes.rs       # when likes ship
        ws.rs
      db/
        migrations/
      state.rs
  conr-client/
    src/
      main.rs
      app.rs
      ui/
        layout.rs
        feed.rs
        composer.rs
        register.rs    # token block, welcome, unlock
        status_bar.rs
        # thread.rs later
      net/
        ws_client.rs
        api.rs
      config.rs
      keychain.rs
      backup.rs
      cache.rs
      reminders.rs
```

### AppState

```rust
pub struct AppState {
    pub db: PgPool,
    pub broadcast_tx: broadcast::Sender<Arc<Event>>,
}
```

v1 fan-out: **broadcast** all `NewPost` (top-level) to connected clients. Bounded channel; slow clients drop/disconnect. Single process.

---

## 7. TUI Layout & UX

### Main chrome (after unlock)

```
┌──────────────────────────────────────────────────────────────┐
│ @handle | FEED | ● connected | HH:MM                         │
├──────────────────────────────────────────────────────────────┤
│                         feed / compose                       │
├──────────────────────────────────────────────────────────────┤
│ j/k scroll  n new post  / help  q quit                       │
└──────────────────────────────────────────────────────────────┘
```

Minimum 80×24.

### Keys (first ship)

| Key | Action |
| --- | --- |
| `j` / `k` | Scroll feed |
| `n` | New post |
| `g` / `G` | Top / bottom |
| `q` | Quit |
| `?` | Help |

**Later keys:** `Enter` thread, `r` reply, `Space` like, richer `/` palette.

### Composer

- Counter `Compose [n/512]`.
- `Ctrl+S` submit, `Esc` cancel.
- Client blocks empty/whitespace; server is source of truth.

### Config (`~/.config/conr/config.toml`)

```toml
server_url = "https://conr.example.com"  # or http://127.0.0.1:8080 for local

[theme]
background = "#1D1F21"
foreground = "#C5C8C6"
accent = "#81A2BE"
error = "#CC6666"
success = "#B5BD68"
dim = "#5A5A5A"
```

---

## 8. Feature Specs by Phase

### Phase A — First ship (do this)

- Register / login / refresh
- Token reveal + welcome animation + re-enter unlock
- Token reminders schedule
- Keychain (+ fallback file) + optional passphrase-encrypted backup
- Global chronological **feed** of top-level posts
- **Create** top-level posts (≤512, non-empty)
- WebSocket `NewPost` for live feed
- Offline cache of recent feed
- Open registration, minimal in-memory rate limits
- Server URL from config.toml

### Phase B — Shipped

- Likes (toggle + count; WS `LikeUpdated`)
- Multi-level replies (unbounded depth), thread view (refetch on open is OK)
- Tombstone delete (`[Deleted]`, keep children; WS `PostDeleted`)

### Phase C — Shipped

- Follows + Following feed tab
- Search (FTS posts, users, `#tags`)
- Tags index (`post_tags`)
- Edit posts (`edited_at`)
- Blocks (mutual hide + unfollow)
- Notifications (like, reply, follow, mention)
- DMs (1:1 conversations)
- Profile + bio
- Simple post rate limit (60/hour)
- No binary images (URLs in text are fine); no analytics

### Content rules (server-enforced)

- `content` length 1–512 after trim (or reject if trim empty).
- No edits in Phase A/B until explicitly added.
- No hard deletes of post rows when delete ships — tombstone only.

---

## 9. HTTP / WebSocket (Phase A)

| Method | Path | Notes |
| --- | --- | --- |
| POST | `/auth/register` | `{ "handle" }` → `{ token, access_token, refresh_token, user }` once |
| POST | `/auth/login` | `{ "handle", "token" }` |
| POST | `/auth/refresh` | rotate refresh |
| GET | `/feed` | top-level posts, cursor pagination (includes like/reply counts) |
| POST | `/posts` | create post; optional `parent_id` for replies |
| GET | `/posts/:id` | thread tree (root + all descendants) |
| DELETE | `/posts/:id` | tombstone (author only) |
| POST | `/posts/:id/like` | toggle like |
| PATCH | `/posts/:id` | edit own post |
| GET | `/feed?filter=following` | following-only feed |
| GET | `/users/:handle` | profile |
| GET | `/users/:handle/posts` | user posts |
| POST/DELETE | `/users/:handle/follow` | follow / unfollow |
| POST/DELETE | `/users/:handle/block` | block / unblock |
| PATCH | `/me/bio` | update bio |
| GET | `/search?q=` | FTS / users / tags |
| GET | `/tags/:tag` | posts for tag |
| GET | `/notifications` | list |
| POST | `/notifications/read` | mark all read |
| GET/POST | `/dms` | list / start DM by handle |
| GET/POST | `/dms/:id` | thread / send |
| GET | `/ws` | auth’d; `NewPost`, `PostDeleted`, `PostEdited`, `LikeUpdated`, `Notification`, `DmMessage`, `Pong`, `Error` |

---

## 10. Deployment & TLS (plain language)

**TLS** means the connection between client and server is encrypted (the `https://` lock). Without it, anyone on the path can read tokens and passwords on the wire.

You do **not** need to implement TLS inside the Rust app for v1.

**Recommended layout:**

```
Internet → Caddy or nginx (HTTPS, Let's Encrypt free cert)
              → reverse proxy → conr-server on 127.0.0.1:8080 (HTTP)
```

- Install Caddy on the VPS, point DNS at the VPS, Caddy fetches certificates automatically.
- `config.toml` on clients uses `server_url = "https://your.domain"`.
- Local dev: `server_url = "http://127.0.0.1:8080"` is fine.

| Env | Description | Default |
| --- | --- | --- |
| `DATABASE_URL` | Postgres URL | required |
| `CONR_SIGNING_KEY` | base64 Ed25519 seed for JWTs | required |
| `CONR_PORT` | listen port | `8080` |
| `CONR_MAX_CONNECTIONS` | WS cap | `1000` |

```bash
createdb conr
export DATABASE_URL=postgres://localhost/conr
export CONR_SIGNING_KEY=...
cargo run -p conr-server
cargo run -p conr-client
```

---

## 11. Roadmap (summary)

| Phase | Scope |
| --- | --- |
| **A — First ship** | Auth, token UX, feed, post, WS, cache, XDG config |
| **B** (done) | Likes, multi-level replies, tombstone delete |
| **C** (done) | Follows, search, tags, blocks, notifs, DMs, edit, profile |
| later | Binary media, spam tooling, polish |

---

## Appendix A: Example `NewPost`

```json
{
  "type": "NewPost",
  "payload": {
    "post": {
      "id": "cv7a4k2abc",
      "author_handle": "alice",
      "parent_id": null,
      "content": "hello from conr",
      "created_at": "2026-07-17T22:00:00Z",
      "deleted_at": null
    }
  }
}
```

---

## Appendix B: Dependencies (indicative)

**conr-shared:** serde, serde_json, ring, blake3, xid  

**conr-server:** axum, tokio, sqlx, jsonwebtoken, tower-http, tracing  

**conr-client:** ratatui, crossterm, tokio, tokio-tungstenite, reqwest, keyring, rusqlite, tui-textarea, toml, dirs, argon2, chacha20poly1305 (or equivalent AEAD)

---

*conr v0.3 · Phase A = post + auth; threads later · avoid collecting real-world identity*
