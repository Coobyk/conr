use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use chrono::{DateTime, Utc};
use conr_shared::{format_token_block, POST_MAX_LEN};

use crate::app::{post_depth, App, MenuEntry, ReturnTo, Screen};
use crate::phase_c::{FeedTab, SearchHit};

// ── Palette (ink dark + violet) ─────────────────────────────────────────────

const BG: Color = Color::Rgb(0x0a, 0x0a, 0x0f);
const SURFACE: Color = Color::Rgb(0x14, 0x14, 0x1c);
const SURFACE2: Color = Color::Rgb(0x1c, 0x1c, 0x28);
const BORDER: Color = Color::Rgb(0x2a, 0x2a, 0x3a);
const TEXT: Color = Color::Rgb(0xf0, 0xee, 0xf6);
const MUTED: Color = Color::Rgb(0x8a, 0x86, 0x9a);
const PURPLE: Color = Color::Rgb(0xa7, 0x8b, 0xfa);
const PURPLE_DIM: Color = Color::Rgb(0x5b, 0x45, 0x8f);
const PURPLE_HOT: Color = Color::Rgb(0xc4, 0xb5, 0xfd);
const LIKE: Color = Color::Rgb(0xf4, 0x72, 0xb6);
const RT: Color = Color::Rgb(0x34, 0xd3, 0x99);
const ERR: Color = Color::Rgb(0xf8, 0x71, 0x71);
const OK: Color = Color::Rgb(0x34, 0xd3, 0x99);
const SELECT_BG: Color = Color::Rgb(0x1e, 0x1b, 0x2e);
const AVATAR_BG: Color = Color::Rgb(0xa7, 0x8b, 0xfa);
const BUBBLE_ME: Color = Color::Rgb(0x2e, 0x22, 0x4a);
const BUBBLE_THEM: Color = Color::Rgb(0x1a, 0x1a, 0x24);

fn style_bg() -> Style {
    Style::default().bg(BG).fg(TEXT)
}

fn style_accent() -> Style {
    Style::default().fg(PURPLE).add_modifier(Modifier::BOLD)
}

fn style_text() -> Style {
    Style::default().fg(TEXT)
}

fn style_dim() -> Style {
    Style::default().fg(MUTED)
}

fn style_err() -> Style {
    Style::default().fg(ERR)
}

fn style_ok() -> Style {
    Style::default().fg(OK)
}

fn style_bold() -> Style {
    Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
}

fn fill_bg(frame: &mut Frame, area: Rect) {
    frame.render_widget(Block::default().style(style_bg()), area);
}

fn card_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(format!(" {title} "), style_accent()))
        .style(Style::default().bg(SURFACE).fg(TEXT))
        .padding(Padding::new(1, 1, 0, 0))
}

// ── Motion / color helpers ──────────────────────────────────────────────────

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            Color::Rgb(lerp_u8(ar, br, t), lerp_u8(ag, bg, t), lerp_u8(ab, bb, t))
        }
        _ => a,
    }
}

/// Thin, dim hairline under headers (not a thick bright bar).
fn gradient_hairline(width: usize, phase: f32) -> Line<'static> {
    let w = width.max(1);
    let mut spans = Vec::with_capacity(w);
    for i in 0..w {
        let base = i as f32 / w as f32;
        let t = (base * 1.2 + phase * 0.04).rem_euclid(1.0);
        let wave = (t * std::f32::consts::TAU).sin().abs();
        // Stay near border / dim purple — never hot.
        let c = lerp_color(BORDER, PURPLE_DIM, 0.25 + 0.35 * wave);
        let ch = if i % 2 == 0 { "─" } else { " " };
        spans.push(Span::styled(ch, Style::default().fg(c).bg(BG)));
    }
    Line::from(spans)
}

fn status_gradient_line(text: &str, phase: f32) -> Line<'static> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Line::from("");
    }
    let n = chars.len().max(1) as f32;
    let mut spans = Vec::with_capacity(chars.len());
    for (i, ch) in chars.into_iter().enumerate() {
        let t = (i as f32 / n + phase * 0.05).rem_euclid(1.0);
        let c = lerp_color(MUTED, PURPLE, 0.08 + 0.18 * (t * std::f32::consts::TAU).sin().abs());
        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(c).bg(SURFACE2),
        ));
    }
    Line::from(spans)
}

/// density: 0.0 sparse … 1.0 party (welcome / login).
fn draw_sparkle_field(frame: &mut Frame, area: Rect, phase: f32, density: f32) {
    draw_sparkle_field_on(frame, area, phase, density, BG);
}

fn draw_sparkle_field_on(frame: &mut Frame, area: Rect, phase: f32, density: f32, bg: Color) {
    if area.width < 6 || area.height < 3 {
        return;
    }
    let density = density.clamp(0.05, 1.0);
    let step = if density > 0.7 {
        1usize
    } else if density > 0.4 {
        2
    } else {
        3
    };
    let threshold = (18.0 + density * 55.0) as u32;

    for row in 0..area.height {
        for col in (0..area.width).step_by(step) {
            let n = ((col as u32).wrapping_mul(374761393)
                ^ (row as u32).wrapping_mul(668265263)
                ^ ((phase * 2.5) as u32).wrapping_mul(1274126177)
                ^ ((phase as u32).wrapping_mul(97)))
                % 1000;
            if n > threshold {
                continue;
            }
            let twinkle =
                ((phase * 2.6 + col as f32 * 0.19 + row as f32 * 0.13).sin() + 1.0) * 0.5;
            if twinkle < 0.28 {
                continue;
            }
            let ch = match n % 5 {
                0 => "✦",
                1 => "·",
                2 => "✧",
                3 => "·",
                _ => "⋆",
            };
            let c = lerp_color(PURPLE_DIM, PURPLE, 0.35 + 0.45 * twinkle);
            let cell = Rect {
                x: area.x + col,
                y: area.y + row,
                width: 1,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(Span::styled(ch, Style::default().fg(c).bg(bg))),
                cell,
            );
        }
    }
}

/// Instant popup shell (no intro animation). Returns content inner rect.
fn begin_popup(frame: &mut Frame, base: Rect, title: &str) -> Rect {
    frame.render_widget(Clear, base);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PURPLE))
        .title(Span::styled(format!(" {title} "), style_accent()))
        .style(Style::default().bg(SURFACE).fg(TEXT))
        .padding(Padding::horizontal(1));
    let inner = block.inner(base);
    frame.render_widget(block, base);
    inner
}

// ── Entry ───────────────────────────────────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    fill_bg(frame, area);

    // Sparkles: login / welcome background only (sidebar sparkles drawn in draw_feed).
    if matches!(
        app.screen,
        Screen::WelcomeMenu { .. } | Screen::Login { .. }
    ) {
        draw_sparkle_field(frame, area, app.ui_secs(), 0.95);
    }

    match &app.screen {
        Screen::Boot => draw_centered(frame, area, "conr", "starting…"),
        Screen::WelcomeMenu { selected } => draw_welcome_menu(frame, area, app, *selected),
        Screen::RegisterHandle { input, error } => {
            draw_input_screen(frame, area, "Create your account", input, error.as_deref())
        }
        Screen::SaveAck => draw_save_ack(frame, area),
        Screen::ShowToken {
            token,
            export_status,
            flash,
        } => draw_token_block(frame, area, token, export_status, *flash),
        Screen::WelcomeAnim { frame_idx } => draw_welcome_anim(frame, area, *frame_idx),
        Screen::UnlockConfirm {
            expected: _,
            input,
            error,
            can_reveal,
        } => draw_unlock(frame, area, input, error.as_deref(), *can_reveal),
        Screen::BackupPrompt { selected } => draw_backup_prompt(frame, area, *selected),
        Screen::BackupPassphrase {
            input,
            confirm,
            stage,
            error,
        } => draw_backup_pass(frame, area, input, confirm, *stage, error.as_deref()),
        Screen::Login {
            handle,
            token_input,
            focus,
            error,
        } => draw_login(frame, area, handle, token_input, *focus, error.as_deref()),
        Screen::Feed => draw_feed(frame, area, app),
        Screen::Thread => draw_thread(frame, area, app),
        Screen::Compose {
            textarea,
            error,
            parent_id,
            edit_id,
            return_to,
        } => {
            match return_to {
                ReturnTo::Thread => draw_thread(frame, area, app),
                ReturnTo::Feed => draw_feed(frame, area, app),
            }
            let mode = if edit_id.is_some() {
                ComposeMode::Edit
            } else if parent_id.is_some() {
                ComposeMode::Reply
            } else {
                ComposeMode::New
            };
            draw_composer(frame, area, textarea, error.as_deref(), mode);
        }
        Screen::Search { state } => draw_search(frame, area, app, state),
        Screen::Profile { state } => draw_profile(frame, area, app, state),
        Screen::Notifications { state } => draw_notifications(frame, area, app, state),
        Screen::DmList { state } => draw_dm_list(frame, area, app, state),
        Screen::DmNew { state } => draw_dm_new(frame, area, app, state),
        Screen::DmThread { state } => draw_dm_thread(frame, area, app, state),
        Screen::EditBio { input, error } => draw_edit_bio(frame, area, input, error.as_deref()),
        Screen::BlockUser {
            handle,
            reason,
            error,
        } => draw_block_user(frame, area, handle, reason, error.as_deref()),
        Screen::ConfirmDelete {
            preview, return_to, ..
        } => {
            match return_to {
                ReturnTo::Thread => draw_thread(frame, area, app),
                ReturnTo::Feed => draw_feed(frame, area, app),
            }
            draw_confirm_delete(frame, area, preview);
        }
        Screen::Reminder { message } => draw_reminder(frame, area, message),
        Screen::Help { return_to } => {
            match return_to {
                ReturnTo::Thread => draw_thread(frame, area, app),
                ReturnTo::Feed => draw_feed(frame, area, app),
            }
            draw_help(frame, area);
        }
    }
}

// ── Auth / onboarding screens ───────────────────────────────────────────────

fn draw_centered(frame: &mut Frame, area: Rect, title: &str, subtitle: &str) {
    let popup = centered_rect(50, 30, area);
    let block = card_block("conr");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(title, style_accent())),
            Line::from(""),
            Line::from(Span::styled(subtitle, style_dim())),
        ])
        .alignment(Alignment::Center),
        inner,
    );
}

fn draw_welcome_menu(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    let popup = begin_popup(frame, centered_rect(56, 70, area), "Welcome");
    let entries = app.menu_entries();
    let n_saved = app.saved_accounts.len();
    let last = app.reminders.handle.as_deref();

    let mut all = vec![
        Line::from(Span::styled("  ◆  conr", style_accent())),
        Line::from(Span::styled(
            "  Happening now — chronological, no algorithm.",
            style_dim(),
        )),
        Line::from(""),
    ];

    if n_saved > 0 {
        all.push(Line::from(Span::styled("  Accounts", style_dim())));
        all.push(Line::from(""));
    }

    for (i, entry) in entries.iter().enumerate() {
        if n_saved > 0 && i == n_saved {
            all.push(Line::from(Span::styled(
                "  ────────────────────────",
                style_dim(),
            )));
        }

        let label = match entry {
            MenuEntry::Account(h) => {
                if last == Some(h.as_str()) {
                    format!("@{h}  · last active")
                } else {
                    format!("@{h}")
                }
            }
            MenuEntry::Register => "Create account".into(),
            MenuEntry::ManualLogin => "Log in".into(),
            MenuEntry::Quit => "Quit".into(),
        };

        if i == selected {
            all.push(Line::from(vec![
                Span::styled("  › ", style_accent()),
                Span::styled(
                    label,
                    Style::default()
                        .fg(TEXT)
                        .bg(SELECT_BG)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            all.push(Line::from(Span::styled(
                format!("    {label}"),
                style_text(),
            )));
        }
    }

    if !app.status.is_empty() {
        all.push(Line::from(""));
        let st = if app.status.contains("fail") || app.status.contains("could not") {
            style_err()
        } else {
            style_dim()
        };
        all.push(Line::from(Span::styled(format!("  {}", app.status), st)));
    }
    all.push(Line::from(""));
    all.push(Line::from(Span::styled(
        "  j/k  ·  Enter to continue",
        style_dim(),
    )));

    frame.render_widget(
        Paragraph::new(all)
            .alignment(Alignment::Left)
            .style(Style::default().bg(SURFACE).fg(TEXT)),
        popup,
    );
}

fn draw_input_screen(frame: &mut Frame, area: Rect, title: &str, input: &str, error: Option<&str>) {
    let popup = centered_rect(56, 40, area);
    let mut lines = vec![
        Line::from(Span::styled(format!("  {title}"), style_bold())),
        Line::from(Span::styled(
            "  Choose a handle — this is your @name.",
            style_dim(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  @", style_accent()),
            Span::styled(format!("{input}█"), style_text()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  letters, digits, _  ·  3–32 characters",
            style_dim(),
        )),
        Line::from(Span::styled("  Enter submit  ·  Esc back", style_dim())),
    ];
    if let Some(e) = error {
        lines.insert(5, Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    frame.render_widget(Paragraph::new(lines).block(card_block("Sign up")), popup);
}

fn draw_save_ack(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(64, 50, area);
    let lines = vec![
        Line::from(Span::styled("  Before we show your token", style_bold())),
        Line::from(""),
        Line::from(Span::styled(
            "  You will get a 128-character secret. Anyone with it",
            style_text(),
        )),
        Line::from(Span::styled(
            "  can use your account. There is NO server recovery.",
            style_text(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Save it in a password manager or offline.",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter — show token    Esc — cancel",
            style_ok(),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(card_block("Security")),
        popup,
    );
}

fn draw_token_block(frame: &mut Frame, area: Rect, token: &str, export_status: &str, flash: u8) {
    let popup = centered_rect(72, 80, area);
    let block_text = format_token_block(token);
    let path = crate::clipboard::token_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~/.local/share/conr/token-ONCE.txt".into());

    let mut lines = vec![
        Line::from(Span::styled(
            "  Save this token — you need it to unlock",
            style_bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Reliable: open this file and copy from there:",
            style_ok(),
        )),
        Line::from(Span::styled(format!("  {path}"), style_accent())),
        Line::from(Span::styled(
            "  (mode 0600 · delete after you save it)",
            style_dim(),
        )),
        Line::from(""),
    ];
    for line in block_text.lines() {
        lines.push(Line::from(Span::styled(
            format!("    {line}"),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    if !export_status.is_empty() {
        let msg = if flash > 0 {
            format!("  exported · {export_status}")
        } else {
            format!("  last export · {export_status}")
        };
        lines.push(Line::from(Span::styled(msg, style_ok())));
    }
    lines.push(Line::from(Span::styled(
        "  c re-copy  ·  Enter continue",
        style_dim(),
    )));
    frame.render_widget(
        Paragraph::new(lines).block(card_block("Identity token")),
        popup,
    );
}

fn draw_welcome_anim(frame: &mut Frame, area: Rect, frame_idx: u8) {
    let dots = ".".repeat((frame_idx as usize % 4) + 1);
    let art = match frame_idx % 6 {
        0 => "◆",
        1 => "◆  conr",
        2 => "◆  welcome",
        3 => "◆  you're in",
        4 => "◆  conr",
        _ => "◆",
    };
    let popup = centered_rect(40, 28, area);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(art, style_accent())),
        Line::from(""),
        Line::from(Span::styled(format!("joining{dots}"), style_dim())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(card_block("")),
        popup,
    );
}

fn draw_unlock(frame: &mut Frame, area: Rect, input: &str, error: Option<&str>, can_reveal: bool) {
    let popup = centered_rect(64, 45, area);
    let shown = if input.len() > 32 {
        format!("{}… ({} chars)", &input[..32], input.len())
    } else {
        format!("{input}█")
    };
    let mut lines = vec![
        Line::from(Span::styled("  Confirm your token", style_bold())),
        Line::from(Span::styled(
            "  Paste it to prove you saved it.",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled(format!("  {shown}"), style_text())),
        Line::from(""),
        Line::from(Span::styled(
            "  128 alphanumerics · whitespace stripped",
            style_dim(),
        )),
    ];
    if let Some(e) = error {
        lines.push(Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    if can_reveal {
        lines.push(Line::from(Span::styled(
            "  F1 — view token again",
            style_dim(),
        )));
    }
    lines.push(Line::from(Span::styled("  Enter unlock", style_dim())));
    frame.render_widget(Paragraph::new(lines).block(card_block("Unlock")), popup);
}

fn draw_backup_prompt(frame: &mut Frame, area: Rect, selected: usize) {
    let popup = centered_rect(64, 45, area);
    let items = [
        "Yes — encrypt a local backup with a passphrase",
        "No — keychain / password manager only",
    ];
    let mut lines = vec![
        Line::from(Span::styled("  Optional local backup", style_bold())),
        Line::from(Span::styled(
            "  ~/.local/share/conr/backup/ — never sent to the server.",
            style_dim(),
        )),
        Line::from(""),
    ];
    for (i, t) in items.iter().enumerate() {
        if i == selected {
            lines.push(Line::from(vec![
                Span::styled("  › ", style_accent()),
                Span::styled(*t, style_bold()),
            ]));
        } else {
            lines.push(Line::from(Span::styled(format!("    {t}"), style_text())));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  j/k  ·  Enter", style_dim())));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(card_block("Backup")),
        popup,
    );
}

fn draw_backup_pass(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    confirm: &str,
    stage: u8,
    error: Option<&str>,
) {
    let popup = centered_rect(56, 40, area);
    let label = if stage == 0 {
        "passphrase"
    } else {
        "confirm passphrase"
    };
    let val = if stage == 0 { input } else { confirm };
    let masked: String = "•".repeat(val.chars().count());
    let mut lines = vec![
        Line::from(Span::styled("  Backup passphrase", style_bold())),
        Line::from(Span::styled(
            "  Never leaves your machine. We cannot reset it.",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled(format!("  {label}: {masked}█"), style_text())),
    ];
    if let Some(e) = error {
        lines.push(Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    lines.push(Line::from(Span::styled(
        "  Enter continue  ·  Esc skip",
        style_dim(),
    )));
    frame.render_widget(Paragraph::new(lines).block(card_block("Passphrase")), popup);
}

fn draw_login(
    frame: &mut Frame,
    area: Rect,
    handle: &str,
    token_input: &str,
    focus: usize,
    error: Option<&str>,
) {
    let popup = centered_rect(56, 45, area);
    let h_style = if focus == 0 {
        style_bold()
    } else {
        style_text()
    };
    let t_style = if focus == 1 {
        style_bold()
    } else {
        style_text()
    };
    let h = if focus == 0 {
        format!("  handle   @{handle}█")
    } else {
        format!("  handle   @{handle}")
    };
    let tok_disp = if token_input.len() > 24 {
        format!("{}… ({} chars)", &token_input[..24], token_input.len())
    } else {
        token_input.to_string()
    };
    let t = if focus == 1 {
        format!("  token    {tok_disp}█")
    } else {
        format!("  token    {tok_disp}")
    };
    let mut lines = vec![
        Line::from(Span::styled("  Log in to conr", style_bold())),
        Line::from(""),
        Line::from(Span::styled(h, h_style)),
        Line::from(Span::styled(t, t_style)),
        Line::from(""),
        Line::from(Span::styled(
            "  Tab switch field  ·  Enter submit  ·  Esc back",
            style_dim(),
        )),
    ];
    if let Some(e) = error {
        lines.insert(5, Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    frame.render_widget(Paragraph::new(lines).block(card_block("Log in")), popup);
}

// ── Main chrome ─────────────────────────────────────────────────────────────

fn shell_columns(area: Rect) -> (Rect, Rect, Option<Rect>) {
    let wide = area.width >= 110;
    if wide {
        let mid = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(22),
                Constraint::Min(42),
                Constraint::Length(30),
            ])
            .split(area);
        (mid[0], mid[1], Some(mid[2]))
    } else {
        let mid = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(30)])
            .split(area);
        (mid[0], mid[1], None)
    }
}

fn draw_feed(frame: &mut Frame, area: Rect, app: &App) {
    let (side, main, rail) = shell_columns(area);
    draw_sidebar(frame, side, app, "home");
    draw_timeline(frame, main, app);
    if let Some(r) = rail {
        draw_right_rail(frame, r, app);
    }
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App, active: &str) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(SURFACE).fg(TEXT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let handle = app
        .session
        .as_ref()
        .map(|s| format!("@{}", s.handle))
        .unwrap_or_else(|| "@—".into());

    let notif = if app.unread_notifs > 0 {
        format!("  Notifs  {}", app.unread_notifs)
    } else {
        "  Notifs".into()
    };

    let items: [(&str, &str); 6] = [
        ("home", "  Home"),
        ("search", "  Search"),
        ("notifs", &notif),
        ("messages", "  Messages"),
        ("profile", "  Profile"),
        ("post", "  Post"),
    ];
    // Profile = Shift+p (your page). Plain p opens the selected author.
    let keys = ["", "/", "i", "m", "P", "n"];

    // ASCII "conr" wordmark — bold wave when returning home.
    let mut lines: Vec<Line> = conr_logo_lines(app.logo_wave_secs());
    lines.push(Line::from(Span::styled("  ╶─────────────╴", style_dim())));

    for (i, (id, label)) in items.iter().enumerate() {
        let selected = *id == active;
        let key = keys[i];
        let row = if key.is_empty() {
            (*label).to_string()
        } else {
            format!("{label:<12} {key}")
        };
        let style = if selected {
            Style::default()
                .fg(TEXT)
                .bg(SELECT_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT).bg(SURFACE)
        };
        let pulse = ((app.ui_secs() * 2.8).sin() + 1.0) * 0.5;
        let marker = if selected {
            let c = lerp_color(PURPLE, PURPLE_HOT, pulse);
            Span::styled("▌", Style::default().fg(c).bg(SELECT_BG))
        } else {
            Span::styled(" ", Style::default().bg(SURFACE))
        };
        lines.push(Line::from(vec![marker, Span::styled(row, style)]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {handle}"),
        style_accent(),
    )));
    // Breathing live indicator.
    let live_line = if app.ws_connected {
        let pulse = ((app.ui_secs() * 2.4).sin() + 1.0) * 0.5;
        let c = lerp_color(OK, PURPLE_HOT, pulse * 0.55);
        Line::from(vec![
            Span::styled("  ● ", Style::default().fg(c)),
            Span::styled("live", Style::default().fg(c).add_modifier(Modifier::BOLD)),
        ])
    } else {
        Line::from(Span::styled("  ○ offline", style_dim()))
    };
    lines.push(live_line);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Tab feed mode", style_dim())));
    lines.push(Line::from(Span::styled("  ? help · q quit", style_dim())));

    let content_rows = lines.len() as u16;
    frame.render_widget(Paragraph::new(lines), inner);

    // Sparkles only on home sidebar, in the free space under the nav.
    if active == "home" && inner.height > content_rows + 1 {
        let sparkle = Rect {
            x: inner.x,
            y: inner.y + content_rows,
            width: inner.width,
            height: inner.height - content_rows,
        };
        draw_sparkle_field_on(frame, sparkle, app.ui_secs(), 0.75, SURFACE);
    }
}

/// Big sidebar wordmark. Default is bold; one slow crest un-bolds as it passes.
fn conr_logo_lines(wave_t: Option<f32>) -> Vec<Line<'static>> {
    const LOGO: [&str; 4] = [
        "╔══╗╔══╗╔══╗╔═╗",
        "║╔═╝║╔╗║║╔╗║║╔╝",
        "║╚═╗║╚╝║║║║║║║ ",
        "╚══╝╚══╝╚╝╚╝╚╝ ",
    ];

    LOGO.iter()
        .enumerate()
        .map(|(row, text)| {
            let mut spans: Vec<Span> = vec![Span::raw("  ")];
            for (i, ch) in text.chars().enumerate() {
                let style = logo_char_style(wave_t, row, i);
                spans.push(Span::styled(ch.to_string(), style));
            }
            Line::from(spans)
        })
        .collect()
}

fn logo_char_style(wave_t: Option<f32>, row: usize, col: usize) -> Style {
    // Resting look: bold purple.
    let bold_accent = Style::default().fg(PURPLE).add_modifier(Modifier::BOLD);
    let Some(t) = wave_t else {
        return bold_accent;
    };

    // Single slow crest, left → right. Wave *removes* bold so it's visible
    // against the bold default.
    const WAVE_SECS: f32 = 2.8;
    const LOGO_COLS: f32 = 15.0;
    // Long soft band (~half the wordmark).
    const CREST_HALF: f32 = 5.5;

    let x = col as f32 + row as f32 * 0.25;
    let peak = -CREST_HALF + (t / WAVE_SECS) * (LOGO_COLS + CREST_HALF * 2.0);
    let dist = (x - peak).abs();
    let u = (dist / CREST_HALF).clamp(0.0, 1.0);
    // 1.0 at center of wave, 0.0 outside.
    let intensity = (1.0 - u) * (1.0 - u);

    if intensity > 0.35 {
        // Crest — regular weight + slightly dimmer so the unbold reads clearly.
        Style::default().fg(PURPLE_DIM)
    } else if intensity > 0.08 {
        // Soft edge — regular weight, full purple.
        Style::default().fg(PURPLE)
    } else {
        // Outside the wave — stay bold.
        bold_accent
    }
}

fn draw_timeline(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    let header_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(BORDER))
        .style(style_bg());
    let header_inner = header_block.inner(chunks[0]);
    frame.render_widget(header_block, chunks[0]);

    let phase = app.ui_secs();
    let (you_style, fol_style) = match app.feed_tab {
        FeedTab::All => (
            Style::default().fg(PURPLE_HOT).add_modifier(Modifier::BOLD),
            style_dim(),
        ),
        FeedTab::Following => (
            style_dim(),
            Style::default().fg(PURPLE_HOT).add_modifier(Modifier::BOLD),
        ),
    };
    let header_lines = vec![
        Line::from(Span::styled("  Home", style_bold())),
        Line::from(vec![
            Span::styled("  For you", you_style),
            Span::styled("   ·   ", style_dim()),
            Span::styled("Following", fol_style),
            Span::styled("            Tab switch", style_dim()),
        ]),
        gradient_hairline(header_inner.width as usize, phase),
    ];
    frame.render_widget(Paragraph::new(header_lines), header_inner);

    let feed_inner = chunks[1];
    let lines = build_feed_lines(app, feed_inner.width as usize);
    frame.render_widget(Paragraph::new(lines).style(style_bg()), feed_inner);

    let status = if app.status.is_empty() {
        "  j/k scroll · n post · r reply · ␣ like · ⏎ thread · m messages · / search · ?"
            .to_string()
    } else {
        format!("  {}", app.status)
    };
    frame.render_widget(
        Paragraph::new(status_gradient_line(&status, phase))
            .style(Style::default().bg(SURFACE2)),
        chunks[2],
    );
}

// ── Thread view ─────────────────────────────────────────────────────────────

fn draw_thread(frame: &mut Frame, area: Rect, app: &App) {
    let (root_id, posts, selected) = match &app.thread {
        Some(th) => (th.root_id.as_str(), th.posts.as_slice(), th.selected),
        None => ("", &[][..], 0),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    let header_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(BORDER))
        .style(style_bg());
    let header_inner = header_block.inner(chunks[0]);
    frame.render_widget(header_block, chunks[0]);

    let root_handle = posts
        .iter()
        .find(|p| p.id == root_id)
        .map(|p| format!("@{}", p.author_handle))
        .unwrap_or_else(|| "thread".into());
    let header_lines = vec![
        Line::from(Span::styled(
            format!("  Thread · {root_handle}"),
            style_bold(),
        )),
        Line::from(Span::styled(
            format!("  {} post(s)  ·  Esc back", posts.len()),
            style_dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(header_lines), header_inner);

    let feed_inner = chunks[1];
    let lines = build_thread_lines(posts, selected, feed_inner.width as usize);
    frame.render_widget(Paragraph::new(lines).style(style_bg()), feed_inner);

    let status = if app.status.is_empty() {
        "  j/k  ·  r reply  ·  ␣ like  ·  d delete  ·  u refresh  ·  Esc back".to_string()
    } else {
        format!("  {}", app.status)
    };
    frame.render_widget(
        Paragraph::new(status_gradient_line(&status, app.ui_secs()))
            .style(Style::default().bg(SURFACE2)),
        chunks[2],
    );
}

fn build_thread_lines(
    posts: &[conr_shared::Post],
    selected: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if posts.is_empty() {
        lines.push(Line::from(Span::styled("  (empty thread)", style_dim())));
        return lines;
    }

    for (i, p) in posts.iter().enumerate() {
        let depth = post_depth(posts, p);
        let indent = "  ".repeat(depth.min(12));
        let selected_row = i == selected;
        let row_bg = if selected_row { SELECT_BG } else { BG };
        let content_width = width.saturating_sub(4 + indent.chars().count() * 2).max(12);

        lines.push(Line::from(Span::styled(
            format!(
                "{indent}{}",
                "─".repeat(width.saturating_sub(indent.len()).min(120))
            ),
            Style::default().fg(BORDER).bg(row_bg),
        )));

        let avatar = avatar_glyph(&p.author_handle);
        let time = relative_time(p.created_at);
        let handle = p.author_handle.clone();
        let marker = if depth == 0 { " " } else { "↳" };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{indent}{marker}"),
                Style::default().fg(MUTED).bg(row_bg),
            ),
            Span::styled(
                format!(" {avatar} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(AVATAR_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" @{handle}"),
                Style::default()
                    .fg(TEXT)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {time}"), Style::default().fg(MUTED).bg(row_bg)),
        ]));

        let body = p.display_content();
        let body_style = if p.deleted_at.is_some() {
            Style::default()
                .fg(MUTED)
                .bg(row_bg)
                .add_modifier(Modifier::ITALIC)
        } else {
            Style::default().fg(TEXT).bg(row_bg)
        };
        for wrap_line in wrap_text(body, content_width) {
            lines.push(Line::from(Span::styled(
                format!("{indent}    {wrap_line}"),
                body_style,
            )));
        }

        let like_label = if p.liked_by_me {
            format!("♥ {}", p.like_count)
        } else {
            format!("♡ {}", p.like_count)
        };
        let like_style = if p.liked_by_me {
            Style::default().fg(LIKE).bg(row_bg)
        } else {
            Style::default().fg(MUTED).bg(row_bg)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{indent}    ↩ {}  ", p.reply_count),
                Style::default().fg(MUTED).bg(row_bg),
            ),
            Span::styled(like_label, like_style),
            if selected_row {
                Span::styled("  · selected", Style::default().fg(PURPLE).bg(row_bg))
            } else {
                Span::styled("", Style::default().bg(row_bg))
            },
        ]));
        lines.push(Line::from(Span::styled(" ", Style::default().bg(row_bg))));
    }
    lines
}

fn build_feed_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let content_width = width.saturating_sub(6).max(20);

    if app.posts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Your timeline is empty",
            style_bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  Posts from everyone (or people you follow) land here.",
            style_dim(),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  n  write a post",
            Style::default().fg(PURPLE),
        )));
        lines.push(Line::from(Span::styled(
            "  /  find people  ·  Tab  following only",
            style_dim(),
        )));
        return lines;
    }

    let phase = app.ui_secs();
    let sel_pulse = ((phase * 2.6).sin() + 1.0) * 0.5;
    let sel_bar = lerp_color(PURPLE, PURPLE_HOT, sel_pulse);

    let start = app.feed_scroll.min(app.posts.len().saturating_sub(1));
    for (i, p) in app.posts.iter().enumerate().skip(start) {
        let selected = i == app.selected;
        let row_bg = if selected { SELECT_BG } else { BG };
        let gutter = if selected { "▌" } else { " " };
        let gutter_style = if selected {
            Style::default().fg(sel_bar).bg(row_bg)
        } else {
            Style::default().fg(BORDER).bg(row_bg)
        };

        // Divider
        lines.push(Line::from(vec![
            Span::styled(gutter, gutter_style),
            Span::styled(
                "─".repeat(width.saturating_sub(1).min(199)),
                Style::default().fg(BORDER).bg(row_bg),
            ),
        ]));

        // Header: avatar + @handle · time
        let avatar = avatar_glyph(&p.author_handle);
        let time = relative_time(p.created_at);
        let handle = p.author_handle.clone();

        lines.push(Line::from(vec![
            Span::styled(gutter, gutter_style),
            Span::styled(
                format!(" {avatar} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(AVATAR_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" @{handle}"),
                Style::default()
                    .fg(TEXT)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {time}"), Style::default().fg(MUTED).bg(row_bg)),
            if p.edited_at.is_some() {
                Span::styled(" · edited", Style::default().fg(MUTED).bg(row_bg))
            } else {
                Span::styled("", Style::default().bg(row_bg))
            },
        ]));

        // Body (wrapped); tombstones render as [Deleted]
        let body = p.display_content();
        let body_style = if p.deleted_at.is_some() {
            Style::default()
                .fg(MUTED)
                .bg(row_bg)
                .add_modifier(Modifier::ITALIC)
        } else {
            Style::default().fg(TEXT).bg(row_bg)
        };
        for wrap_line in wrap_text(body, content_width) {
            lines.push(Line::from(vec![
                Span::styled(gutter, gutter_style),
                Span::styled(format!("   {wrap_line}"), body_style),
            ]));
        }

        // Action row with live counts
        let action_style = Style::default().fg(MUTED).bg(row_bg);
        let like_label = if p.liked_by_me {
            format!("♥ {}", p.like_count)
        } else {
            format!("♡ {}", p.like_count)
        };
        let like_style = if p.liked_by_me || selected {
            Style::default().fg(LIKE).bg(row_bg)
        } else {
            action_style
        };
        let sel_hint = if selected {
            Span::styled(
                "  · selected",
                Style::default().fg(sel_bar).bg(row_bg),
            )
        } else {
            Span::styled("", Style::default().bg(row_bg))
        };
        lines.push(Line::from(vec![
            Span::styled(gutter, gutter_style),
            Span::styled(format!("   ↩ {}   ", p.reply_count), action_style),
            Span::styled(format!("{like_label}   "), like_style),
            Span::styled(
                "↗",
                Style::default()
                    .fg(if selected { RT } else { MUTED })
                    .bg(row_bg),
            ),
            sel_hint,
        ]));
        lines.push(Line::from(Span::styled(" ", Style::default().bg(row_bg))));
    }

    lines
}

fn avatar_glyph(handle: &str) -> String {
    handle
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

fn relative_time(ts: DateTime<Utc>) -> String {
    let now = Utc::now();
    let secs = (now - ts).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else if secs < 86400 * 7 {
        format!("{}d", secs / 86400)
    } else {
        ts.format("%b %d").to_string()
    }
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    for paragraph in s.split('\n') {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if line.is_empty() {
                // hard-break very long words
                if word.chars().count() > width {
                    let mut acc = String::new();
                    for ch in word.chars() {
                        if acc.chars().count() >= width {
                            out.push(acc);
                            acc = String::new();
                        }
                        acc.push(ch);
                    }
                    line = acc;
                } else {
                    line = word.to_string();
                }
            } else if line.chars().count() + 1 + word.chars().count() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(line);
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn draw_right_rail(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(SURFACE).fg(TEXT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let handle = app
        .session
        .as_ref()
        .map(|s| s.handle.as_str())
        .unwrap_or("—");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Compose", style_bold())),
        Line::from(Span::styled("  n  new post", style_dim())),
        Line::from(Span::styled("  r  reply", style_dim())),
        Line::from(""),
        Line::from(Span::styled("  ─────────────", style_dim())),
        Line::from(""),
        Line::from(Span::styled("  You", style_dim())),
        Line::from(Span::styled(format!("  @{handle}"), style_accent())),
        Line::from(""),
        Line::from(Span::styled("  Feed mode", style_dim())),
        Line::from(Span::styled(
            format!("  {}", app.feed_tab.label()),
            style_text(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} loaded", app.posts.len()),
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("  no algo for now((", style_dim())),
        Line::from(Span::styled("  chronological only", style_dim())),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

// ── Composer (tweet box) ────────────────────────────────────────────────────

enum ComposeMode {
    New,
    Reply,
    Edit,
}

fn draw_composer(
    frame: &mut Frame,
    area: Rect,
    textarea: &tui_textarea::TextArea,
    error: Option<&str>,
    mode: ComposeMode,
) {
    let (title, prompt) = match mode {
        ComposeMode::New => ("Post", "What's happening?"),
        ComposeMode::Reply => ("Reply", "Post your reply"),
        ComposeMode::Edit => ("Edit", "Edit your post"),
    };
    let inner = begin_popup(frame, centered_rect(70, 42, area), title);

    let content = textarea.lines().join("\n");
    let count = content.chars().count();
    let over = count > POST_MAX_LEN;
    let count_style = if over {
        style_err()
    } else if count > POST_MAX_LEN * 9 / 10 {
        Style::default().fg(Color::Rgb(0xff, 0xd4, 0x00))
    } else {
        style_dim()
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(prompt, style_dim())))
            .style(Style::default().bg(SURFACE)),
        chunks[0],
    );

    let mut ta = textarea.clone();
    ta.set_style(Style::default().fg(TEXT).bg(SURFACE));
    ta.set_cursor_style(Style::default().bg(PURPLE).fg(Color::Black));
    frame.render_widget(&ta, chunks[1]);

    let mut foot = vec![Line::from(vec![
        Span::styled("Ctrl+S Post", style_accent()),
        Span::styled("  ·  Esc cancel  ·  ", style_dim()),
        Span::styled(format!("{count}/{POST_MAX_LEN}"), count_style),
    ])];
    if let Some(e) = error {
        foot.insert(0, Line::from(Span::styled(e, style_err())));
    }
    frame.render_widget(
        Paragraph::new(foot).style(Style::default().bg(SURFACE)),
        chunks[2],
    );
}

// ── Overlays ────────────────────────────────────────────────────────────────

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = begin_popup(frame, centered_rect(60, 85, area), "Help");
    let lines = vec![
        Line::from(Span::styled("  Keyboard shortcuts", style_bold())),
        Line::from(""),
        Line::from(Span::styled("  Feed", style_dim())),
        Line::from(vec![
            Span::styled("  j/k  n r e ␣ ⏎ d  ", style_accent()),
            Span::styled("scroll/post/reply/edit/like/thread/del", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  Tab             ", style_accent()),
            Span::styled("For you ↔ Following", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  /  i  m  p  f   ", style_accent()),
            Span::styled("search · notifs · DMs · author profile · follow", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  Shift+p        ", style_accent()),
            Span::styled("your own profile", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  u  ?  q  S-L    ", style_accent()),
            Span::styled("refresh · help · quit · logout", style_text()),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Profile", style_dim())),
        Line::from(vec![
            Span::styled("  f b m           ", style_accent()),
            Span::styled("follow · block · DM (others)", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  e               ", style_accent()),
            Span::styled("edit bio (your profile only)", style_text()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  #tags and @mentions work in posts.",
            style_dim(),
        )),
        Line::from(Span::styled("  Esc to close", style_dim())),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(SURFACE).fg(TEXT)),
        popup,
    );
}

fn draw_search(frame: &mut Frame, area: Rect, app: &App, state: &crate::phase_c::SearchState) {
    let (side, main, rail) = shell_columns(area);
    draw_sidebar(frame, side, app, "search");
    if let Some(r) = rail {
        draw_right_rail(frame, r, app);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(main);

    let cursor = if state.editing { "▌" } else { "" };
    let q = if state.query.is_empty() && state.editing {
        format!("  Search posts, @people, #tags {cursor}")
    } else {
        format!("  {}", state.query) + cursor
    };
    let box_style = if state.editing {
        Style::default().fg(PURPLE)
    } else {
        Style::default().fg(BORDER)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("  Explore", style_bold())),
            Line::from(Span::styled(q, style_text())),
        ])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(box_style)
                .style(style_bg()),
        ),
        chunks[0],
    );

    let mut lines = Vec::new();
    if let Some(e) = &state.error {
        lines.push(Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    for (i, hit) in state.hits.iter().enumerate() {
        let sel = !state.editing && i == state.selected;
        let bg = if sel { SELECT_BG } else { BG };
        let (kind, body) = match hit {
            SearchHit::Post(p) => (
                "post",
                format!("@{}  {}", p.author_handle, p.display_content()),
            ),
            SearchHit::User(u) => (
                "user",
                format!(
                    "@{}  · {} followers · {} posts",
                    u.handle, u.followers, u.posts
                ),
            ),
            SearchHit::Tag(n, c) => ("tag", format!("#{n}  · {c} posts")),
        };
        let kind_style = Style::default().fg(PURPLE).bg(bg);
        let body_style = if sel {
            Style::default()
                .fg(TEXT)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT).bg(bg)
        };
        let trunc: String = body
            .chars()
            .take((main.width as usize).saturating_sub(12))
            .collect();
        lines.push(Line::from(vec![
            Span::styled(format!("  {kind:<4}  "), kind_style),
            Span::styled(trunc, body_style),
        ]));
    }
    if state.hits.is_empty() && state.error.is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Type a query and press Enter",
            style_dim(),
        )));
        lines.push(Line::from(Span::styled(
            "  Try  rust   @alice   #conr",
            style_dim(),
        )));
    }
    frame.render_widget(Paragraph::new(lines).style(style_bg()), chunks[1]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {} · Enter open · / retype · Esc back", app.status),
            style_dim(),
        )))
        .style(Style::default().bg(SURFACE2)),
        chunks[2],
    );
}

fn draw_profile(frame: &mut Frame, area: Rect, app: &App, state: &crate::phase_c::ProfileState) {
    let (side, main, _) = shell_columns(area);
    draw_sidebar(frame, side, app, "profile");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(main);

    let p = &state.profile;
    let avatar = avatar_glyph(&p.handle);
    // Own profile: edit bio only. Others: follow / DM / block — never "e bio".
    let actions = if p.is_me {
        "you  ·  e edit bio  ·  Esc".to_string()
    } else {
        let follow = if p.followed_by_me {
            "f unfollow"
        } else {
            "f follow"
        };
        let block = if p.blocked_by_me {
            "b unblock"
        } else {
            "b block"
        };
        format!("{follow}  ·  m message  ·  {block}  ·  Esc")
    };
    let header = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!(" {avatar} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(AVATAR_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  @{}", p.handle), style_bold()),
        ]),
        Line::from(Span::styled(
            format!("    {}", p.bio.as_deref().unwrap_or("No bio yet.")),
            style_dim(),
        )),
        Line::from(Span::styled(
            format!(
                "    {} posts   {} followers   {} following",
                p.posts, p.followers, p.following
            ),
            style_text(),
        )),
        Line::from(Span::styled(
            format!("    {actions}"),
            Style::default().fg(PURPLE),
        )),
    ];
    frame.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(BORDER))
                .style(style_bg()),
        ),
        chunks[0],
    );

    let mut lines = Vec::new();
    if state.posts.is_empty() {
        lines.push(Line::from(Span::styled("  No posts yet.", style_dim())));
    } else {
        for (i, post) in state.posts.iter().enumerate() {
            let sel = i == state.selected;
            let bg = if sel { SELECT_BG } else { BG };
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} · {}",
                    relative_time(post.created_at),
                    if post.edited_at.is_some() {
                        "edited"
                    } else {
                        ""
                    }
                ),
                Style::default().fg(MUTED).bg(bg),
            )));
            let body: String = post.display_content().chars().take(90).collect();
            lines.push(Line::from(Span::styled(
                format!("  {body}"),
                Style::default()
                    .fg(if post.deleted_at.is_some() {
                        MUTED
                    } else {
                        TEXT
                    })
                    .bg(bg),
            )));
            lines.push(Line::from(Span::styled(
                format!("  ↩ {}   ♡ {}", post.reply_count, post.like_count),
                Style::default().fg(MUTED).bg(bg),
            )));
            lines.push(Line::from(Span::styled(" ", Style::default().bg(bg))));
        }
    }
    frame.render_widget(Paragraph::new(lines).style(style_bg()), chunks[1]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", app.status),
            style_dim(),
        )))
        .style(Style::default().bg(SURFACE2)),
        chunks[2],
    );
}

fn draw_notifications(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &crate::phase_c::NotifState,
) {
    let (side, main, _) = shell_columns(area);
    draw_sidebar(frame, side, app, "notifs");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(main);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("  Notifications", style_bold())),
            Line::from(Span::styled("  Enter open · Esc back", style_dim())),
        ])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(BORDER))
                .style(style_bg()),
        ),
        chunks[0],
    );

    let mut lines = Vec::new();
    if state.items.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  You're all caught up.",
            style_dim(),
        )));
    }
    for (i, n) in state.items.iter().enumerate() {
        let sel = i == state.selected;
        let bg = if sel { SELECT_BG } else { BG };
        let (icon, text) = match n.kind.as_str() {
            "like" => ("♡", format!("@{} liked your post", n.actor_handle)),
            "reply" => ("↩", format!("@{} replied to you", n.actor_handle)),
            "follow" => ("+", format!("@{} followed you", n.actor_handle)),
            "mention" => ("@", format!("@{} mentioned you", n.actor_handle)),
            other => ("·", format!("@{} · {other}", n.actor_handle)),
        };
        let icon_col = match n.kind.as_str() {
            "like" => LIKE,
            "follow" => OK,
            "mention" => PURPLE,
            _ => MUTED,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {icon}  "), Style::default().fg(icon_col).bg(bg)),
            Span::styled(text, Style::default().fg(TEXT).bg(bg)),
            Span::styled(
                format!("  {}", relative_time(n.created_at)),
                Style::default().fg(MUTED).bg(bg),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).style(style_bg()), chunks[1]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", app.status),
            style_dim(),
        )))
        .style(Style::default().bg(SURFACE2)),
        chunks[2],
    );
}

fn draw_dm_list(frame: &mut Frame, area: Rect, app: &App, state: &crate::phase_c::DmListState) {
    let (side, main, _) = shell_columns(area);
    draw_sidebar(frame, side, app, "messages");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(main);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("  Messages", style_bold())),
            Line::from(Span::styled(
                "  n new chat  ·  Enter open  ·  u refresh",
                style_dim(),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(BORDER))
                .style(style_bg()),
        ),
        chunks[0],
    );

    let mut lines = Vec::new();
    if state.items.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No conversations yet.",
            style_dim(),
        )));
        lines.push(Line::from(Span::styled(
            "  Press n and type a handle to start chatting.",
            style_dim(),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Or open a profile (p) and press m.",
            Style::default().fg(PURPLE),
        )));
    }
    for (i, c) in state.items.iter().enumerate() {
        let sel = i == state.selected;
        let bg = if sel { SELECT_BG } else { BG };
        let preview = c
            .last_message
            .as_deref()
            .map(|s| {
                let t: String = s.chars().take(48).collect();
                t
            })
            .unwrap_or_else(|| "—".into());
        let time = c.last_at.map(relative_time).unwrap_or_else(|| "".into());
        let av = avatar_glyph(&c.peer_handle);
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {av} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(if sel { PURPLE } else { PURPLE_DIM }),
            ),
            Span::styled(
                format!(" @{}", c.peer_handle),
                Style::default()
                    .fg(TEXT)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {time}"), Style::default().fg(MUTED).bg(bg)),
        ]));
        lines.push(Line::from(Span::styled(
            format!("     {preview}"),
            Style::default().fg(MUTED).bg(bg),
        )));
        lines.push(Line::from(Span::styled(" ", Style::default().bg(bg))));
    }
    frame.render_widget(Paragraph::new(lines).style(style_bg()), chunks[1]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", app.status),
            style_dim(),
        )))
        .style(Style::default().bg(SURFACE2)),
        chunks[2],
    );
}

fn draw_dm_new(frame: &mut Frame, area: Rect, app: &App, state: &crate::phase_c::DmNewState) {
    let (side, main, _) = shell_columns(area);
    draw_sidebar(frame, side, app, "messages");

    let popup = centered_rect(50, 40, main);
    frame.render_widget(Clear, popup);

    let mut lines = vec![
        Line::from(Span::styled("  New message", style_bold())),
        Line::from(""),
        Line::from(Span::styled("  Who do you want to message?", style_dim())),
        Line::from(""),
        Line::from(Span::styled(
            format!("  @{}▌", state.handle),
            style_accent(),
        )),
        Line::from(""),
        Line::from(Span::styled("  Enter continue · Esc cancel", style_dim())),
    ];
    if let Some(e) = &state.error {
        lines.insert(5, Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(PURPLE))
                .style(Style::default().bg(SURFACE).fg(TEXT))
                .padding(Padding::new(1, 1, 0, 0)),
        ),
        popup,
    );
}

fn draw_dm_thread(frame: &mut Frame, area: Rect, app: &App, state: &crate::phase_c::DmThreadState) {
    let (side, main, _) = shell_columns(area);
    draw_sidebar(frame, side, app, "messages");

    let me = app
        .session
        .as_ref()
        .map(|s| s.handle.as_str())
        .unwrap_or("");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(main);

    // Header
    let title = format!("  @{}", state.peer_handle);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(title, style_bold())),
            Line::from(Span::styled(
                "  Enter send  ·  Esc back to inbox",
                style_dim(),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(BORDER))
                .style(style_bg()),
        ),
        chunks[0],
    );

    // Message bubbles
    let mut lines: Vec<Line> = Vec::new();
    if state.messages.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  Say hi to @{}…", state.peer_handle),
            style_dim(),
        )));
    } else {
        // show a window ending near scroll
        let total = state.messages.len();
        let view_h = (chunks[1].height as usize).saturating_sub(1).max(4);
        let end = (state.scroll + 1).min(total);
        let start = end.saturating_sub(view_h);
        for m in &state.messages[start..end] {
            let mine = m.sender_handle == me;
            let time = relative_time(m.created_at);
            let max_w = (chunks[1].width as usize).saturating_sub(8).max(20);
            let body_lines = wrap_text(&m.content, max_w.saturating_sub(4));

            if mine {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{:>width$}",
                        format!("you · {time}"),
                        width = chunks[1].width as usize
                    ),
                    style_dim(),
                )));
                for bl in &body_lines {
                    let pad = chunks[1].width as usize;
                    let bubble = format!("  {bl}  ");
                    let styled = format!("{:>width$}", bubble, width = pad);
                    lines.push(Line::from(Span::styled(
                        styled,
                        Style::default().fg(TEXT).bg(BUBBLE_ME),
                    )));
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  @{} · {time}", m.sender_handle),
                    style_dim(),
                )));
                for bl in &body_lines {
                    lines.push(Line::from(Span::styled(
                        format!("  {bl}  "),
                        Style::default().fg(TEXT).bg(BUBBLE_THEM),
                    )));
                }
            }
            lines.push(Line::from(""));
        }
    }
    frame.render_widget(Paragraph::new(lines).style(style_bg()), chunks[1]);

    // Composer
    let count = state.draft.lines().join("\n").chars().count();
    let mut ta = state.draft.clone();
    ta.set_style(Style::default().fg(TEXT).bg(SURFACE2));
    ta.set_cursor_style(Style::default().bg(PURPLE).fg(Color::Black));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PURPLE))
        .title(Span::styled(
            format!(" message  {count}/{POST_MAX_LEN} "),
            style_accent(),
        ))
        .style(Style::default().bg(SURFACE2));
    let inner = block.inner(chunks[2]);
    frame.render_widget(block, chunks[2]);
    frame.render_widget(&ta, inner);

    let mut foot = app.status.clone();
    if let Some(e) = &state.error {
        foot = e.clone();
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {foot}"),
            if state.error.is_some() {
                style_err()
            } else {
                style_dim()
            },
        )))
        .style(Style::default().bg(SURFACE2)),
        chunks[3],
    );
}

fn draw_edit_bio(frame: &mut Frame, area: Rect, input: &str, error: Option<&str>) {
    let popup = begin_popup(frame, centered_rect(60, 30, area), "Bio");
    let mut lines = vec![
        Line::from(Span::styled("  Edit bio (160 chars)", style_bold())),
        Line::from(""),
        Line::from(Span::styled(format!("  {input}_"), style_text())),
        Line::from(""),
        Line::from(Span::styled("  Enter save · Esc cancel", style_dim())),
    ];
    if let Some(e) = error {
        lines.insert(3, Line::from(Span::styled(format!("  {e}"), style_err())));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(SURFACE).fg(TEXT)),
        popup,
    );
}

fn draw_block_user(
    frame: &mut Frame,
    area: Rect,
    handle: &str,
    reason: &str,
    error: Option<&str>,
) {
    use conr_shared::BLOCK_REASON_MAX_LEN;
    let popup = begin_popup(frame, centered_rect(64, 40, area), "Block");
    let n = reason.chars().count();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("  Block @{handle}?"),
            style_bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  A reason is required (you won't see their posts).",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {reason}_"),
            style_text(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {n}/{BLOCK_REASON_MAX_LEN}  ·  Enter block  ·  Esc cancel"),
            style_dim(),
        )),
    ];
    if let Some(e) = error {
        lines.insert(
            5,
            Line::from(Span::styled(format!("  {e}"), style_err())),
        );
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(SURFACE).fg(TEXT)),
        popup,
    );
}

fn draw_confirm_delete(frame: &mut Frame, area: Rect, preview: &str) {
    let popup = begin_popup(frame, centered_rect(56, 36, area), "Confirm");
    let lines = vec![
        Line::from(Span::styled("  Delete this post?", style_bold())),
        Line::from(""),
        Line::from(Span::styled(format!("  “{preview}”"), style_dim())),
        Line::from(""),
        Line::from(Span::styled(
            "  Replies stay; content becomes [Deleted].",
            style_dim(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  y / Enter  ", style_accent()),
            Span::styled("delete", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  n / Esc    ", style_accent()),
            Span::styled("cancel", style_text()),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(SURFACE).fg(TEXT)),
        popup,
    );
}

fn draw_reminder(frame: &mut Frame, area: Rect, message: &str) {
    let popup = begin_popup(frame, centered_rect(64, 45, area), "Reminder");
    let lines = vec![
        Line::from(Span::styled("  Token reminder", style_bold())),
        Line::from(""),
        Line::from(Span::styled(format!("  {message}"), style_text())),
        Line::from(""),
        Line::from(Span::styled(
            "  The server cannot resend your token.",
            style_dim(),
        )),
        Line::from(Span::styled(
            "  Keep it in a password manager.",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("  Enter dismiss", style_ok())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(SURFACE).fg(TEXT)),
        popup,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
