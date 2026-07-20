use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use chrono::{DateTime, Utc};
use conr_shared::{format_token_block, POST_MAX_LEN};

use crate::app::{App, MenuEntry, Screen};

// ── Palette (dark mode, purple accent) ──────────────────────────────────────

const BG: Color = Color::Rgb(0x00, 0x00, 0x00);
const SURFACE: Color = Color::Rgb(0x16, 0x18, 0x1c);
const BORDER: Color = Color::Rgb(0x2f, 0x33, 0x36);
const TEXT: Color = Color::Rgb(0xe7, 0xe9, 0xea);
const MUTED: Color = Color::Rgb(0x71, 0x76, 0x7b);
const PURPLE: Color = Color::Rgb(0x9b, 0x59, 0xf5);
const LIKE: Color = Color::Rgb(0xf9, 0x18, 0x80);
const RT: Color = Color::Rgb(0x00, 0xba, 0x7c);
const ERR: Color = Color::Rgb(0xf4, 0x21, 0x2e);
const OK: Color = Color::Rgb(0x00, 0xba, 0x7c);
const SELECT_BG: Color = Color::Rgb(0x1a, 0x1a, 0x1a);
const AVATAR_BG: Color = Color::Rgb(0x9b, 0x59, 0xf5);

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

// ── Entry ───────────────────────────────────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    fill_bg(frame, area);
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
        Screen::Compose { textarea, error } => {
            draw_feed(frame, area, app);
            draw_composer(frame, area, textarea, error.as_deref());
        }
        Screen::Reminder { message } => draw_reminder(frame, area, message),
        Screen::Help => {
            draw_feed(frame, area, app);
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
    let popup = centered_rect(56, 70, area);
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

    let block = card_block("Welcome");
    frame.render_widget(
        Paragraph::new(all).block(block).alignment(Alignment::Left),
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

// ── Main feed (Twitter-style chrome) ────────────────────────────────────────

fn draw_feed(frame: &mut Frame, area: Rect, app: &App) {
    // Three-column layout: nav | timeline | right rail (when wide enough)
    let wide = area.width >= 100;
    let mid = if wide {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(22),
                Constraint::Min(40),
                Constraint::Length(28),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(18), Constraint::Min(30)])
            .split(area)
    };

    draw_sidebar(frame, mid[0], app);
    draw_timeline(frame, mid[1], app);
    if wide {
        draw_right_rail(frame, mid[2], app);
    }
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(BORDER))
        .style(style_bg());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let handle = app
        .session
        .as_ref()
        .map(|s| format!("@{}", s.handle))
        .unwrap_or_else(|| "@—".into());

    // Unicode symbols only (no emoji presentation).
    let nav = [
        ("  ◆", true, false),
        ("", false, false),
        ("  ⌂  Home", true, true),
        ("  ⌕  Explore", false, false),
        ("  ◉  Notifications", false, false),
        ("  ▤  Messages", false, false),
        ("  ★  Bookmarks", false, false),
        ("  ○  Profile", false, false),
        ("  ··· More", false, false),
        ("", false, false),
        ("  +  Post", true, false),
    ];

    let mut lines: Vec<Line> = Vec::new();
    for (label, active, selected) in nav {
        if label.is_empty() {
            lines.push(Line::from(""));
            continue;
        }
        let style = if selected {
            Style::default()
                .fg(TEXT)
                .bg(SELECT_BG)
                .add_modifier(Modifier::BOLD)
        } else if active {
            style_bold()
        } else {
            style_dim()
        };
        lines.push(Line::from(Span::styled(label, style)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {handle}"),
        style_accent(),
    )));

    let conn = if app.ws_connected {
        ("  * Live", style_ok())
    } else {
        ("  o Offline", style_dim())
    };
    lines.push(Line::from(Span::styled(conn.0, conn.1)));

    // Bottom key hints squeezed into sidebar when short
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  n post  r refresh", style_dim())));
    lines.push(Line::from(Span::styled("  j/k  ?  q", style_dim())));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_timeline(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    // Header: Home + underline tab
    let header_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(BORDER))
        .style(style_bg());
    let header_inner = header_block.inner(chunks[0]);
    frame.render_widget(header_block, chunks[0]);

    let header_lines = vec![
        Line::from(Span::styled("  Home", style_bold())),
        Line::from(vec![
            Span::styled("  For you", style_accent()),
            Span::styled("          Following", style_dim()),
        ]),
    ];
    frame.render_widget(Paragraph::new(header_lines), header_inner);

    // Tweet list
    let feed_block = Block::default().style(style_bg());
    let feed_inner = feed_block.inner(chunks[1]);
    frame.render_widget(feed_block, chunks[1]);

    let lines = build_feed_lines(app, feed_inner.width as usize);
    frame.render_widget(Paragraph::new(lines), feed_inner);

    // Status strip
    let status = if app.status.is_empty() {
        "  j/k scroll  ·  n What's happening?  ·  r refresh  ·  ? help  ·  q quit".to_string()
    } else {
        format!("  {}", app.status)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(status, style_dim())))
            .style(Style::default().bg(SURFACE).fg(MUTED)),
        chunks[2],
    );
}

fn build_feed_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let content_width = width.saturating_sub(6).max(20);

    if app.posts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Welcome to your timeline",
            style_bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  When people post, they'll show up here.",
            style_dim(),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Press n to post what's happening.",
            Style::default().fg(PURPLE),
        )));
        return lines;
    }

    for (i, p) in app.posts.iter().enumerate() {
        let selected = i == app.selected;
        let row_bg = if selected { SELECT_BG } else { BG };

        // Divider
        lines.push(Line::from(Span::styled(
            "─".repeat(width.min(200)),
            Style::default().fg(BORDER).bg(row_bg),
        )));

        // Header: avatar + @handle · time
        let avatar = avatar_glyph(&p.author_handle);
        let time = relative_time(p.created_at);
        let handle = p.author_handle.clone();

        lines.push(Line::from(vec![
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

        // Body (wrapped)
        for wrap_line in wrap_text(&p.content, content_width) {
            lines.push(Line::from(Span::styled(
                format!("    {wrap_line}"),
                Style::default().fg(TEXT).bg(row_bg),
            )));
        }

        // Action row (visual only — Phase A has no likes/replies)
        let action_style = Style::default().fg(MUTED).bg(row_bg);
        let sel_hint = if selected {
            Span::styled("  · selected", Style::default().fg(PURPLE).bg(row_bg))
        } else {
            Span::styled("", Style::default().bg(row_bg))
        };
        lines.push(Line::from(vec![
            Span::styled("    ↩     ", action_style),
            Span::styled(
                "↻     ",
                Style::default()
                    .fg(if selected { RT } else { MUTED })
                    .bg(row_bg),
            ),
            Span::styled(
                "♥     ",
                Style::default()
                    .fg(if selected { LIKE } else { MUTED })
                    .bg(row_bg),
            ),
            Span::styled("↗", action_style),
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
        .style(style_bg());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let handle = app
        .session
        .as_ref()
        .map(|s| s.handle.as_str())
        .unwrap_or("—");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("  What's happening", style_bold())),
        Line::from(""),
        Line::from(Span::styled(
            "  ┌──────────────────┐",
            Style::default().fg(BORDER),
        )),
        Line::from(Span::styled("  │  press n to post │", style_dim())),
        Line::from(Span::styled(
            "  └──────────────────┘",
            Style::default().fg(BORDER),
        )),
        Line::from(""),
        Line::from(Span::styled("  ───────────────", style_dim())),
        Line::from(""),
        Line::from(Span::styled("  Who to follow", style_bold())),
        Line::from(Span::styled("  (follows ship later)", style_dim())),
        Line::from(""),
        Line::from(Span::styled("  ───────────────", style_dim())),
        Line::from(""),
        Line::from(Span::styled("  You", style_dim())),
        Line::from(Span::styled(format!("  @{handle}"), style_accent())),
        Line::from(""),
        Line::from(Span::styled("  chronological feed", style_dim())),
        Line::from(Span::styled("  no ads · no algo", style_dim())),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} posts loaded", app.posts.len()),
            style_dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

// ── Composer (tweet box) ────────────────────────────────────────────────────

fn draw_composer(
    frame: &mut Frame,
    area: Rect,
    textarea: &tui_textarea::TextArea,
    error: Option<&str>,
) {
    let popup = centered_rect(70, 42, area);
    frame.render_widget(Clear, popup);

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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PURPLE))
        .title(Span::styled(" Post ", style_accent()))
        .style(Style::default().bg(SURFACE).fg(TEXT))
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("What's happening?", style_dim()))),
        chunks[0],
    );

    // Textarea with surface bg
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
    frame.render_widget(Paragraph::new(foot), chunks[2]);
}

// ── Overlays ────────────────────────────────────────────────────────────────

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 55, area);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled("  Keyboard shortcuts", style_bold())),
        Line::from(""),
        Line::from(vec![
            Span::styled("  j / k     ", style_accent()),
            Span::styled("scroll feed", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  n         ", style_accent()),
            Span::styled("new post", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  r         ", style_accent()),
            Span::styled("refresh timeline", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  Shift+L   ", style_accent()),
            Span::styled("logout → accounts", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  q         ", style_accent()),
            Span::styled("quit", style_text()),
        ]),
        Line::from(vec![
            Span::styled("  ?         ", style_accent()),
            Span::styled("this help", style_text()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  conr is chronological. No algorithm.",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("  Esc to close", style_dim())),
    ];
    frame.render_widget(Paragraph::new(lines).block(card_block("Help")), popup);
}

fn draw_reminder(frame: &mut Frame, area: Rect, message: &str) {
    let popup = centered_rect(64, 45, area);
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
            .block(card_block("Reminder")),
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
