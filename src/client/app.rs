use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::shared::{
    format_token_block, strip_token, validate_token, DmConversation, DmMessage, Event, MediaInfo,
    Notification, Post, UserProfile, MEDIA_MAX_BYTES, MEDIA_MAX_PER_POST, POST_MAX_LEN,
};
use anyhow::{Context, Result};
use chrono::Utc;
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use crate::client::api::Api;
use crate::client::backup;
use crate::client::cache::Cache;
use crate::client::config::Config;
use crate::client::keychain;
use crate::client::phase_c::{
    DmListState, DmNewState, DmThreadState, FeedTab, NotifState, ProfileState, SearchState,
};
use crate::client::reminders::ReminderState;
use crate::client::ui;
use crate::client::ui::theme::Palette;
use crate::client::view_cache::{merge_posts_append, ViewCache};

#[derive(Debug, Clone)]
pub struct Session {
    pub handle: String,
    pub user_id: String,
}

/// Open thread view state (kept while overlays like help/compose sit on top).
#[derive(Debug, Clone)]
pub struct ThreadState {
    pub root_id: String,
    pub posts: Vec<Post>,
    pub selected: usize,
}

/// Where to return after leaving compose / confirm dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnTo {
    Feed,
    Thread,
}

#[derive(Debug)]
pub enum Screen {
    #[allow(dead_code)]
    Boot,
    WelcomeMenu {
        /// Index into the combined menu: saved accounts first, then actions.
        selected: usize,
    },
    RegisterHandle {
        input: String,
        error: Option<String>,
    },
    SaveAck,
    ShowToken {
        token: String,
        /// Status from last export (clipboard / file path).
        export_status: String,
        flash: u8,
    },
    WelcomeAnim {
        frame_idx: u8,
    },
    UnlockConfirm {
        expected: String,
        input: String,
        error: Option<String>,
        can_reveal: bool,
    },
    BackupPrompt {
        selected: usize,
    },
    BackupPassphrase {
        input: String,
        confirm: String,
        stage: u8, // 0 enter, 1 confirm
        error: Option<String>,
    },
    Login {
        handle: String,
        token_input: String,
        focus: usize, // 0 handle 1 token
        error: Option<String>,
    },
    /// Pre-auth client settings (server URL, auto-login, …).
    Settings {
        server_url: String,
        auto_login: bool,
        /// 0 = server URL field, 1 = auto-login toggle
        focus: usize,
        error: Option<String>,
        back: SettingsBack,
    },
    Feed,
    Thread,
    Compose {
        textarea: TextArea<'static>,
        error: Option<String>,
        parent_id: Option<String>,
        /// When set, Ctrl+S PATCHes this post instead of creating.
        edit_id: Option<String>,
        /// Pending quote preview (quotable snapshot of the selected post).
        quote: Option<crate::shared::QuotedPost>,
        /// Live @mention candidates under the caret.
        mention_ac: crate::client::interactions::MentionAc,
        return_to: ReturnTo,
        /// Selected row in the color autocomplete list (`{…}`).
        color_ac_idx: usize,
        /// Uploaded images ready to attach on submit.
        images: Vec<MediaInfo>,
        /// When set, composer is collecting a local file path to upload.
        attach_path: Option<String>,
    },
    ConfirmDelete {
        post_id: String,
        preview: String,
        return_to: ReturnTo,
    },
    Search {
        state: SearchState,
    },
    Profile {
        state: ProfileState,
    },
    Notifications {
        state: NotifState,
    },
    DmList {
        state: DmListState,
    },
    DmNew {
        state: DmNewState,
    },
    DmThread {
        state: DmThreadState,
    },
    EditBio {
        input: String,
        error: Option<String>,
    },
    /// Confirm block with a required reason.
    BlockUser {
        handle: String,
        reason: String,
        error: Option<String>,
    },
    Reminder {
        message: String,
    },
    Help {
        return_to: ReturnTo,
    },
    /// Fuzzy command palette (Ctrl+P / Ctrl+K); `prev` is the screen underneath.
    Palette {
        query: String,
        selected: usize,
        prev: Box<Screen>,
    },
}

/// Where Settings returns after Esc / save.
#[derive(Debug, Clone)]
pub enum SettingsBack {
    WelcomeMenu {
        selected: usize,
    },
    Login {
        handle: String,
        token_input: String,
        focus: usize,
        error: Option<String>,
    },
}

/// One row on the welcome menu.
#[derive(Debug, Clone)]
pub enum MenuEntry {
    /// Saved local account — Enter logs in with stored token (no typing).
    Account(String),
    Register,
    ManualLogin,
    Quit,
}

pub const FEED_PAGE: i64 = 50;

pub struct App {
    #[allow(dead_code)]
    pub config: Config,
    pub palette: Palette,
    pub api: Api,
    pub screen: Screen,
    pub session: Option<Session>,
    /// Global chronological feed (top-level posts).
    pub posts: Vec<Post>,
    pub selected: usize,
    /// First visible post index in the feed viewport.
    pub feed_scroll: usize,
    /// Active thread (if any). Survives compose/help/delete overlays.
    pub thread: Option<ThreadState>,
    pub feed_tab: FeedTab,
    pub feed_next_cursor: Option<String>,
    pub feed_has_more: bool,
    feed_loading_more: bool,
    /// After `G`, keep the caret on the newest last row when a page appends.
    feed_pin_end: bool,
    feed_epoch: u64,
    pub unread_notifs: i64,
    pub unread_dms: i64,
    pub status: String,
    pub ws_connected: bool,
    pub pending_token: Option<String>,
    /// Saved handles for the welcome picker (last-active first).
    pub saved_accounts: Vec<String>,
    /// Handles the session user follows (live Following-tab filter).
    pub following_handles: HashSet<String>,
    /// Handles the session user blocked (live feed filter).
    pub blocked_handles: HashSet<String>,
    pub(crate) cache: Cache,
    /// Session memory for instant page switches (stale-while-revalidate).
    pub(crate) view_cache: ViewCache,
    /// In-flight navigation keys so stale responses don't clobber a newer view.
    pub(crate) pending_profile: Option<String>,
    pub(crate) pending_thread: Option<String>,
    pub(crate) pending_dm: Option<String>,
    pub reminders: ReminderState,
    anim_started: Option<Instant>,
    /// Sidebar ASCII logo wave (bold sweep) after returning home.
    pub logo_wave_at: Option<Instant>,
    /// App clock for ambient UI motion (live pulse, gradients).
    pub ui_epoch: Instant,
    event_rx: Option<mpsc::UnboundedReceiver<ClientEvent>>,
    pub(crate) event_tx: mpsc::UnboundedSender<ClientEvent>,
    should_quit: bool,
    /// Bumped to cancel in-flight WebSocket reconnect loops (logout / quit).
    ws_generation: Arc<AtomicU64>,
    quit_flag: Arc<AtomicBool>,
}

pub(crate) enum ClientEvent {
    WsConnected,
    WsDisconnected,
    NewPost(Post),
    PostDeleted(Post),
    PostEdited(Post),
    LikeUpdated {
        post_id: String,
        like_count: i64,
    },
    Notification(Notification),
    DmMessage(DmMessage),
    Status(String),
    /// Background feed refresh finished.
    FeedLoaded {
        epoch: u64,
        following: bool,
        posts: Vec<Post>,
        append: bool,
        next_cursor: Option<String>,
        unread: Option<i64>,
        unread_dms: Option<i64>,
        following_handles: Option<Vec<String>>,
        blocked_handles: Option<Vec<String>>,
    },
    FeedMoreFailed {
        epoch: u64,
    },
    ProfileLoaded {
        profile: UserProfile,
        posts: Vec<Post>,
        append: bool,
        next_cursor: Option<String>,
    },
    ThreadLoaded {
        focus_id: String,
        root_id: String,
        posts: Vec<Post>,
    },
    NotificationsLoaded {
        items: Vec<Notification>,
        unread: i64,
    },
    DmListLoaded {
        items: Vec<DmConversation>,
    },
    DmThreadLoaded {
        request_key: String,
        conversation_id: String,
        peer_handle: String,
        messages: Vec<DmMessage>,
    },
    MentionSearched {
        request: u64,
        items: Vec<String>,
    },
}

impl App {
    pub async fn new(config: Config) -> Result<Self> {
        let api = Api::new(&config.server_url)?;
        let cache = Cache::open()?;
        let reminders = ReminderState::load().unwrap_or_default();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let saved_accounts = Self::load_saved_accounts(reminders.handle.as_deref());
        let palette = Palette::from_theme(&config.theme);
        let mut app = Self {
            config,
            palette,
            api,
            screen: Screen::WelcomeMenu { selected: 0 },
            session: None,
            posts: cache.recent(50).unwrap_or_default(),
            selected: 0,
            feed_scroll: 0,
            thread: None,
            feed_tab: FeedTab::All,
            feed_next_cursor: None,
            feed_has_more: true,
            feed_loading_more: false,
            feed_pin_end: false,
            feed_epoch: 0,
            unread_notifs: 0,
            unread_dms: 0,
            status: String::new(),
            ws_connected: false,
            pending_token: None,
            saved_accounts,
            following_handles: HashSet::new(),
            blocked_handles: HashSet::new(),
            cache,
            view_cache: ViewCache::new(),
            pending_profile: None,
            pending_thread: None,
            pending_dm: None,
            reminders,
            anim_started: None,
            logo_wave_at: None,
            ui_epoch: Instant::now(),
            event_rx: Some(event_rx),
            event_tx,
            should_quit: false,
            ws_generation: Arc::new(AtomicU64::new(0)),
            quit_flag: Arc::new(AtomicBool::new(false)),
        };
        // Seed for-you tab cache from disk so first paint is instant.
        if !app.posts.is_empty() {
            app.view_cache.set_feed(FeedTab::All, app.posts.clone());
        }
        // Silent auto-login only when exactly one saved account + auto_login on.
        // With multiple accounts, land on the picker (last-active preselected at top).
        if app.reminders.auto_login && app.saved_accounts.len() == 1 {
            let handle = app.saved_accounts[0].clone();
            if let Err(e) = app.login_saved_handle(&handle).await {
                tracing::info!(?e, "auto-login failed");
                app.status = format!("could not resume @{handle}: {e:#}");
            }
        } else if app.saved_accounts.is_empty() {
            app.status.clear();
        } else {
            app.status = format!(
                "{} saved account(s) — Enter to open, or pick another",
                app.saved_accounts.len()
            );
        }
        Ok(app)
    }

    /// Saved handles: last-active first (if present), then the rest A–Z.
    fn load_saved_accounts(last_active: Option<&str>) -> Vec<String> {
        let mut handles = keychain::list_saved_handles().unwrap_or_default();
        if let Some(last) = last_active {
            if let Some(pos) = handles.iter().position(|h| h == last) {
                let h = handles.remove(pos);
                handles.insert(0, h);
            } else if keychain::load_token(last).ok().flatten().is_some() {
                // Token may live only in keyring; still offer it first.
                handles.insert(0, last.to_string());
            }
        }
        handles
    }

    pub fn menu_entries(&self) -> Vec<MenuEntry> {
        let mut entries: Vec<MenuEntry> = self
            .saved_accounts
            .iter()
            .cloned()
            .map(MenuEntry::Account)
            .collect();
        entries.push(MenuEntry::Register);
        entries.push(MenuEntry::ManualLogin);
        entries.push(MenuEntry::Quit);
        entries
    }

    fn refresh_saved_accounts(&mut self) {
        self.saved_accounts = Self::load_saved_accounts(self.reminders.handle.as_deref());
    }

    /// Log in with a stored identity token for `handle` (no typing).
    async fn login_saved_handle(&mut self, handle: &str) -> Result<()> {
        let token = keychain::load_token(handle)?
            .with_context(|| format!("no saved token for @{handle}"))?;
        let resp = self.api.login(handle, &token).await?;
        self.session = Some(Session {
            handle: resp.user.handle.clone(),
            user_id: resp.user.id,
        });
        let _ = self.reminders.set_active_handle(&resp.user.handle);
        let _ = keychain::store_token(&resp.user.handle, &token);
        self.refresh_saved_accounts();
        self.enter_feed().await?;
        self.status = format!("welcome back @{handle}");
        Ok(())
    }

    pub async fn run<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        let mut event_rx = self.event_rx.take().expect("event_rx");

        loop {
            // welcome animation tick
            if let Screen::WelcomeAnim { frame_idx } = &mut self.screen {
                if let Some(start) = self.anim_started {
                    let elapsed = start.elapsed().as_millis();
                    *frame_idx = ((elapsed / 120) % 12) as u8;
                    if elapsed > 1800 {
                        self.screen = Screen::UnlockConfirm {
                            expected: self.pending_token.clone().unwrap_or_default(),
                            input: String::new(),
                            error: None,
                            can_reveal: true,
                        };
                        self.anim_started = None;
                    }
                }
            }
            if let Screen::ShowToken { flash, .. } = &mut self.screen {
                if *flash > 0 {
                    *flash -= 1;
                }
            }

            terminal
                .draw(|f| ui::draw(f, self))
                .map_err(|e| anyhow::anyhow!("draw: {e}"))?;

            // drain async events
            while let Ok(ev) = event_rx.try_recv() {
                self.handle_client_event(ev);
            }

            if self.should_quit {
                break;
            }

            // Smoother redraw while logo wave / ambient motion runs.
            let tick = if self.ui_needs_fast_tick() {
                Duration::from_millis(30)
            } else {
                Duration::from_millis(50)
            };
            if event::poll(tick)? {
                match event::read()? {
                    CEvent::Key(key) => self.handle_key(key).await?,
                    CEvent::Paste(text) => self.handle_paste(&text).await?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Seconds since UI clock started (ambient animation phase).
    pub fn ui_secs(&self) -> f32 {
        self.ui_epoch.elapsed().as_secs_f32()
    }

    fn ui_needs_fast_tick(&self) -> bool {
        self.logo_wave_secs().is_some()
            || matches!(
                self.screen,
                Screen::WelcomeAnim { .. }
                    | Screen::WelcomeMenu { .. }
                    | Screen::Login { .. }
                    | Screen::Settings { .. }
                    | Screen::Feed
            )
    }

    fn open_settings(&mut self, back: SettingsBack) {
        self.screen = Screen::Settings {
            server_url: self.config.server_url.clone(),
            auto_login: self.reminders.auto_login,
            focus: 0,
            error: None,
            back,
        };
    }

    fn leave_settings(&mut self, back: SettingsBack) {
        self.screen = match back {
            SettingsBack::WelcomeMenu { selected } => Screen::WelcomeMenu { selected },
            SettingsBack::Login {
                handle,
                token_input,
                focus,
                error,
            } => Screen::Login {
                handle,
                token_input,
                focus,
                error,
            },
        };
    }

    fn apply_settings(&mut self, server_url: &str, auto_login: bool) -> Result<(), String> {
        let url = server_url.trim();
        if url.is_empty() {
            return Err("server URL cannot be empty".into());
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("URL must start with http:// or https://".into());
        }
        // Basic sanity — reject spaces and control chars.
        if url.chars().any(|c| c.is_whitespace()) {
            return Err("URL must not contain spaces".into());
        }

        let normalized = url.trim_end_matches('/').to_string();
        self.config.server_url = normalized.clone();
        if let Err(e) = self.config.save() {
            return Err(format!("could not save config: {e:#}"));
        }
        self.api.set_server_url(&normalized);

        self.reminders.auto_login = auto_login;
        if let Err(e) = self.reminders.save() {
            return Err(format!("could not save prefs: {e:#}"));
        }

        self.status = format!("server → {normalized}");
        Ok(())
    }

    fn handle_client_event(&mut self, ev: ClientEvent) {
        match ev {
            ClientEvent::WsConnected => {
                self.ws_connected = true;
                self.status = "live".into();
            }
            ClientEvent::WsDisconnected => {
                self.ws_connected = false;
                self.status = "reconnecting…".into();
            }
            ClientEvent::NewPost(post) => {
                let blocked = self.blocked_handles.contains(&post.author_handle);
                let me = self.session.as_ref().map(|s| s.handle.as_str());
                let on_following = self.feed_tab == FeedTab::Following;
                let visible_on_tab = !on_following
                    || me == Some(post.author_handle.as_str())
                    || self.following_handles.contains(&post.author_handle);

                // Feed only shows top-level posts that match the active tab / blocks.
                if post.parent_id.is_none()
                    && !blocked
                    && visible_on_tab
                    && !self.posts.iter().any(|p| p.id == post.id)
                {
                    self.posts.insert(0, post.clone());
                    if !on_following {
                        let _ = self.cache.upsert_posts(std::slice::from_ref(&post));
                    }
                } else if let Some(parent_id) = post.parent_id.as_ref() {
                    if !blocked {
                        if let Some(p) = self.posts.iter_mut().find(|p| p.id == *parent_id) {
                            p.reply_count += 1;
                        }
                    }
                }
                // Append into open thread when the post belongs to it.
                if !blocked {
                    if let Some(th) = self.thread.as_mut() {
                        let belongs = post.id == th.root_id
                            || post
                                .parent_id
                                .as_ref()
                                .map(|pid| posts_contains(&th.posts, pid))
                                .unwrap_or(false);
                        if belongs && !posts_contains(&th.posts, &post.id) {
                            th.posts.push(post.clone());
                            if let Some(pid) = post.parent_id.as_ref() {
                                if let Some(p) = th.posts.iter_mut().find(|p| p.id == *pid) {
                                    p.reply_count += 1;
                                }
                            }
                        }
                    }
                }
            }
            ClientEvent::PostDeleted(post) => {
                Self::apply_deleted(&mut self.posts, &post);
                crate::client::interactions::update_post(&mut self.posts, &post);
                let _ = self.cache.upsert_posts(std::slice::from_ref(&post));
                if let Some(th) = self.thread.as_mut() {
                    Self::apply_deleted(&mut th.posts, &post);
                    crate::client::interactions::update_post(&mut th.posts, &post);
                }
            }
            ClientEvent::PostEdited(post) => {
                Self::apply_edited(&mut self.posts, &post);
                crate::client::interactions::update_post(&mut self.posts, &post);
                let _ = self.cache.upsert_posts(std::slice::from_ref(&post));
                if let Some(th) = self.thread.as_mut() {
                    Self::apply_edited(&mut th.posts, &post);
                    crate::client::interactions::update_post(&mut th.posts, &post);
                }
            }
            ClientEvent::LikeUpdated {
                post_id,
                like_count,
            } => {
                Self::apply_like_count(&mut self.posts, &post_id, like_count);
                if let Some(th) = self.thread.as_mut() {
                    Self::apply_like_count(&mut th.posts, &post_id, like_count);
                }
            }
            ClientEvent::Notification(n) => {
                // Server already filters; still gate on session user_id.
                if self
                    .session
                    .as_ref()
                    .map(|s| s.user_id == n.user_id)
                    .unwrap_or(false)
                {
                    self.unread_notifs = self.unread_notifs.saturating_add(1);
                    self.status = format!("notif: {} from @{}", n.kind, n.actor_handle);
                }
            }
            ClientEvent::DmMessage(msg) => {
                // Server only delivers DMs for conversations we participate in.
                let mine = self
                    .session
                    .as_ref()
                    .map(|s| s.user_id == msg.sender_id)
                    .unwrap_or(false);
                self.view_cache.touch_dm_preview(&msg);
                if let Screen::DmThread { state } = &mut self.screen {
                    let for_this = state.conversation_id == msg.conversation_id
                        || (state.conversation_id.is_empty()
                            && state.peer_handle == msg.sender_handle);
                    if for_this && !state.messages.iter().any(|m| m.id == msg.id) {
                        if state.conversation_id.is_empty() {
                            state.conversation_id = msg.conversation_id.clone();
                        }
                        state.messages.push(msg.clone());
                        state.scroll = state.messages.len().saturating_sub(1);
                    }
                }
                if !mine {
                    let viewing = matches!(
                        &self.screen,
                        Screen::DmThread { state }
                            if state.conversation_id == msg.conversation_id
                                || (state.conversation_id.is_empty()
                                    && state.peer_handle == msg.sender_handle)
                    );
                    if viewing {
                        self.view_cache.clear_dm_unread(&msg.conversation_id);
                    } else {
                        self.view_cache.bump_dm_unread(&msg.conversation_id, true);
                        self.unread_dms = self.unread_dms.saturating_add(1);
                        self.status = format!("✉ message from @{}", msg.sender_handle);
                    }
                }
            }
            ClientEvent::Status(s) => self.status = s,
            ClientEvent::FeedLoaded {
                epoch,
                following,
                posts,
                append,
                next_cursor,
                unread,
                unread_dms,
                following_handles,
                blocked_handles,
            } => {
                if epoch != self.feed_epoch {
                    return;
                }
                self.feed_loading_more = false;
                let tab = if following {
                    FeedTab::Following
                } else {
                    FeedTab::All
                };
                if !following {
                    let _ = self.cache.upsert_posts(&posts);
                }
                if let Some(u) = unread {
                    self.unread_notifs = u;
                }
                if let Some(u) = unread_dms {
                    self.unread_dms = u;
                }
                if let Some(h) = following_handles {
                    self.following_handles = h.into_iter().collect();
                }
                if let Some(h) = blocked_handles {
                    self.blocked_handles = h.into_iter().collect();
                }
                // Keep `self.posts` in sync whenever this is the active tab, even if
                // another screen is open — so Esc home / compose backdrop stay fresh.
                if (self.feed_tab == FeedTab::Following) == following {
                    let keep = self.posts.get(self.selected).map(|p| p.id.clone());
                    let incoming_len = posts.len() as i64;
                    if append {
                        self.posts = merge_posts_append(&self.posts, posts);
                    } else {
                        self.posts = posts;
                    }
                    self.feed_next_cursor = next_cursor.clone();
                    self.feed_has_more = incoming_len >= FEED_PAGE && next_cursor.is_some();
                    self.view_cache.stash_feed_page(
                        tab,
                        self.posts.clone(),
                        self.feed_next_cursor.clone(),
                        self.feed_has_more,
                    );
                    if self.feed_pin_end && append && !self.posts.is_empty() {
                        self.selected = self.posts.len() - 1;
                        self.ensure_feed_visible();
                        if !self.feed_has_more {
                            self.feed_pin_end = false;
                        }
                    } else if let Some(id) = keep {
                        if let Some(i) = self.posts.iter().position(|p| p.id == id) {
                            self.selected = i;
                        } else if !append {
                            self.selected = 0;
                            self.feed_scroll = 0;
                        }
                    } else if !append {
                        self.selected = 0;
                        self.feed_scroll = 0;
                    }
                    if matches!(self.screen, Screen::Feed) {
                        let more = if self.feed_has_more { "+" } else { "" };
                        self.status = format!(
                            "{} · {}{more} posts",
                            self.feed_tab.label(),
                            self.posts.len()
                        );
                    }
                } else if !append {
                    self.view_cache
                        .stash_feed_page(tab, posts, next_cursor, true);
                }
            }
            ClientEvent::FeedMoreFailed { epoch } => {
                if epoch == self.feed_epoch {
                    self.feed_loading_more = false;
                }
            }
            ClientEvent::ProfileLoaded {
                profile,
                posts,
                append,
                next_cursor,
            } => {
                let handle = profile.handle.to_ascii_lowercase();
                let _ = self.cache.upsert_posts(&posts);
                let apply = self.pending_profile.as_deref() == Some(handle.as_str())
                    || matches!(
                        &self.screen,
                        Screen::Profile { state } if state.profile.handle.eq_ignore_ascii_case(&handle)
                    );
                if apply {
                    self.pending_profile = None;
                    let incoming_len = posts.len() as i64;
                    let (selected, merged) = if let Screen::Profile { state } = &self.screen {
                        let merged = if append {
                            merge_posts_append(&state.posts, posts)
                        } else {
                            posts
                        };
                        let selected = state
                            .posts
                            .get(state.selected)
                            .and_then(|p| merged.iter().position(|q| q.id == p.id))
                            .unwrap_or(if append { state.selected } else { 0 });
                        (selected, merged)
                    } else {
                        (0, posts)
                    };
                    let has_more = incoming_len >= FEED_PAGE && next_cursor.is_some();
                    self.view_cache.put_profile_page(
                        profile.clone(),
                        merged.clone(),
                        next_cursor.clone(),
                        has_more,
                    );
                    self.screen = Screen::Profile {
                        state: ProfileState {
                            profile,
                            posts: merged,
                            selected,
                            next_cursor,
                            has_more,
                        },
                    };
                    self.status = format!("@{handle}");
                } else if !append {
                    self.view_cache
                        .put_profile_page(profile, posts, next_cursor, true);
                }
            }
            ClientEvent::ThreadLoaded {
                focus_id,
                root_id,
                posts,
            } => {
                let _ = self.cache.upsert_posts(&posts);
                self.view_cache.put_thread(root_id.clone(), posts.clone());
                let want = self.pending_thread.as_deref() == Some(focus_id.as_str())
                    || matches!(
                        &self.thread,
                        Some(th) if th.root_id == root_id
                            || th.posts.iter().any(|p| p.id == focus_id)
                    );
                if want
                    && matches!(
                        self.screen,
                        Screen::Thread
                            | Screen::Compose {
                                return_to: ReturnTo::Thread,
                                ..
                            }
                            | Screen::Help {
                                return_to: ReturnTo::Thread,
                                ..
                            }
                            | Screen::ConfirmDelete {
                                return_to: ReturnTo::Thread,
                                ..
                            }
                    )
                {
                    self.pending_thread = None;
                    let selected = posts.iter().position(|p| p.id == focus_id).unwrap_or(0);
                    self.thread = Some(ThreadState {
                        root_id,
                        posts,
                        selected,
                    });
                    if matches!(self.screen, Screen::Thread) {
                        self.status = "thread".into();
                    }
                }
            }
            ClientEvent::NotificationsLoaded { items, unread } => {
                self.view_cache.put_notifications(items.clone());
                self.unread_notifs = unread;
                if matches!(self.screen, Screen::Notifications { .. }) {
                    let selected = if let Screen::Notifications { state } = &self.screen {
                        state.selected.min(items.len().saturating_sub(1))
                    } else {
                        0
                    };
                    self.screen = Screen::Notifications {
                        state: NotifState {
                            items,
                            selected,
                            unread,
                        },
                    };
                    self.status = "notifications".into();
                }
            }
            ClientEvent::DmListLoaded { items } => {
                self.view_cache.put_dm_list(items.clone());
                self.unread_dms = self.view_cache.unread_dms();
                if matches!(self.screen, Screen::DmList { .. }) {
                    let selected = if let Screen::DmList { state } = &self.screen {
                        state.selected.min(items.len().saturating_sub(1))
                    } else {
                        0
                    };
                    self.screen = Screen::DmList {
                        state: DmListState { items, selected },
                    };
                    self.status = "messages · n new · Enter open".into();
                }
            }
            ClientEvent::DmThreadLoaded {
                request_key,
                conversation_id,
                peer_handle,
                messages,
            } => {
                self.view_cache.put_dm_thread(
                    conversation_id.clone(),
                    peer_handle.clone(),
                    messages.clone(),
                );
                let apply = self.pending_dm.as_deref() == Some(request_key.as_str())
                    || matches!(
                        &self.screen,
                        Screen::DmThread { state }
                            if state.conversation_id == conversation_id
                                || (state.conversation_id.is_empty()
                                    && state.peer_handle.eq_ignore_ascii_case(&peer_handle))
                    );
                if apply {
                    self.pending_dm = None;
                    let n = messages.len();
                    let draft = if let Screen::DmThread { state } = &self.screen {
                        state.draft.clone()
                    } else {
                        let mut t = TextArea::default();
                        t.set_placeholder_text("Write a message…");
                        t
                    };
                    self.screen = Screen::DmThread {
                        state: DmThreadState {
                            conversation_id,
                            peer_handle,
                            messages,
                            draft,
                            error: None,
                            scroll: n.saturating_sub(1),
                        },
                    };
                }
            }
            ClientEvent::MentionSearched { request, items } => {
                if let Screen::Compose { mention_ac, .. } = &mut self.screen {
                    if mention_ac.request == request {
                        mention_ac.items = items;
                        mention_ac.selected = 0;
                    }
                }
            }
        }
    }

    fn apply_edited(list: &mut [Post], post: &Post) {
        if let Some(p) = list.iter_mut().find(|p| p.id == post.id) {
            *p = post.clone();
        }
    }

    pub(crate) fn apply_deleted(list: &mut [Post], post: &Post) {
        if let Some(p) = list.iter_mut().find(|p| p.id == post.id) {
            p.content = "[Deleted]".into();
            p.deleted_at = post.deleted_at;
        }
    }

    fn apply_like_count(list: &mut [Post], post_id: &str, like_count: i64) {
        if let Some(p) = list.iter_mut().find(|p| p.id == post_id) {
            p.like_count = like_count;
            // Keep liked_by_me as-is (we may have set it ourselves).
        }
    }

    async fn handle_paste(&mut self, text: &str) -> Result<()> {
        // Compose: pasted image path or file:// URI attaches the file.
        if matches!(
            &self.screen,
            Screen::Compose {
                attach_path: None,
                edit_id: None,
                ..
            }
        ) {
            if let Some(path) = crate::client::clipboard::path_from_paste(text) {
                match self.attach_image_from_path(&path).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        if let Screen::Compose { error, .. } = &mut self.screen {
                            *error = Some(e);
                        }
                        return Ok(());
                    }
                }
            }
        }

        match &mut self.screen {
            Screen::UnlockConfirm { input, error, .. } => {
                input.push_str(text);
                *error = None;
            }
            Screen::Login {
                handle,
                token_input,
                focus,
                error,
            } => {
                if *focus == 0 {
                    handle.push_str(text);
                } else {
                    token_input.push_str(text);
                }
                *error = None;
            }
            Screen::RegisterHandle { input, error } => {
                input.push_str(text);
                *error = None;
            }
            Screen::Settings {
                server_url,
                focus,
                error,
                ..
            } if *focus == 0 => {
                server_url.push_str(text.trim());
                *error = None;
            }
            Screen::Compose {
                textarea,
                attach_path,
                error,
                ..
            } => {
                if let Some(path) = attach_path {
                    path.push_str(text.trim_matches(|c| c == '\n' || c == '\r'));
                    *error = None;
                } else {
                    textarea.insert_str(text);
                    self.composer_mention_tick();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Upload a local image file and attach it to the open composer.
    async fn attach_image_from_path(
        &mut self,
        path: &std::path::Path,
    ) -> std::result::Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if bytes.len() > MEDIA_MAX_BYTES {
            return Err(format!("max {MEDIA_MAX_BYTES} bytes"));
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .to_string();
        self.attach_image_bytes(&name, bytes).await
    }

    /// Pull an image from the system clipboard (wl-paste / xclip) and attach it.
    async fn attach_image_from_clipboard(&mut self) -> std::result::Result<(), String> {
        match crate::client::clipboard::read_image() {
            Ok(Some(img)) => self.attach_image_bytes(&img.filename, img.bytes).await,
            Ok(None) => Err("no image on clipboard (copy a screenshot first)".into()),
            Err(e) => Err(format!("{e:#}")),
        }
    }

    async fn attach_image_bytes(
        &mut self,
        name: &str,
        bytes: Vec<u8>,
    ) -> std::result::Result<(), String> {
        let Screen::Compose {
            images,
            edit_id,
            error,
            attach_path,
            ..
        } = &mut self.screen
        else {
            return Err("open the composer first (n)".into());
        };
        if edit_id.is_some() {
            return Err("cannot attach images while editing".into());
        }
        if images.len() >= MEDIA_MAX_PER_POST {
            return Err(format!("max {MEDIA_MAX_PER_POST} images"));
        }
        match self.api.upload_media(name, bytes).await {
            Ok(info) => {
                images.push(info);
                *attach_path = None;
                *error = None;
                self.status = format!("attached image ({}/{})", images.len(), MEDIA_MAX_PER_POST);
                Ok(())
            }
            Err(e) => Err(format!("upload: {e:#}")),
        }
    }

    fn request_quit(&mut self) {
        self.should_quit = true;
        self.quit_flag.store(true, Ordering::SeqCst);
        // Cancel any WS reconnect loops.
        self.ws_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Logout: stop silent auto-login, KEEP all identity tokens, show picker.
    pub(crate) fn logout(&mut self) {
        let who = self.session.as_ref().map(|s| s.handle.clone());
        self.session = None;
        let _ = self.reminders.clear_session();
        let _ = self.cache.clear();
        self.view_cache.clear();
        self.pending_profile = None;
        self.pending_thread = None;
        self.pending_dm = None;
        self.api.clear_tokens();
        self.following_handles.clear();
        self.blocked_handles.clear();
        self.posts.clear();
        self.feed_next_cursor = None;
        self.feed_has_more = true;
        self.feed_pin_end = false;
        self.unread_dms = 0;
        self.unread_notifs = 0;
        self.ws_connected = false;
        // Stop old WebSocket loop so it stops spamming status after logout.
        self.ws_generation.fetch_add(1, Ordering::SeqCst);
        self.refresh_saved_accounts();
        let sel = who
            .as_ref()
            .and_then(|h| self.saved_accounts.iter().position(|a| a == h))
            .unwrap_or(0);
        self.screen = Screen::WelcomeMenu { selected: sel };
        let saved = self.saved_accounts.len();
        self.status = match who {
            Some(h) => format!("logged out @{h} · pick an account ({saved} saved)"),
            None => format!("logged out · {saved} account(s) saved"),
        };
    }

    /// True when letter keys should insert text, not trigger shortcuts.
    fn is_typing_screen(&self) -> bool {
        matches!(
            &self.screen,
            Screen::RegisterHandle { .. }
                | Screen::UnlockConfirm { .. }
                | Screen::BackupPassphrase { .. }
                | Screen::Login { .. }
                | Screen::Settings { focus: 0, .. }
                | Screen::Compose { .. }
                | Screen::EditBio { .. }
                | Screen::BlockUser { .. }
                | Screen::DmNew { .. }
                | Screen::DmThread { .. }
                | Screen::Palette { .. }
                | Screen::Search {
                    state: SearchState { editing: true, .. },
                }
        )
    }

    /// Letter from a key press, lowercased (Caps Lock / Shift-safe for shortcuts).
    fn shortcut_letter(key: &KeyEvent) -> Option<char> {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            _ => None,
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Ignore key release/repeat (Ghostty / kitty keyboard protocol).
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        // Always allow force-quit.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return Ok(());
        }

        // Global quit on q/Q when not typing into a field.
        if !self.is_typing_screen()
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && Self::shortcut_letter(&key) == Some('q')
        {
            self.request_quit();
            return Ok(());
        }

        // Welcome menu: take selection out so we can call &mut self methods freely.
        if matches!(self.screen, Screen::WelcomeMenu { .. }) {
            let mut selected = match &self.screen {
                Screen::WelcomeMenu { selected } => *selected,
                _ => 0,
            };
            let entries = self.menu_entries();
            let n = entries.len().max(1);
            match key.code {
                KeyCode::Char(c)
                    if matches!(c, 'j' | 'J') && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    selected = (selected + 1) % n;
                    self.screen = Screen::WelcomeMenu { selected };
                }
                KeyCode::Down => {
                    selected = (selected + 1) % n;
                    self.screen = Screen::WelcomeMenu { selected };
                }
                KeyCode::Char(c)
                    if matches!(c, 'k' | 'K') && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    selected = selected.checked_sub(1).unwrap_or(n - 1);
                    self.screen = Screen::WelcomeMenu { selected };
                }
                KeyCode::Up => {
                    selected = selected.checked_sub(1).unwrap_or(n - 1);
                    self.screen = Screen::WelcomeMenu { selected };
                }
                KeyCode::Enter => {
                    let idx = selected.min(entries.len().saturating_sub(1));
                    match entries.get(idx).cloned() {
                        Some(MenuEntry::Account(handle)) => {
                            if let Err(e) = self.login_saved_handle(&handle).await {
                                self.status = format!("login @{handle} failed: {e:#}");
                                self.refresh_saved_accounts();
                                self.screen = Screen::WelcomeMenu { selected: idx };
                            }
                        }
                        Some(MenuEntry::Register) => {
                            self.screen = Screen::RegisterHandle {
                                input: String::new(),
                                error: None,
                            };
                        }
                        Some(MenuEntry::ManualLogin) => {
                            self.screen = Screen::Login {
                                handle: String::new(),
                                token_input: String::new(),
                                focus: 0,
                                error: None,
                            };
                        }
                        Some(MenuEntry::Quit) | None => self.request_quit(),
                    }
                }
                // Settings: change server URL, auto-login, etc.
                KeyCode::Char('t') | KeyCode::Char('T')
                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.open_settings(SettingsBack::WelcomeMenu { selected });
                }
                _ => {
                    self.screen = Screen::WelcomeMenu { selected };
                }
            }
            return Ok(());
        }

        // Settings screen (pre-auth).
        if matches!(self.screen, Screen::Settings { .. }) {
            return self.handle_settings_key(key);
        }

        // Command palette: open with Ctrl+P / Ctrl+K on main screens.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key.code,
                KeyCode::Char('p') | KeyCode::Char('P') | KeyCode::Char('k') | KeyCode::Char('K')
            )
            && !matches!(self.screen, Screen::Palette { .. })
            && self.palette_openable()
        {
            self.open_palette();
            return Ok(());
        }
        if matches!(self.screen, Screen::Palette { .. }) {
            return self.handle_palette_key(key).await;
        }

        // Phase C screens
        if self.handle_phase_c_key(key).await? {
            return Ok(());
        }

        // Thread view keys
        if matches!(self.screen, Screen::Thread) {
            return self.handle_thread_key(key).await;
        }

        match &mut self.screen {
            Screen::RegisterHandle { input, error } => match key.code {
                KeyCode::Esc => {
                    self.refresh_saved_accounts();
                    self.screen = Screen::WelcomeMenu { selected: 0 };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if input.len() < 32 {
                        input.push(c);
                    }
                    *error = None;
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    let handle = input.clone();
                    match self.api.register(&handle).await {
                        Ok(resp) => {
                            self.pending_token = Some(resp.token.clone());
                            self.session = Some(Session {
                                handle: resp.user.handle.clone(),
                                user_id: resp.user.id,
                            });
                            self.screen = Screen::SaveAck;
                        }
                        Err(e) => *error = Some(format!("{e:#}")),
                    }
                }
                _ => {}
            },
            Screen::SaveAck => match key.code {
                KeyCode::Enter => {
                    if let Some(token) = self.pending_token.clone() {
                        // Always dump to file + try clipboard immediately.
                        let export_status = crate::client::clipboard::export_token(&token)
                            .unwrap_or_else(|e| format!("export failed: {e:#}"));
                        self.screen = Screen::ShowToken {
                            token,
                            export_status,
                            flash: 30,
                        };
                    }
                }
                KeyCode::Esc => {
                    self.pending_token = None;
                    self.session = None;
                    self.screen = Screen::WelcomeMenu { selected: 0 };
                }
                _ => {}
            },
            Screen::ShowToken {
                token,
                export_status,
                flash,
            } => match key.code {
                KeyCode::Char('c') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    match crate::client::clipboard::export_token(token) {
                        Ok(status) => {
                            *export_status = status;
                            *flash = 30;
                        }
                        Err(e) => {
                            *export_status = format!("copy failed: {e:#}");
                            *flash = 30;
                        }
                    }
                }
                KeyCode::Enter => {
                    self.anim_started = Some(Instant::now());
                    self.screen = Screen::WelcomeAnim { frame_idx: 0 };
                }
                _ => {}
            },
            Screen::WelcomeAnim { .. } => {
                // auto-advances; Enter skips
                if key.code == KeyCode::Enter {
                    self.screen = Screen::UnlockConfirm {
                        expected: self.pending_token.clone().unwrap_or_default(),
                        input: String::new(),
                        error: None,
                        can_reveal: true,
                    };
                    self.anim_started = None;
                }
            }
            Screen::UnlockConfirm {
                expected,
                input,
                error,
                can_reveal,
            } => match key.code {
                // Must NOT use A–Z/a–z/0–9 — those appear in the token and break paste.
                KeyCode::F(1) if *can_reveal => {
                    let token = expected.clone();
                    let export_status = crate::client::clipboard::export_token(&token)
                        .unwrap_or_else(|e| format!("export failed: {e:#}"));
                    self.screen = Screen::ShowToken {
                        token,
                        export_status,
                        flash: 20,
                    };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.push(c);
                    *error = None;
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    let cleaned = strip_token(input);
                    if validate_token(&cleaned).is_err() {
                        *error = Some(format!(
                            "need {} alphanumerics, got {}",
                            crate::shared::TOKEN_LEN,
                            cleaned.len()
                        ));
                    } else if cleaned != *expected {
                        *error = Some("token does not match — F1 to view again".into());
                    } else {
                        let handle = self
                            .session
                            .as_ref()
                            .map(|s| s.handle.clone())
                            .unwrap_or_default();
                        keychain::store_token(&handle, &cleaned)?;
                        let _ = self.reminders.mark_registered(&handle);
                        let _ = self.reminders.set_active_handle(&handle);
                        self.refresh_saved_accounts();
                        self.pending_token = None;
                        self.screen = Screen::BackupPrompt { selected: 0 };
                    }
                }
                _ => {}
            },
            Screen::BackupPrompt { selected } => match key.code {
                KeyCode::Char('j') | KeyCode::Down => *selected = (*selected + 1) % 2,
                KeyCode::Char('k') | KeyCode::Up => *selected = 1 - *selected,
                KeyCode::Enter => {
                    if *selected == 0 {
                        self.screen = Screen::BackupPassphrase {
                            input: String::new(),
                            confirm: String::new(),
                            stage: 0,
                            error: None,
                        };
                    } else {
                        self.enter_feed().await?;
                    }
                }
                _ => {}
            },
            Screen::BackupPassphrase {
                input,
                confirm,
                stage,
                error,
            } => match key.code {
                KeyCode::Esc => {
                    self.enter_feed().await?;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if *stage == 0 {
                        input.push(c);
                    } else {
                        confirm.push(c);
                    }
                    *error = None;
                }
                KeyCode::Backspace => {
                    if *stage == 0 {
                        input.pop();
                    } else {
                        confirm.pop();
                    }
                }
                KeyCode::Enter => {
                    if *stage == 0 {
                        if input.is_empty() {
                            *error = Some("passphrase required".into());
                        } else {
                            *stage = 1;
                        }
                    } else if input != confirm {
                        *error = Some("passphrases do not match".into());
                        confirm.clear();
                        *stage = 0;
                    } else {
                        let token = keychain::load_token(
                            &self
                                .session
                                .as_ref()
                                .map(|s| s.handle.clone())
                                .unwrap_or_default(),
                        )?
                        .context("token missing from keychain")?;
                        backup::write_backup(&token, input)?;
                        self.enter_feed().await?;
                    }
                }
                _ => {}
            },
            Screen::Login {
                handle,
                token_input,
                focus,
                error,
            } => match key.code {
                KeyCode::Esc => {
                    self.refresh_saved_accounts();
                    // Select "Log in with token" row (after N saved accounts).
                    let n = self.saved_accounts.len();
                    self.screen = Screen::WelcomeMenu {
                        selected: n + 1, // Register=n, ManualLogin=n+1
                    };
                }
                // Ctrl+T → settings (plain `t` is typed into handle/token).
                KeyCode::Char('t') | KeyCode::Char('T')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let back = SettingsBack::Login {
                        handle: handle.clone(),
                        token_input: token_input.clone(),
                        focus: *focus,
                        error: error.clone(),
                    };
                    self.open_settings(back);
                }
                KeyCode::Tab => *focus = 1 - *focus,
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if *focus == 0 {
                        if handle.len() < 32 {
                            handle.push(c);
                        }
                    } else {
                        token_input.push(c);
                    }
                    *error = None;
                }
                KeyCode::Backspace => {
                    if *focus == 0 {
                        handle.pop();
                    } else {
                        token_input.pop();
                    }
                }
                KeyCode::Enter => {
                    let h = handle.clone();
                    let t = strip_token(token_input);
                    match self.api.login(&h, &t).await {
                        Ok(resp) => {
                            keychain::store_token(&resp.user.handle, &t)?;
                            let _ = self.reminders.set_active_handle(&resp.user.handle);
                            self.session = Some(Session {
                                handle: resp.user.handle,
                                user_id: resp.user.id,
                            });
                            self.refresh_saved_accounts();
                            self.enter_feed().await?;
                        }
                        Err(e) => *error = Some(format!("{e:#}")),
                    }
                }
                _ => {}
            },
            Screen::Settings { .. } => {} // handled above
            Screen::Feed => match key.code {
                // Logout: stop silent auto-login, KEEP all identity tokens, show picker.
                KeyCode::Char('L') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.logout();
                }
                KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => {
                    if !self.posts.is_empty() {
                        self.selected = (self.selected + 1).min(self.posts.len() - 1);
                        self.ensure_feed_visible();
                    }
                    self.maybe_load_more_feed();
                }
                KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => {
                    self.selected = self.selected.saturating_sub(1);
                    self.ensure_feed_visible();
                }
                KeyCode::Char('g') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.feed_pin_end = false;
                    self.selected = 0;
                    self.feed_scroll = 0;
                }
                KeyCode::Char('G') | KeyCode::Char('g')
                    if key.modifiers.contains(KeyModifiers::SHIFT) =>
                {
                    self.feed_pin_end = true;
                    if !self.posts.is_empty() {
                        self.selected = self.posts.len() - 1;
                        self.ensure_feed_visible();
                    }
                    self.maybe_load_more_feed();
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.open_compose(None, ReturnTo::Feed);
                }
                KeyCode::Char('r') => {
                    if let Some(post) = self.posts.get(self.selected) {
                        let id = post.id.clone();
                        self.open_compose(Some(id), ReturnTo::Feed);
                    }
                }
                KeyCode::Char('R') => {
                    if let Some(post) = self.posts.get(self.selected).cloned() {
                        if post.deleted_at.is_some() {
                            self.status = "cannot quote a deleted post".into();
                        } else {
                            self.open_quote_compose(&post);
                        }
                    }
                }
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    if let Some(post) = self.posts.get(self.selected) {
                        if self.is_mine(post) && post.deleted_at.is_none() {
                            let id = post.id.clone();
                            let content = post.content.clone();
                            self.open_edit_post(&id, &content, ReturnTo::Feed);
                        } else {
                            self.status = "you can only edit your own posts".into();
                        }
                    }
                }
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.status = "refreshing…".into();
                    self.spawn_refresh_feed();
                }
                KeyCode::Tab => {
                    self.switch_feed_tab();
                }
                KeyCode::Char('/') => {
                    self.open_search().await;
                }
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    if let Err(e) = self.open_notifications().await {
                        self.status = format!("notifications: {e:#}");
                    }
                }
                KeyCode::Char('m') | KeyCode::Char('M') => {
                    if let Err(e) = self.open_dm_list().await {
                        self.status = format!("dms: {e:#}");
                    }
                }
                // p = selected author's profile (what you want on the feed).
                // Shift+p = your own profile (sidebar "Profile").
                KeyCode::Char('p') | KeyCode::Char('P')
                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let mine = key.modifiers.contains(KeyModifiers::SHIFT);
                    let handle = if mine {
                        self.session.as_ref().map(|s| s.handle.clone())
                    } else {
                        self.posts
                            .get(self.selected)
                            .map(|p| p.author_handle.clone())
                            .or_else(|| self.session.as_ref().map(|s| s.handle.clone()))
                    };
                    match handle {
                        Some(h) => {
                            if let Err(e) = self.open_profile(&h).await {
                                self.status = format!("profile: {e:#}");
                            }
                        }
                        None if mine => self.status = "not logged in".into(),
                        None => self.status = "no post selected".into(),
                    }
                }
                KeyCode::Char('f') | KeyCode::Char('F')
                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    // follow author of selected post
                    if let Some(post) = self.posts.get(self.selected) {
                        let h = post.author_handle.clone();
                        if self
                            .session
                            .as_ref()
                            .map(|s| s.handle == h)
                            .unwrap_or(false)
                        {
                            self.status = "that's you".into();
                        } else {
                            match self.api.follow(&h).await {
                                Ok(_) => {
                                    self.following_handles.insert(h.clone());
                                    self.status = format!("followed @{h}");
                                }
                                Err(e) => self.status = format!("follow: {e:#}"),
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(post) = self.posts.get(self.selected) {
                        let id = post.id.clone();
                        if let Err(e) = self.open_thread(&id).await {
                            self.status = format!("thread failed: {e:#}");
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    if let Some(post) = self.posts.get(self.selected).cloned() {
                        self.toggle_like_on(&post.id).await;
                    }
                }
                KeyCode::Char('o') | KeyCode::Char('O')
                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if let Some(post) = self.posts.get(self.selected).cloned() {
                        self.open_post_images(&post).await;
                    }
                }
                KeyCode::Char('d') => {
                    if let Some(post) = self.posts.get(self.selected) {
                        if self.is_mine(post) && post.deleted_at.is_none() {
                            let preview: String = post.display_content().chars().take(40).collect();
                            self.screen = Screen::ConfirmDelete {
                                post_id: post.id.clone(),
                                preview,
                                return_to: ReturnTo::Feed,
                            };
                        } else if !self.is_mine(post) {
                            self.status = "you can only delete your own posts".into();
                        }
                    }
                }
                KeyCode::Char('?') => {
                    self.screen = Screen::Help {
                        return_to: ReturnTo::Feed,
                    };
                }
                _ => {}
            },
            // Ctrl+V paste image — handled before Compose field borrow so we can call helpers.
            Screen::Compose { .. }
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V')) =>
            {
                match self.attach_image_from_clipboard().await {
                    Ok(()) => {}
                    Err(e) => {
                        if let Screen::Compose { error, .. } = &mut self.screen {
                            *error = Some(e);
                        }
                    }
                }
            }
            Screen::Compose {
                textarea,
                error,
                parent_id,
                quote,
                mention_ac,
                edit_id,
                return_to,
                color_ac_idx,
                images,
                attach_path,
            } => {
                if key.code == KeyCode::Esc {
                    if attach_path.is_some() {
                        *attach_path = None;
                        *error = None;
                        return Ok(());
                    }
                    let ret = *return_to;
                    self.return_from(ret);
                    return Ok(());
                }

                // Mention autocomplete dropdown (only on quote composers, no edit).
                let mention_open = mention_ac.query.is_some() && !mention_ac.items.is_empty();
                if mention_open && edit_id.is_none() {
                    match key.code {
                        KeyCode::Down if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let n = mention_ac.items.len();
                            mention_ac.selected = (mention_ac.selected + 1) % n;
                            return Ok(());
                        }
                        KeyCode::Up => {
                            let n = mention_ac.items.len();
                            mention_ac.selected =
                                mention_ac.selected.checked_sub(1).unwrap_or(n - 1);
                            return Ok(());
                        }
                        // Enter accepts the mention (Ctrl+S still submits).
                        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                            let handle = mention_ac.items
                                [mention_ac.selected.min(mention_ac.items.len() - 1)]
                            .clone();
                            let start = mention_ac.start;
                            let cursor =
                                crate::client::ui::compose_edit::cursor_char_offset(textarea);
                            let new_cursor = crate::client::interactions::apply_mention(
                                textarea, start, cursor, &handle,
                            );
                            *mention_ac = crate::client::interactions::MentionAc::default();
                            let lines: Vec<String> = textarea.lines().to_vec();
                            let (r, c) =
                                crate::client::ui::compose_edit::char_to_rowcol(&lines, new_cursor);
                            textarea
                                .move_cursor(tui_textarea::CursorMove::Jump(r as u16, c as u16));
                            return Ok(());
                        }
                        KeyCode::Tab => {
                            let handle = mention_ac.items
                                [mention_ac.selected.min(mention_ac.items.len() - 1)]
                            .clone();
                            let start = mention_ac.start;
                            let cursor =
                                crate::client::ui::compose_edit::cursor_char_offset(textarea);
                            let new_cursor = crate::client::interactions::apply_mention(
                                textarea, start, cursor, &handle,
                            );
                            *mention_ac = crate::client::interactions::MentionAc::default();
                            let lines: Vec<String> = textarea.lines().to_vec();
                            let (r, c) =
                                crate::client::ui::compose_edit::char_to_rowcol(&lines, new_cursor);
                            textarea
                                .move_cursor(tui_textarea::CursorMove::Jump(r as u16, c as u16));
                            return Ok(());
                        }
                        KeyCode::Esc => {
                            *mention_ac = crate::client::interactions::MentionAc::default();
                            return Ok(());
                        }
                        _ => {}
                    }
                }

                // Path entry mode for attaching a local image.
                if attach_path.is_some() {
                    match key.code {
                        KeyCode::Enter => {
                            let path = attach_path.clone().unwrap_or_default();
                            let path = path.trim().trim_matches('"').to_string();
                            if path.is_empty() {
                                *error = Some("enter a file path".into());
                                return Ok(());
                            }
                            if images.len() >= MEDIA_MAX_PER_POST {
                                *error = Some(format!("max {MEDIA_MAX_PER_POST} images"));
                                return Ok(());
                            }
                            match std::fs::read(&path) {
                                Ok(bytes) => {
                                    if bytes.len() > MEDIA_MAX_BYTES {
                                        *error = Some(format!("max {MEDIA_MAX_BYTES} bytes"));
                                        return Ok(());
                                    }
                                    let name = std::path::Path::new(&path)
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("image")
                                        .to_string();
                                    match self.api.upload_media(&name, bytes).await {
                                        Ok(info) => {
                                            images.push(info);
                                            *attach_path = None;
                                            *error = None;
                                            self.status = format!(
                                                "attached image ({}/{})",
                                                images.len(),
                                                MEDIA_MAX_PER_POST
                                            );
                                        }
                                        Err(e) => *error = Some(format!("upload: {e:#}")),
                                    }
                                }
                                Err(e) => *error = Some(format!("read {path}: {e}")),
                            }
                            return Ok(());
                        }
                        KeyCode::Backspace => {
                            if let Some(p) = attach_path.as_mut() {
                                p.pop();
                            }
                            *error = None;
                            return Ok(());
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if let Some(p) = attach_path.as_mut() {
                                p.push(c);
                            }
                            *error = None;
                            return Ok(());
                        }
                        _ => return Ok(()),
                    }
                }

                // Color autocomplete (active after typing `{` / `{re` …)
                let color_q = crate::client::ui::compose_edit::active_color_query(textarea);
                let color_suggestions = color_q
                    .as_ref()
                    .map(|q| crate::client::ui::markup::filter_colors(&q.prefix))
                    .unwrap_or_default();
                if !color_suggestions.is_empty() {
                    if *color_ac_idx >= color_suggestions.len() {
                        *color_ac_idx = 0;
                    }
                    match key.code {
                        KeyCode::Down => {
                            *color_ac_idx = (*color_ac_idx + 1) % color_suggestions.len();
                            return Ok(());
                        }
                        KeyCode::Up => {
                            *color_ac_idx = if *color_ac_idx == 0 {
                                color_suggestions.len() - 1
                            } else {
                                *color_ac_idx - 1
                            };
                            return Ok(());
                        }
                        KeyCode::Tab => {
                            if let Some(q) = color_q {
                                let name = color_suggestions[*color_ac_idx];
                                crate::client::ui::compose_edit::apply_color_completion(
                                    textarea, &q, name,
                                );
                                *color_ac_idx = 0;
                            }
                            return Ok(());
                        }
                        // Enter accepts the color only when a menu is open (still Ctrl+S to post).
                        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                            if let Some(q) = color_q {
                                let name = color_suggestions[*color_ac_idx];
                                crate::client::ui::compose_edit::apply_color_completion(
                                    textarea, &q, name,
                                );
                                *color_ac_idx = 0;
                            }
                            return Ok(());
                        }
                        _ => {}
                    }
                }

                // Markup wrap shortcuts (intercept before tui-textarea's Ctrl+B = back).
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    match key.code {
                        KeyCode::Char('b') | KeyCode::Char('B') => {
                            crate::client::ui::compose_edit::wrap_markup(textarea, "**", "**");
                            *error = None;
                            return Ok(());
                        }
                        KeyCode::Char('u') | KeyCode::Char('U') => {
                            crate::client::ui::compose_edit::wrap_markup(textarea, "__", "__");
                            *error = None;
                            return Ok(());
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') => {
                            // Ctrl+T = strikeThrough (Ctrl+I is often consumed by terminals).
                            crate::client::ui::compose_edit::wrap_markup(textarea, "~~", "~~");
                            *error = None;
                            return Ok(());
                        }
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // Ctrl+Y starts a color tag; Tab/Enter picks from autocomplete.
                            if textarea.selection_range().is_some() {
                                crate::client::ui::compose_edit::wrap_markup(
                                    textarea, "{red}", "{/}",
                                );
                            } else {
                                textarea.insert_str("{");
                            }
                            *color_ac_idx = 0;
                            *error = None;
                            return Ok(());
                        }
                        KeyCode::Char('o') | KeyCode::Char('O') => {
                            if edit_id.is_some() {
                                *error = Some("cannot attach images while editing".into());
                                return Ok(());
                            }
                            if images.len() >= MEDIA_MAX_PER_POST {
                                *error = Some(format!("max {MEDIA_MAX_PER_POST} images"));
                                return Ok(());
                            }
                            *attach_path = Some(String::new());
                            *error = None;
                            return Ok(());
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            if images.pop().is_some() {
                                *error = None;
                                self.status = format!(
                                    "removed image ({}/{})",
                                    images.len(),
                                    MEDIA_MAX_PER_POST
                                );
                            }
                            return Ok(());
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            let content = textarea.lines().join("\n");
                            let trimmed = content.trim();
                            let img_ids: Vec<String> =
                                images.iter().map(|m| m.id.clone()).collect();
                            if trimmed.is_empty() && img_ids.is_empty() {
                                *error = Some("empty post rejected".into());
                                return Ok(());
                            }
                            if trimmed.chars().count() > POST_MAX_LEN {
                                *error = Some(format!("max {POST_MAX_LEN} characters"));
                                return Ok(());
                            }
                            let parent = parent_id.clone();
                            let edit = edit_id.clone();
                            let ret = *return_to;

                            if let Some(eid) = edit {
                                if trimmed.is_empty() {
                                    *error = Some("edit needs text".into());
                                    return Ok(());
                                }
                                match self.api.edit_post(&eid, trimmed).await {
                                    Ok(post) => {
                                        Self::apply_edited(&mut self.posts, &post);
                                        let _ =
                                            self.cache.upsert_posts(std::slice::from_ref(&post));
                                        if let Some(th) = self.thread.as_mut() {
                                            Self::apply_edited(&mut th.posts, &post);
                                        }
                                        self.status = "edited".into();
                                        self.return_from(ret);
                                    }
                                    Err(e) => *error = Some(format!("{e:#}")),
                                }
                                return Ok(());
                            }

                            match self
                                .api
                                .create_post(
                                    trimmed,
                                    parent.as_deref(),
                                    quote.as_ref().map(|q| q.id.as_str()),
                                    &img_ids,
                                )
                                .await
                            {
                                Ok(post) => {
                                    if post.parent_id.is_none() {
                                        if !self.posts.iter().any(|p| p.id == post.id) {
                                            self.posts.insert(0, post.clone());
                                        }
                                        let _ =
                                            self.cache.upsert_posts(std::slice::from_ref(&post));
                                        self.selected = 0;
                                    } else if let Some(ref pid) = post.parent_id {
                                        if let Some(p) =
                                            self.posts.iter_mut().find(|p| p.id == *pid)
                                        {
                                            p.reply_count += 1;
                                        }
                                    }
                                    self.status = if parent.is_some() {
                                        "replied".into()
                                    } else {
                                        "posted".into()
                                    };
                                    match ret {
                                        ReturnTo::Thread => {
                                            let root = self
                                                .thread
                                                .as_ref()
                                                .map(|t| t.root_id.clone())
                                                .or(parent.clone());
                                            if let Some(rid) = root {
                                                if let Err(e) = self.open_thread(&rid).await {
                                                    self.status =
                                                        format!("reply ok, thread refresh: {e:#}");
                                                    self.go_home();
                                                }
                                            } else {
                                                self.go_home();
                                            }
                                        }
                                        ReturnTo::Feed => {
                                            if let Some(pid) = parent {
                                                if let Err(e) = self.open_thread(&pid).await {
                                                    self.status =
                                                        format!("reply ok, open thread: {e:#}");
                                                    self.go_home();
                                                }
                                            } else {
                                                self.go_home();
                                            }
                                        }
                                    }
                                }
                                Err(e) => *error = Some(format!("{e:#}")),
                            }
                            return Ok(());
                        }
                        _ => {}
                    }
                }

                // Typing into an open color tag resets the highlight row.
                if matches!(key.code, KeyCode::Char(_))
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    *color_ac_idx = 0;
                }
                textarea.input(key);
                self.composer_mention_tick();
            }
            Screen::ConfirmDelete {
                post_id, return_to, ..
            } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let id = post_id.clone();
                    let ret = *return_to;
                    match self.api.delete_post(&id).await {
                        Ok(post) => {
                            Self::apply_deleted(&mut self.posts, &post);
                            let _ = self.cache.upsert_posts(std::slice::from_ref(&post));
                            self.status = "deleted".into();
                            match ret {
                                ReturnTo::Thread => {
                                    if let Some(rid) =
                                        self.thread.as_ref().map(|t| t.root_id.clone())
                                    {
                                        if let Err(e) = self.open_thread(&rid).await {
                                            self.status = format!("deleted, thread: {e:#}");
                                            self.go_home();
                                        }
                                    } else {
                                        self.go_home();
                                    }
                                }
                                ReturnTo::Feed => self.go_home(),
                            }
                        }
                        Err(e) => {
                            self.status = format!("delete failed: {e:#}");
                            self.return_from(ret);
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    let ret = *return_to;
                    self.return_from(ret);
                }
                _ => {}
            },
            Screen::Help { return_to } => {
                // q already handled as global quit; Esc / ? close help.
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    let ret = *return_to;
                    self.return_from(ret);
                }
            }
            Screen::Reminder { .. } => {
                // Must dismiss before other shortcuts work (q still quits globally).
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ')) {
                    let _ = self.reminders.acknowledge();
                    self.go_home();
                }
            }
            Screen::WelcomeMenu { .. } => {} // handled above
            Screen::Thread => {}             // handled above
            Screen::Search { .. }
            | Screen::Profile { .. }
            | Screen::Notifications { .. }
            | Screen::DmList { .. }
            | Screen::DmNew { .. }
            | Screen::DmThread { .. }
            | Screen::EditBio { .. }
            | Screen::BlockUser { .. } => {} // phase_c
            Screen::Boot => {}
            Screen::Palette { .. } => {} // handled above
        }

        Ok(())
    }

    fn handle_settings_key(&mut self, key: KeyEvent) -> Result<()> {
        let Screen::Settings {
            server_url,
            auto_login,
            focus,
            error,
            back,
        } = &mut self.screen
        else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                let back = back.clone();
                self.leave_settings(back);
            }
            // Tab / arrows move between fields (letters type into the URL when focused).
            KeyCode::Tab | KeyCode::Down => {
                *focus = (*focus + 1) % 2;
                *error = None;
            }
            KeyCode::Up | KeyCode::BackTab => {
                *focus = focus.checked_sub(1).unwrap_or(1);
                *error = None;
            }
            KeyCode::Char(' ') if *focus == 1 => {
                *auto_login = !*auto_login;
                *error = None;
            }
            // Ctrl+R → reset server URL to local default.
            KeyCode::Char('r') | KeyCode::Char('R')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                *server_url = "http://127.0.0.1:8080".into();
                *error = None;
            }
            KeyCode::Char(c) if *focus == 0 && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if server_url.len() < 256 {
                    server_url.push(c);
                }
                *error = None;
            }
            KeyCode::Backspace if *focus == 0 => {
                server_url.pop();
                *error = None;
            }
            KeyCode::Enter => {
                let url = server_url.clone();
                let auto = *auto_login;
                let back = back.clone();
                match self.apply_settings(&url, auto) {
                    Ok(()) => self.leave_settings(back),
                    Err(e) => {
                        if let Screen::Settings { error, .. } = &mut self.screen {
                            *error = Some(e);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Keep selected feed row roughly in view (approx 6 posts per screen).
    fn ensure_feed_visible(&mut self) {
        const WINDOW: usize = 5;
        if self.selected < self.feed_scroll {
            self.feed_scroll = self.selected;
        } else if self.selected >= self.feed_scroll + WINDOW {
            self.feed_scroll = self.selected.saturating_sub(WINDOW - 1);
        }
    }

    async fn handle_thread_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(th) = self.thread.as_ref() else {
            self.go_home();
            return Ok(());
        };
        let root_id = th.root_id.clone();
        let selected_post = th.posts.get(th.selected).cloned();

        match key.code {
            KeyCode::Esc => {
                self.thread = None;
                self.go_home();
                self.status = "timeline".into();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(th) = self.thread.as_mut() {
                    if !th.posts.is_empty() {
                        th.selected = (th.selected + 1).min(th.posts.len() - 1);
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(th) = self.thread.as_mut() {
                    th.selected = th.selected.saturating_sub(1);
                }
            }
            KeyCode::Char('g') => {
                if let Some(th) = self.thread.as_mut() {
                    th.selected = 0;
                }
            }
            KeyCode::Char('G') => {
                if let Some(th) = self.thread.as_mut() {
                    if !th.posts.is_empty() {
                        th.selected = th.posts.len() - 1;
                    }
                }
            }
            KeyCode::Char('r') => {
                if let Some(post) = selected_post {
                    self.open_compose(Some(post.id), ReturnTo::Thread);
                }
            }
            KeyCode::Char(' ') => {
                if let Some(post) = selected_post {
                    self.toggle_like_on(&post.id).await;
                }
            }
            KeyCode::Char('o') | KeyCode::Char('O')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Some(post) = selected_post {
                    self.open_post_images(&post).await;
                }
            }
            KeyCode::Char('d') => {
                if let Some(post) = selected_post {
                    if self.is_mine(&post) && post.deleted_at.is_none() {
                        let preview: String = post.display_content().chars().take(40).collect();
                        self.screen = Screen::ConfirmDelete {
                            post_id: post.id,
                            preview,
                            return_to: ReturnTo::Thread,
                        };
                    } else if !self.is_mine(&post) {
                        self.status = "you can only delete your own posts".into();
                    }
                }
            }
            KeyCode::Char('u') => {
                if let Err(e) = self.open_thread(&root_id).await {
                    self.status = format!("refresh failed: {e:#}");
                }
            }
            KeyCode::Char('?') => {
                self.screen = Screen::Help {
                    return_to: ReturnTo::Thread,
                };
            }
            KeyCode::Char('n') => {
                let parent = selected_post
                    .as_ref()
                    .map(|p| p.id.clone())
                    .unwrap_or(root_id);
                self.open_compose(Some(parent), ReturnTo::Thread);
            }
            KeyCode::Char('e') => {
                if let Some(post) = selected_post {
                    if self.is_mine(&post) && post.deleted_at.is_none() {
                        self.open_edit_post(&post.id, &post.content, ReturnTo::Thread);
                    } else {
                        self.status = "you can only edit your own posts".into();
                    }
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                // p → selected author; Shift+p → you
                let mine = key.modifiers.contains(KeyModifiers::SHIFT);
                let handle = if mine {
                    self.session.as_ref().map(|s| s.handle.clone())
                } else {
                    selected_post
                        .map(|p| p.author_handle)
                        .or_else(|| self.session.as_ref().map(|s| s.handle.clone()))
                };
                match handle {
                    Some(h) => {
                        if let Err(e) = self.open_profile(&h).await {
                            self.status = format!("profile: {e:#}");
                        }
                    }
                    None => self.status = "no profile".into(),
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn open_compose(&mut self, parent_id: Option<String>, return_to: ReturnTo) {
        let mut ta = TextArea::default();
        if parent_id.is_some() {
            ta.set_placeholder_text("post your reply");
        } else {
            ta.set_placeholder_text("what's happening?");
        }
        self.screen = Screen::Compose {
            textarea: ta,
            error: None,
            parent_id,
            quote: None,
            mention_ac: crate::client::interactions::MentionAc::default(),
            edit_id: None,
            return_to,
            color_ac_idx: 0,
            images: Vec::new(),
            attach_path: None,
        };
    }

    /// Download attached images and open them with the system viewer (`xdg-open`).
    pub(crate) async fn open_post_images(&mut self, post: &Post) {
        if !post.has_images() {
            self.status = "no images on this post".into();
            return;
        }
        let dir = match crate::client::config::data_dir() {
            Ok(d) => d.join("media-cache"),
            Err(e) => {
                self.status = format!("media cache: {e:#}");
                return;
            }
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status = format!("media cache: {e}");
            return;
        }

        let mut opened = 0usize;
        let mut last_err: Option<String> = None;
        for img in &post.images {
            let ext = match img.content_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                _ => "bin",
            };
            let path = dir.join(format!("{}.{}", img.id, ext));
            if !path.exists() {
                match self.api.download_media(&img.id).await {
                    Ok((_ct, bytes)) => {
                        if let Err(e) = std::fs::write(&path, &bytes) {
                            last_err = Some(format!("write: {e}"));
                            continue;
                        }
                    }
                    Err(e) => {
                        last_err = Some(format!("download: {e:#}"));
                        continue;
                    }
                }
            }
            match std::process::Command::new("xdg-open").arg(&path).spawn() {
                Ok(_) => opened += 1,
                Err(e) => last_err = Some(format!("xdg-open: {e}")),
            }
        }
        if opened > 0 {
            self.status = format!("opened {opened} image(s)");
        } else if let Some(e) = last_err {
            self.status = e;
        } else {
            self.status = "could not open images".into();
        }
    }

    /// Navigate to the home feed and play the sidebar logo wave (if not already home).
    pub fn go_home(&mut self) {
        let was_home = matches!(self.screen, Screen::Feed);
        self.screen = Screen::Feed;
        if !was_home {
            self.play_logo_wave();
        }
    }

    pub fn play_logo_wave(&mut self) {
        self.logo_wave_at = Some(Instant::now());
    }

    /// Seconds into the logo wave, if still playing.
    pub fn logo_wave_secs(&self) -> Option<f32> {
        // One slow pass across the wordmark.
        const WAVE_SECS: f32 = 2.8;
        let t = self.logo_wave_at?.elapsed().as_secs_f32();
        if t < WAVE_SECS {
            Some(t)
        } else {
            None
        }
    }

    fn return_from(&mut self, ret: ReturnTo) {
        match ret {
            ReturnTo::Feed => {
                self.thread = None;
                self.go_home();
            }
            ReturnTo::Thread => {
                if self.thread.is_some() {
                    self.screen = Screen::Thread;
                } else {
                    self.go_home();
                }
            }
        }
    }

    fn is_mine(&self, post: &Post) -> bool {
        self.session
            .as_ref()
            .map(|s| s.user_id == post.author_id)
            .unwrap_or(false)
    }

    pub(crate) async fn toggle_like_on(&mut self, post_id: &str) {
        match self.api.toggle_like(post_id).await {
            Ok(resp) => {
                Self::apply_like_full(
                    &mut self.posts,
                    &resp.post_id,
                    resp.like_count,
                    resp.liked_by_me,
                );
                if let Some(th) = self.thread.as_mut() {
                    Self::apply_like_full(
                        &mut th.posts,
                        &resp.post_id,
                        resp.like_count,
                        resp.liked_by_me,
                    );
                }
                if let Some(p) = self.posts.iter().find(|p| p.id == resp.post_id) {
                    let _ = self.cache.upsert_posts(std::slice::from_ref(p));
                }
                self.status = if resp.liked_by_me {
                    format!("liked · {}", resp.like_count)
                } else {
                    format!("unliked · {}", resp.like_count)
                };
            }
            Err(e) => self.status = format!("like failed: {e:#}"),
        }
    }

    fn apply_like_full(list: &mut [Post], post_id: &str, like_count: i64, liked_by_me: bool) {
        if let Some(p) = list.iter_mut().find(|p| p.id == post_id) {
            p.like_count = like_count;
            p.liked_by_me = liked_by_me;
        }
    }

    pub(crate) async fn open_thread(&mut self, post_id: &str) -> Result<()> {
        self.pending_thread = Some(post_id.to_string());

        // 1) Memory cache (any post in a previously opened thread).
        if let Some(cached) = self.view_cache.thread_for_post(post_id) {
            let selected = cached
                .posts
                .iter()
                .position(|p| p.id == post_id)
                .unwrap_or(0);
            self.thread = Some(ThreadState {
                root_id: cached.root_id.clone(),
                posts: cached.posts.clone(),
                selected,
            });
            self.screen = Screen::Thread;
            self.status = "thread".into();
            self.spawn_load_thread(post_id.to_string());
            return Ok(());
        }

        // 2) Optimistic single-post view from feed / disk / open profile.
        let mut seed = self.posts.iter().find(|p| p.id == post_id).cloned();
        if seed.is_none() {
            if let Screen::Profile { state } = &self.screen {
                seed = state.posts.iter().find(|p| p.id == post_id).cloned();
            }
        }
        if seed.is_none() {
            seed = self.cache.get(post_id).ok().flatten();
        }
        if let Some(p) = seed {
            let root_id = p.parent_id.clone().unwrap_or_else(|| p.id.clone());
            self.thread = Some(ThreadState {
                root_id,
                posts: vec![p],
                selected: 0,
            });
            self.screen = Screen::Thread;
            self.status = "thread · loading…".into();
        } else {
            self.thread = Some(ThreadState {
                root_id: post_id.to_string(),
                posts: Vec::new(),
                selected: 0,
            });
            self.screen = Screen::Thread;
            self.status = "loading thread…".into();
        }
        self.spawn_load_thread(post_id.to_string());
        Ok(())
    }

    async fn enter_feed(&mut self) -> Result<()> {
        self.thread = None;
        self.go_home();
        self.play_logo_wave(); // always greet on login / enter
                               // Disk/memory feed is already on screen — refresh in the background.
        if self.posts.is_empty() {
            self.status = "loading feed…".into();
        }
        self.spawn_refresh_feed();
        self.spawn_ws();
        if self.reminders.should_remind() {
            self.screen = Screen::Reminder {
                message: "Do you still have your conr token saved somewhere safe?".into(),
            };
        }
        Ok(())
    }

    /// Instant Tab switch: swap in the last snapshot for that tab, revalidate async.
    pub(crate) fn switch_feed_tab(&mut self) {
        self.view_cache.stash_feed_page(
            self.feed_tab,
            self.posts.clone(),
            self.feed_next_cursor.clone(),
            self.feed_has_more,
        );
        self.feed_tab = self.feed_tab.toggle();
        if let Some(cached) = self.view_cache.feed_page(self.feed_tab) {
            self.posts = cached.posts.clone();
            self.feed_next_cursor = cached.next_cursor.clone();
            self.feed_has_more = cached.has_more;
            self.selected = 0;
            self.feed_scroll = 0;
            self.status = format!(
                "tab: {} · {} posts",
                self.feed_tab.label(),
                self.posts.len()
            );
        } else {
            self.posts.clear();
            self.feed_next_cursor = None;
            self.feed_has_more = true;
            self.selected = 0;
            self.feed_scroll = 0;
            self.status = format!("tab: {} · loading…", self.feed_tab.label());
        }
        self.spawn_refresh_feed();
    }

    /// Blocking refresh kept for rare call sites that still await it.
    pub(crate) async fn refresh_feed(&mut self) -> Result<()> {
        self.spawn_refresh_feed();
        Ok(())
    }

    pub(crate) fn spawn_refresh_feed(&mut self) {
        self.feed_epoch = self.feed_epoch.saturating_add(1);
        let epoch = self.feed_epoch;
        self.spawn_feed_page(None, false, epoch);
    }

    fn maybe_load_more_feed(&mut self) {
        if !self.feed_has_more || self.feed_loading_more {
            return;
        }
        if self.posts.is_empty() {
            return;
        }
        let near_end = self.selected + 3 >= self.posts.len();
        if !near_end {
            return;
        }
        let cursor = self
            .feed_next_cursor
            .clone()
            .or_else(|| self.posts.last().map(|p| p.id.clone()));
        let Some(cursor) = cursor else { return };
        self.feed_loading_more = true;
        self.status = "loading older posts…".into();
        self.spawn_feed_page(Some(cursor), true, self.feed_epoch);
    }

    fn spawn_feed_page(&self, cursor: Option<String>, append: bool, epoch: u64) {
        let following = self.feed_tab == FeedTab::Following;
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            let feed = api.feed(cursor.as_deref(), following).await;
            match feed {
                Ok(resp) => {
                    let mut unread = None;
                    let mut unread_dms = None;
                    let mut following_handles = None;
                    let mut blocked_handles = None;
                    if !append {
                        if let Ok(n) = api.notifications().await {
                            unread = Some(n.unread);
                        }
                        if let Ok(d) = api.list_dms().await {
                            unread_dms = Some(d.unread);
                            let _ = tx.send(ClientEvent::DmListLoaded {
                                items: d.conversations,
                            });
                        }
                        if let Ok(h) = api.list_following().await {
                            following_handles = Some(h);
                        }
                        if let Ok(h) = api.list_blocks().await {
                            blocked_handles = Some(h);
                        }
                    }
                    let next_cursor = resp.next_cursor;
                    let _ = tx.send(ClientEvent::FeedLoaded {
                        epoch,
                        following,
                        posts: resp.posts,
                        append,
                        next_cursor,
                        unread,
                        unread_dms,
                        following_handles,
                        blocked_handles,
                    });
                }
                Err(e) => {
                    let _ = tx.send(ClientEvent::Status(format!("refresh failed: {e:#}")));
                    if append {
                        let _ = tx.send(ClientEvent::FeedMoreFailed { epoch });
                    }
                }
            }
        });
    }
    pub(crate) fn spawn_load_thread(&self, post_id: String) {
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match api.get_thread(&post_id).await {
                Ok(resp) => {
                    let ordered = order_thread_dfs(&resp.posts, &resp.root.id);
                    let _ = tx.send(ClientEvent::ThreadLoaded {
                        focus_id: post_id,
                        root_id: resp.root.id,
                        posts: ordered,
                    });
                }
                Err(e) => {
                    let _ = tx.send(ClientEvent::Status(format!("thread: {e:#}")));
                }
            }
        });
    }

    pub(crate) fn spawn_mention_search(&mut self, query: String, request: u64) {
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            if let Ok(resp) = api.search(&format!("@{query}")).await {
                let items: Vec<String> = resp
                    .users
                    .into_iter()
                    .filter(|u| u.handle.to_ascii_lowercase().starts_with(&query))
                    .take(crate::client::interactions::MENTION_MAX_ITEMS)
                    .map(|u| u.handle)
                    .collect();
                let _ = tx.send(ClientEvent::MentionSearched { request, items });
            }
        });
    }

    pub(crate) fn spawn_load_profile(&self, handle: String) {
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        let h = handle.clone();
        tokio::spawn(async move {
            let profile = api.get_profile(&h).await;
            let posts = api.profile_posts(&h, None).await;
            match (profile, posts) {
                (Ok(profile), Ok(feed)) => {
                    let _ = tx.send(ClientEvent::ProfileLoaded {
                        profile,
                        posts: feed.posts,
                        append: false,
                        next_cursor: feed.next_cursor,
                    });
                }
                (Err(e), _) | (_, Err(e)) => {
                    let _ = tx.send(ClientEvent::Status(format!("profile: {e:#}")));
                }
            }
        });
    }

    pub(crate) fn maybe_load_more_profile(&mut self) {
        let (handle, cursor, should) = if let Screen::Profile { state } = &self.screen {
            if !state.has_more || state.posts.is_empty() {
                return;
            }
            if state.selected + 3 < state.posts.len() {
                return;
            }
            let cursor = state
                .next_cursor
                .clone()
                .or_else(|| state.posts.last().map(|p| p.id.clone()));
            let ready = cursor.is_some();
            (state.profile.handle.clone(), cursor, ready)
        } else {
            return;
        };
        if !should {
            return;
        }
        let Some(cursor) = cursor else { return };
        self.pending_profile = Some(handle.to_ascii_lowercase());
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            let profile = api.get_profile(&handle).await;
            let posts = api.profile_posts(&handle, Some(&cursor)).await;
            match (profile, posts) {
                (Ok(profile), Ok(feed)) => {
                    let _ = tx.send(ClientEvent::ProfileLoaded {
                        profile,
                        posts: feed.posts,
                        append: true,
                        next_cursor: feed.next_cursor,
                    });
                }
                (Err(e), _) | (_, Err(e)) => {
                    let _ = tx.send(ClientEvent::Status(format!("profile: {e:#}")));
                }
            }
        });
    }

    pub(crate) fn spawn_load_notifications(&self) {
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match api.notifications().await {
                Ok(resp) => {
                    let _ = api.mark_notifications_read().await;
                    let _ = tx.send(ClientEvent::NotificationsLoaded {
                        items: resp.notifications,
                        unread: 0,
                    });
                }
                Err(e) => {
                    let _ = tx.send(ClientEvent::Status(format!("notifications: {e:#}")));
                }
            }
        });
    }

    pub(crate) fn spawn_load_dm_list(&self) {
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match api.list_dms().await {
                Ok(resp) => {
                    let _ = tx.send(ClientEvent::DmListLoaded {
                        items: resp.conversations,
                    });
                }
                Err(e) => {
                    let _ = tx.send(ClientEvent::Status(format!("dms: {e:#}")));
                }
            }
        });
    }

    pub(crate) fn spawn_load_dm_thread(&self, request_key: String, id: String) {
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match api.get_dm(&id).await {
                Ok(resp) => {
                    let _ = tx.send(ClientEvent::DmThreadLoaded {
                        request_key,
                        conversation_id: resp.conversation.id,
                        peer_handle: resp.conversation.peer_handle,
                        messages: resp.messages,
                    });
                }
                Err(e) => {
                    let _ = tx.send(ClientEvent::Status(format!("dm: {e:#}")));
                }
            }
        });
    }

    /// Placeholder profile used while network data is in flight.
    pub(crate) fn stub_profile(handle: &str, is_me: bool) -> UserProfile {
        UserProfile {
            id: String::new(),
            handle: handle.to_string(),
            bio: None,
            created_at: Utc::now(),
            followers: 0,
            following: 0,
            posts: 0,
            followed_by_me: false,
            follows_me: false,
            blocked_by_me: false,
            is_me,
        }
    }

    fn spawn_ws(&self) {
        let Ok(url) = self.api.ws_url() else {
            return;
        };
        // Invalidate any previous loop, then start a new generation.
        let gen = self.ws_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let gen_flag = self.ws_generation.clone();
        let quit_flag = self.quit_flag.clone();
        let api = self.api.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            loop {
                if quit_flag.load(Ordering::SeqCst) || gen_flag.load(Ordering::SeqCst) != gen {
                    break;
                }
                // Prefer Authorization header (no token in URL / proxy logs).
                let mut token = api.access_token();
                if token.is_none() {
                    let _ = api.refresh_session().await;
                    token = api.access_token();
                }
                let Some(access) = token else {
                    // Don't spam status after logout — just wait or exit if superseded.
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                };

                let connect = async {
                    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
                    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
                    let mut req = url
                        .as_str()
                        .into_client_request()
                        .map_err(|e| anyhow::anyhow!("ws request: {e}"))?;
                    req.headers_mut().insert(
                        AUTHORIZATION,
                        format!("Bearer {access}")
                            .parse()
                            .map_err(|e| anyhow::anyhow!("ws auth header: {e}"))?,
                    );
                    tokio_tungstenite::connect_async(req)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))
                };

                match connect.await {
                    Ok((ws, _)) => {
                        if gen_flag.load(Ordering::SeqCst) != gen {
                            break;
                        }
                        let _ = tx.send(ClientEvent::WsConnected);
                        let (_write, mut read) = ws.split();
                        while let Some(msg) = read.next().await {
                            if quit_flag.load(Ordering::SeqCst)
                                || gen_flag.load(Ordering::SeqCst) != gen
                            {
                                break;
                            }
                            match msg {
                                Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                                    if let Ok(ev) = serde_json::from_str::<Event>(&text) {
                                        match ev {
                                            Event::NewPost { post } => {
                                                let _ = tx.send(ClientEvent::NewPost(post));
                                            }
                                            Event::PostDeleted { post } => {
                                                let _ = tx.send(ClientEvent::PostDeleted(post));
                                            }
                                            Event::LikeUpdated {
                                                post_id,
                                                like_count,
                                            } => {
                                                let _ = tx.send(ClientEvent::LikeUpdated {
                                                    post_id,
                                                    like_count,
                                                });
                                            }
                                            Event::PostEdited { post } => {
                                                let _ = tx.send(ClientEvent::PostEdited(post));
                                            }
                                            Event::Notification { notification } => {
                                                let _ = tx
                                                    .send(ClientEvent::Notification(notification));
                                            }
                                            Event::DmMessage { message } => {
                                                let _ = tx.send(ClientEvent::DmMessage(message));
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => break,
                                Err(_) => break,
                                _ => {}
                            }
                        }
                        let _ = tx.send(ClientEvent::WsDisconnected);
                    }
                    Err(e) => {
                        if gen_flag.load(Ordering::SeqCst) == gen
                            && !quit_flag.load(Ordering::SeqCst)
                        {
                            let _ = api.refresh_session().await;
                            let _ = tx.send(ClientEvent::Status(format!("ws error: {e}")));
                        }
                    }
                }
                if quit_flag.load(Ordering::SeqCst) || gen_flag.load(Ordering::SeqCst) != gen {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }
}

fn posts_contains(posts: &[Post], id: &str) -> bool {
    posts.iter().any(|p| p.id == id)
}

/// DFS pre-order so replies nest under their parent in the TUI.
pub fn order_thread_dfs(posts: &[Post], root_id: &str) -> Vec<Post> {
    use std::collections::HashMap;
    let mut children: HashMap<&str, Vec<&Post>> = HashMap::new();
    for p in posts {
        if let Some(ref parent) = p.parent_id {
            children.entry(parent.as_str()).or_default().push(p);
        }
    }
    for v in children.values_mut() {
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    }

    let mut out = Vec::with_capacity(posts.len());
    fn walk<'a>(
        id: &str,
        posts: &'a [Post],
        children: &std::collections::HashMap<&str, Vec<&'a Post>>,
        out: &mut Vec<Post>,
    ) {
        if let Some(p) = posts.iter().find(|p| p.id == id) {
            out.push(p.clone());
            if let Some(kids) = children.get(id) {
                for kid in kids {
                    walk(&kid.id, posts, children, out);
                }
            }
        }
    }
    walk(root_id, posts, &children, &mut out);
    // Any orphans (shouldn't happen) append chronologically.
    for p in posts {
        if !out.iter().any(|o| o.id == p.id) {
            out.push(p.clone());
        }
    }
    out
}

/// Depth of a post within a thread list (0 = root).
pub fn post_depth(posts: &[Post], post: &Post) -> usize {
    let mut depth = 0;
    let mut current_parent = post.parent_id.clone();
    while let Some(pid) = current_parent {
        depth += 1;
        current_parent = posts
            .iter()
            .find(|p| p.id == pid)
            .and_then(|p| p.parent_id.clone());
        if depth > 64 {
            break; // safety
        }
    }
    depth
}

// silence unused
#[allow(dead_code)]
fn _fmt(t: &str) -> String {
    format_token_block(t)
}
