use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use conr_shared::{format_token_block, strip_token, validate_token, Event, Post, POST_MAX_LEN};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use crate::api::Api;
use crate::backup;
use crate::cache::Cache;
use crate::config::Config;
use crate::keychain;
use crate::reminders::ReminderState;
use crate::ui;

#[derive(Debug, Clone)]
pub struct Session {
    pub handle: String,
    pub user_id: String,
}

#[derive(Debug)]
pub enum Screen {
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
    Feed,
    Compose {
        textarea: TextArea<'static>,
        error: Option<String>,
    },
    Reminder {
        message: String,
    },
    Help,
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

pub struct App {
    pub config: Config,
    pub api: Api,
    pub screen: Screen,
    pub session: Option<Session>,
    pub posts: Vec<Post>,
    pub selected: usize,
    pub status: String,
    pub ws_connected: bool,
    pub pending_token: Option<String>,
    /// Saved handles for the welcome picker (last-active first).
    pub saved_accounts: Vec<String>,
    cache: Cache,
    pub reminders: ReminderState,
    anim_started: Option<Instant>,
    event_rx: Option<mpsc::UnboundedReceiver<ClientEvent>>,
    event_tx: mpsc::UnboundedSender<ClientEvent>,
    should_quit: bool,
}

enum ClientEvent {
    WsConnected,
    WsDisconnected,
    NewPost(Post),
    Status(String),
}

impl App {
    pub async fn new(config: Config) -> Result<Self> {
        let api = Api::new(&config.server_url)?;
        let cache = Cache::open()?;
        let reminders = ReminderState::load().unwrap_or_default();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let saved_accounts = Self::load_saved_accounts(reminders.handle.as_deref());
        let mut app = Self {
            config,
            api,
            screen: Screen::WelcomeMenu { selected: 0 },
            session: None,
            posts: cache.recent(50).unwrap_or_default(),
            selected: 0,
            status: String::new(),
            ws_connected: false,
            pending_token: None,
            saved_accounts,
            cache,
            reminders,
            anim_started: None,
            event_rx: Some(event_rx),
            event_tx,
            should_quit: false,
        };
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

            terminal.draw(|f| ui::draw(f, self))?;

            // drain async events
            while let Ok(ev) = event_rx.try_recv() {
                self.handle_client_event(ev);
            }

            if self.should_quit {
                break;
            }

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    CEvent::Key(key) => self.handle_key(key).await?,
                    CEvent::Paste(text) => self.handle_paste(&text).await?,
                    _ => {}
                }
            }
        }
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
                if !self.posts.iter().any(|p| p.id == post.id) {
                    self.posts.insert(0, post.clone());
                    let _ = self.cache.upsert_posts(std::slice::from_ref(&post));
                }
            }
            ClientEvent::Status(s) => self.status = s,
        }
    }

    async fn handle_paste(&mut self, text: &str) -> Result<()> {
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
            Screen::Compose { textarea, .. } => {
                textarea.insert_str(text);
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // global quit on ctrl+c
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
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
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1) % n;
                    self.screen = Screen::WelcomeMenu { selected };
                }
                KeyCode::Char('k') | KeyCode::Up => {
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
                        Some(MenuEntry::Quit) | None => self.should_quit = true,
                    }
                }
                KeyCode::Char('q') => self.should_quit = true,
                _ => {
                    self.screen = Screen::WelcomeMenu { selected };
                }
            }
            return Ok(());
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
                        let export_status = crate::clipboard::export_token(&token)
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
                KeyCode::Char('c')
                    if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    match crate::clipboard::export_token(token) {
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
                    let export_status = crate::clipboard::export_token(&token)
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
                            conr_shared::TOKEN_LEN,
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
                            &self.session.as_ref().map(|s| s.handle.clone()).unwrap_or_default(),
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
            Screen::Feed => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                // Logout: stop silent auto-login, KEEP all identity tokens, show picker.
                KeyCode::Char('L') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    let who = self.session.as_ref().map(|s| s.handle.clone());
                    self.session = None;
                    let _ = self.reminders.clear_session();
                    let _ = self.cache.clear();
                    self.api.access_token = None;
                    self.api.refresh_token = None;
                    self.posts.clear();
                    self.ws_connected = false;
                    self.refresh_saved_accounts();
                    // Preselect the account we just left (still at top if present).
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
                KeyCode::Char('j') | KeyCode::Down => {
                    if !self.posts.is_empty() {
                        self.selected = (self.selected + 1).min(self.posts.len() - 1);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.selected = self.selected.saturating_sub(1);
                }
                KeyCode::Char('g') => self.selected = 0,
                KeyCode::Char('G') => {
                    if !self.posts.is_empty() {
                        self.selected = self.posts.len() - 1;
                    }
                }
                KeyCode::Char('n') => {
                    let mut ta = TextArea::default();
                    ta.set_placeholder_text("what's happening?");
                    self.screen = Screen::Compose {
                        textarea: ta,
                        error: None,
                    };
                }
                KeyCode::Char('r') => {
                    if let Err(e) = self.refresh_feed().await {
                        self.status = format!("refresh failed: {e:#}");
                    }
                }
                KeyCode::Char('?') => self.screen = Screen::Help,
                _ => {}
            },
            Screen::Compose { textarea, error } => {
                if key.code == KeyCode::Esc {
                    self.screen = Screen::Feed;
                    return Ok(());
                }
                if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    let content = textarea.lines().join("\n");
                    let trimmed = content.trim();
                    if trimmed.is_empty() {
                        *error = Some("empty post rejected".into());
                        return Ok(());
                    }
                    if trimmed.chars().count() > POST_MAX_LEN {
                        *error = Some(format!("max {POST_MAX_LEN} characters"));
                        return Ok(());
                    }
                    match self.api.create_post(trimmed).await {
                        Ok(post) => {
                            if !self.posts.iter().any(|p| p.id == post.id) {
                                self.posts.insert(0, post.clone());
                            }
                            let _ = self.cache.upsert_posts(std::slice::from_ref(&post));
                            self.selected = 0;
                            self.screen = Screen::Feed;
                            self.status = "posted".into();
                        }
                        Err(e) => *error = Some(format!("{e:#}")),
                    }
                    return Ok(());
                }
                // forward to textarea
                textarea.input(key);
            }
            Screen::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')) {
                    self.screen = Screen::Feed;
                }
            }
            Screen::Reminder { .. } => {
                if key.code == KeyCode::Enter {
                    let _ = self.reminders.acknowledge();
                    self.screen = Screen::Feed;
                }
            }
            Screen::WelcomeMenu { .. } => {} // handled above
            Screen::Boot => {}
        }
        Ok(())
    }

    async fn enter_feed(&mut self) -> Result<()> {
        self.screen = Screen::Feed;
        if let Err(e) = self.refresh_feed().await {
            self.status = format!("using cache ({e:#})");
        }
        self.spawn_ws();
        if self.reminders.should_remind() {
            self.screen = Screen::Reminder {
                message: "Do you still have your conr token saved somewhere safe?".into(),
            };
        }
        Ok(())
    }

    async fn refresh_feed(&mut self) -> Result<()> {
        let resp = self.api.feed(None).await?;
        self.posts = resp.posts;
        let _ = self.cache.upsert_posts(&self.posts);
        self.selected = 0;
        self.status = format!("{} posts", self.posts.len());
        Ok(())
    }

    fn spawn_ws(&self) {
        let Ok(url) = self.api.ws_url() else {
            return;
        };
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            loop {
                match tokio_tungstenite::connect_async(&url).await {
                    Ok((ws, _)) => {
                        let _ = tx.send(ClientEvent::WsConnected);
                        let (_write, mut read) = ws.split();
                        while let Some(msg) = read.next().await {
                            match msg {
                                Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                                    if let Ok(ev) = serde_json::from_str::<Event>(&text) {
                                        if let Event::NewPost { post } = ev {
                                            let _ = tx.send(ClientEvent::NewPost(post));
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
                        let _ = tx.send(ClientEvent::Status(format!("ws error: {e}")));
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }
}

// silence unused
#[allow(dead_code)]
fn _fmt(t: &str) -> String {
    format_token_block(t)
}
