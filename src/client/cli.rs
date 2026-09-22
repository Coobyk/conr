use std::path::PathBuf;

use crate::shared::{AuthResponse, Event, Post, RegisterResponse};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::client::api::Api;
use crate::client::config::{data_dir, Config};
use crate::client::keychain;
use crate::client::reminders::ReminderState;

#[derive(Parser, Debug)]
#[command(
    name = "conr",
    about = "conr client — TUI when launched with no arguments, CLI when given a subcommand",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Server base URL (overrides config.toml)
    #[arg(long, global = true, env = "CONR_SERVER")]
    server: Option<String>,

    /// Account handle (defaults to last active session)
    #[arg(long, global = true, env = "CONR_HANDLE")]
    handle: Option<String>,

    /// Identity token (register token); falls back to keychain / file store
    #[arg(long, global = true, env = "CONR_IDENTITY")]
    identity: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create an account; prints token once and stores it for later commands
    Register {
        #[arg(long)]
        handle: String,
    },
    /// Save an identity token for an existing handle
    Login,
    /// First page of the chronological feed as JSON
    Feed {
        /// Only posts from accounts you follow
        #[arg(long)]
        following: bool,
        /// Pagination cursor from a previous feed response
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Print the next post as `@user: text` (starts a new feed walk)
    Next,
    /// Print the previous post as `@user: text`
    #[command(alias = "prev")]
    Previous,
    /// Enter replies to the current post (stack a new walk)
    Replies,
    /// Leave replies / go up one level; reprints the parent post
    Up,
    /// Create a post (or reply/quote with --reply / --quote)
    Post {
        /// Post body (max 512 chars)
        content: String,
        /// Parent post id for a reply
        #[arg(long)]
        reply: Option<String>,
        /// Post id to quote (requires non-empty content as commentary)
        #[arg(long)]
        quote: Option<String>,
    },
    /// Toggle like (current post if --id omitted)
    Like {
        #[arg(long)]
        id: Option<String>,
    },
    /// Repost (quote) the current post with commentary
    Repost {
        /// Commentary shown with the quoted post (required by the server)
        content: String,
        /// Quote this post id instead of the current one
        #[arg(long)]
        id: Option<String>,
    },
    /// Fetch a thread (root + descendants)
    Thread { id: String },
    /// Search posts, users, and tags
    Search { query: String },
    /// Profile for a handle (defaults to the active session)
    Profile { handle: Option<String> },
    /// Unread + recent notifications
    Notifications,
    /// Direct messages: `dm list` | `dm show ID` | `dm @user123 send hi …`
    #[command(name = "dm", alias = "dms")]
    Dm {
        #[command(subcommand)]
        action: DmAction,
    },
    /// Stream server events as JSONL on stdout (Ctrl+C to stop)
    Watch,
}

#[derive(Subcommand, Debug)]
enum DmAction {
    /// List conversations
    List,
    /// Fetch one conversation + messages by conversation id
    Show { id: String },
    /// `dm @user123 send hello world` / `dm <conv-id> send …` / `dm <conv-id> show`
    #[command(external_subcommand)]
    Target(Vec<String>),
}

/// Stack frames for sequential post walking across process invocations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NavState {
    /// Handle this walk belongs to (reset when the session changes).
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    stack: Vec<Frame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Frame {
    kind: FrameKind,
    posts: Vec<Post>,
    /// Index of the current post in `posts`.
    index: usize,
    /// Feed page cursor (empty string / none = end).
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    following: bool,
    /// Post whose replies this frame holds.
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FrameKind {
    Feed,
    Replies,
}

impl NavState {
    fn path() -> Result<PathBuf> {
        Ok(data_dir()?.join("cli-nav.json"))
    }

    fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Drop the walk if it was started under a different account.
    fn sync_handle(&mut self, handle: &str) {
        if self.handle.as_deref() != Some(handle) {
            self.handle = Some(handle.to_string());
            self.stack.clear();
        }
    }

    fn current(&self) -> Option<&Post> {
        let frame = self.stack.last()?;
        frame.posts.get(frame.index)
    }

    fn current_mut(&mut self) -> Option<&mut Frame> {
        self.stack.last_mut()
    }

    fn print_current(&self) -> Result<()> {
        let post = self
            .current()
            .context("no current post — run `next` first")?;
        println!("{}", format_post_line(post));
        Ok(())
    }
}

/// One-line agent format: `@user123: Hi everybody!`
fn format_post_line(post: &Post) -> String {
    let raw = post.display_content();
    let parts: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let body = if parts.is_empty() {
        if post.has_images() {
            "[image]".to_string()
        } else {
            String::new()
        }
    } else {
        let joined = parts.join(" ");
        if parts.len() > 1 {
            format!("{joined}…")
        } else {
            joined
        }
    };
    if body.is_empty() {
        format!("@{}", post.author_handle)
    } else {
        format!("@{}: {}", post.author_handle, body)
    }
}

impl Cli {
    /// Parse agent CLI from process argv (throws clap errors on bad input).
    pub fn parse() -> Self {
        <Self as clap::Parser>::parse()
    }

    pub async fn run(mut self) -> Result<()> {
        let Some(command) = self.command.take() else {
            use clap::CommandFactory;
            Self::command().print_help()?;
            println!();
            std::process::exit(2);
        };
        self.exec(command).await
    }

    async fn exec(&self, command: Command) -> Result<()> {
        match command {
            Command::Register { handle } => self.cmd_register(&handle).await,
            Command::Login => self.cmd_login().await,
            Command::Feed { following, cursor } => {
                let api = self.authed().await?;
                let resp = api.feed(cursor.as_deref(), following).await?;
                print_json(&resp)
            }
            Command::Next => self.cmd_next().await,
            Command::Previous => self.cmd_previous(),
            Command::Replies => self.cmd_replies().await,
            Command::Up => self.cmd_up(),
            Command::Post {
                content,
                reply,
                quote,
            } => {
                let api = self.authed().await?;
                if reply.is_some() && quote.is_some() {
                    bail!("a post cannot be both a reply and a quote");
                }
                let post = api
                    .create_post(&content, reply.as_deref(), quote.as_deref(), &[])
                    .await?;
                print_json(&post)
            }
            Command::Like { id } => {
                let api = self.authed().await?;
                let id = match id {
                    Some(id) => id,
                    None => self.current_post_id()?,
                };
                let resp = api.toggle_like(&id).await?;
                print_json(&resp)
            }
            Command::Repost { content, id } => {
                let api = self.authed().await?;
                let id = match id {
                    Some(id) => id,
                    None => self.current_post_id()?,
                };
                let post = api.create_post(&content, None, Some(&id), &[]).await?;
                print_json(&post)
            }
            Command::Thread { id } => {
                let api = self.authed().await?;
                let resp = api.get_thread(&id).await?;
                print_json(&resp)
            }
            Command::Search { query } => {
                let api = self.authed().await?;
                let resp = api.search(&query).await?;
                print_json(&resp)
            }
            Command::Profile { handle } => {
                let api = self.authed().await?;
                let handle = match handle {
                    Some(h) => h,
                    None => self
                        .resolved_handle()?
                        .context("pass --handle or log in first")?,
                };
                let resp = api.get_profile(&handle).await?;
                print_json(&resp)
            }
            Command::Notifications => {
                let api = self.authed().await?;
                let resp = api.notifications().await?;
                print_json(&resp)
            }
            Command::Dm { action } => self.cmd_dm(action).await,
            Command::Watch => self.cmd_watch().await,
        }
    }

    // --- sequential walk -------------------------------------------------

    async fn cmd_next(&self) -> Result<()> {
        let api = self.authed().await?;
        let handle = self.active_handle()?;
        let mut nav = NavState::load()?;
        nav.sync_handle(&handle);

        if nav.stack.is_empty() {
            let following = false;
            let resp = api.feed(None, following).await?;
            nav.stack.push(Frame {
                kind: FrameKind::Feed,
                posts: resp.posts,
                index: 0,
                next_cursor: resp.next_cursor,
                following,
                parent_id: None,
            });
            let empty = nav.stack.last().map(|f| f.posts.is_empty()).unwrap_or(true);
            if empty {
                bail!("feed is empty");
            }
            nav.save()?;
            return nav.print_current();
        }

        let advanced = {
            let frame = nav
                .current_mut()
                .context("corrupt nav state — run `up` or delete cli-nav.json")?;
            if frame.index + 1 < frame.posts.len() {
                frame.index += 1;
                true
            } else {
                false
            }
        };
        if advanced {
            nav.save()?;
            return nav.print_current();
        }

        // Need another feed page.
        let (cursor, following) = {
            let frame = nav.stack.last().expect("stack non-empty");
            (frame.next_cursor.clone(), frame.following)
        };
        if Self::frame_is_replies(&nav) {
            bail!("end of replies — run `up` or `previous`");
        }
        let Some(cursor) = cursor else {
            bail!("end of feed");
        };
        let resp = api.feed(Some(&cursor), following).await?;
        if resp.posts.is_empty() {
            bail!("end of feed");
        }
        let frame = nav.current_mut().expect("stack non-empty");
        frame.posts.extend(resp.posts);
        frame.next_cursor = resp.next_cursor;
        frame.index += 1;
        nav.save()?;
        nav.print_current()
    }

    fn cmd_previous(&self) -> Result<()> {
        let mut nav = NavState::load()?;
        if nav.stack.is_empty() {
            bail!("no posts yet — run `next` first");
        }
        {
            let frame = nav.current_mut().expect("stack non-empty");
            if frame.index == 0 {
                bail!("start of list");
            }
            frame.index -= 1;
        }
        nav.save()?;
        nav.print_current()
    }

    async fn cmd_replies(&self) -> Result<()> {
        let api = self.authed().await?;
        let handle = self.active_handle()?;
        let mut nav = NavState::load()?;
        nav.sync_handle(&handle);
        let current_id = {
            let post = nav
                .current()
                .context("no current post — run `next` first")?;
            post.id.clone()
        };
        let thread = api.get_thread(&current_id).await?;
        let replies: Vec<Post> = thread
            .posts
            .into_iter()
            .filter(|p| p.id != current_id && p.parent_id.as_deref() == Some(current_id.as_str()))
            .collect();
        if replies.is_empty() {
            println!("(no replies)");
            return Ok(());
        }
        nav.stack.push(Frame {
            kind: FrameKind::Replies,
            posts: replies,
            index: 0,
            next_cursor: None,
            following: false,
            parent_id: Some(current_id),
        });
        nav.save()?;
        nav.print_current()
    }

    fn cmd_up(&self) -> Result<()> {
        let mut nav = NavState::load()?;
        if nav.stack.len() <= 1 {
            // Top level: allow leaving a lone replies frame if somehow alone.
            if nav.stack.len() == 1 && nav.stack[0].kind == FrameKind::Replies {
                nav.stack.clear();
                nav.save()?;
                println!("(left replies)");
                return Ok(());
            }
            bail!("already at top");
        }
        nav.stack.pop();
        nav.save()?;
        nav.print_current()
    }

    fn current_post_id(&self) -> Result<String> {
        let nav = NavState::load()?;
        let post = nav
            .current()
            .context("no current post — run `next` (or pass --id)")?;
        Ok(post.id.clone())
    }

    fn active_handle(&self) -> Result<String> {
        self.resolved_handle()?
            .context("no active session — pass --handle, or run register/login")
    }

    fn frame_is_replies(nav: &NavState) -> bool {
        nav.stack
            .last()
            .map(|f| f.kind == FrameKind::Replies)
            .unwrap_or(false)
    }

    // --- auth / session --------------------------------------------------

    async fn cmd_register(&self, handle: &str) -> Result<()> {
        let config = self.load_config()?;
        let api = Api::new(&config.server_url)?;
        let resp: RegisterResponse = api.register(handle).await?;
        keychain::store_token(&resp.user.handle, &resp.token)?;
        let mut state = ReminderState::load()?;
        state.mark_registered(&resp.user.handle)?;
        // New account starts a fresh walk.
        let mut nav = NavState::load()?;
        nav.handle = Some(resp.user.handle.clone());
        nav.stack.clear();
        nav.save()?;
        print_json(&resp)
    }

    async fn cmd_login(&self) -> Result<()> {
        let handle = self
            .resolved_handle()?
            .context("pass --handle (or CONR_HANDLE) to log in")?;
        let identity = match &self.identity {
            Some(t) => t.clone(),
            None => keychain::load_token(&handle)
                .ok()
                .flatten()
                .context("no identity token — pass --identity")?,
        };
        let config = self.load_config()?;
        let api = Api::new(&config.server_url)?;
        let resp: AuthResponse = api.login(&handle, &identity).await?;
        keychain::store_token(&resp.user.handle, &identity)?;
        let mut state = ReminderState::load()?;
        state.set_active_handle(&resp.user.handle)?;
        let mut nav = NavState::load()?;
        nav.handle = Some(resp.user.handle.clone());
        nav.stack.clear();
        nav.save()?;
        print_json(&resp)
    }

    async fn cmd_dm(&self, action: DmAction) -> Result<()> {
        let api = self.authed().await?;
        match action {
            DmAction::List => {
                let resp = api.list_dms().await?;
                print_json(&resp)
            }
            DmAction::Show { id } => {
                let resp = api.get_dm(&id).await?;
                print_json(&resp)
            }
            DmAction::Target(args) => {
                if args.len() < 2 {
                    bail!("usage: dm <@handle|conv-id> send <message…>  |  dm <conv-id> show");
                }
                let target = args[0].clone();
                let verb = args[1].to_ascii_lowercase();
                match verb.as_str() {
                    "send" => {
                        if args.len() < 3 {
                            bail!("usage: dm <@handle|conv-id> send <message…>");
                        }
                        let message = args[2..].join(" ");
                        let handle_like = target.starts_with('@');
                        let msg = if handle_like {
                            api.send_dm_to(target.trim_start_matches('@'), &message)
                                .await?
                        } else {
                            api.send_dm_in(&target, &message).await?
                        };
                        print_json(&msg)
                    }
                    "show" | "open" | "read" => {
                        let resp = api.get_dm(&target).await?;
                        print_json(&resp)
                    }
                    other => bail!("unknown dm verb `{other}` — use send or show"),
                }
            }
        }
    }

    async fn cmd_watch(&self) -> Result<()> {
        let api = self.authed().await?;
        let url = api.ws_url()?;
        loop {
            let access = match api.access_token() {
                Some(t) => t,
                None => {
                    if api.refresh_session().await? {
                        api.access_token()
                            .context("no access token after refresh")?
                    } else {
                        bail!("session expired — log in again");
                    }
                }
            };
            match connect_and_stream(&url, &access).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("watch disconnected: {e:#}; reconnecting in 2s");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let _ = api.refresh_session().await;
                }
            }
        }
    }

    fn load_config(&self) -> Result<Config> {
        let mut config = Config::load().context("load config")?;
        if let Some(s) = &self.server {
            config.server_url = s.clone();
        }
        Ok(config)
    }

    fn resolved_handle(&self) -> Result<Option<String>> {
        if let Some(h) = &self.handle {
            return Ok(Some(h.clone()));
        }
        Ok(ReminderState::load()?.handle)
    }

    /// Authenticated API client: config server + identity → access session.
    async fn authed(&self) -> Result<Api> {
        let config = self.load_config()?;
        let api = Api::new(&config.server_url)?;
        let handle = self
            .resolved_handle()?
            .context("no active session — pass --handle, or run register/login")?;
        let identity = match &self.identity {
            Some(t) => t.clone(),
            None => keychain::load_token(&handle)
                .ok()
                .flatten()
                .with_context(|| format!("no saved token for @{handle} — pass --identity"))?,
        };
        api.login(&handle, &identity).await?;
        Ok(api)
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    serde_json::to_writer(std::io::stdout(), value)?;
    use std::io::Write;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}

async fn connect_and_stream(url: &str, access: &str) -> Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

    let mut req = url.into_client_request().context("ws request")?;
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {access}")
            .parse()
            .context("ws auth header")?,
    );
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .context("ws connect")?;
    let (_write, mut read) = ws.split();
    while let Some(msg) = read.next().await {
        match msg {
            Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                if let Ok(ev) = serde_json::from_str::<Event>(&text) {
                    print_json(&ev)?;
                } else {
                    println!("{text}");
                }
            }
            Ok(tokio_tungstenite::tungstenite::Message::Close(frame)) => {
                bail!("ws closed: {frame:?}");
            }
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }
    }
    bail!("ws stream ended")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn post(handle: &str, content: &str) -> Post {
        Post {
            id: format!("p-{handle}-{content}"),
            author_id: "u".into(),
            author_handle: handle.into(),
            parent_id: None,
            quote_id: None,
            quote: None,
            content: content.into(),
            created_at: Utc::now(),
            deleted_at: None,
            edited_at: None,
            like_count: 0,
            liked_by_me: false,
            reply_count: 0,
            images: Vec::new(),
        }
    }

    #[test]
    fn format_is_at_handle_colon_text() {
        assert_eq!(
            format_post_line(&post("user123", "Hi everybody!")),
            "@user123: Hi everybody!"
        );
    }

    #[test]
    fn format_collapses_newlines_and_marks_images() {
        let mut p = post("alice", "line one\nline two");
        assert_eq!(format_post_line(&p), "@alice: line one line two…");
        p.content = String::new();
        assert_eq!(format_post_line(&p), "@alice");
        p.images = vec![crate::shared::MediaInfo {
            id: "img".into(),
            content_type: "image/png".into(),
            size_bytes: 1,
            width: None,
            height: None,
        }];
        assert_eq!(format_post_line(&p), "@alice: [image]");
    }

    #[test]
    fn nav_round_trips_and_resets_on_handle_change() {
        let mut nav = NavState::default();
        nav.sync_handle("alice");
        nav.stack.push(Frame {
            kind: FrameKind::Feed,
            posts: vec![post("bob", "hi")],
            index: 0,
            next_cursor: Some("c".into()),
            following: false,
            parent_id: None,
        });
        assert_eq!(nav.current().map(|p| p.author_handle.as_str()), Some("bob"));
        nav.sync_handle("alice");
        assert_eq!(nav.stack.len(), 1);
        nav.sync_handle("carol");
        assert!(nav.stack.is_empty());
        assert_eq!(nav.handle.as_deref(), Some("carol"));
    }

    #[test]
    fn previous_and_next_walk_indices() {
        let mut nav = NavState::default();
        nav.stack.push(Frame {
            kind: FrameKind::Feed,
            posts: vec![post("a", "1"), post("b", "2"), post("c", "3")],
            index: 0,
            next_cursor: None,
            following: false,
            parent_id: None,
        });
        assert_eq!(nav.current().unwrap().content, "1");
        nav.stack.last_mut().unwrap().index = 2;
        assert_eq!(nav.current().unwrap().content, "3");
        nav.stack.last_mut().unwrap().index = 1;
        assert_eq!(nav.current().unwrap().content, "2");
    }
}
