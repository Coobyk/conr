use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use conr_shared::{format_token_block, POST_MAX_LEN};

use crate::app::{App, MenuEntry, Screen};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    match &app.screen {
        Screen::Boot => draw_centered(frame, area, "conr", "starting…"),
        Screen::WelcomeMenu { selected } => {
            draw_welcome_menu(frame, area, app, *selected)
        }
        Screen::RegisterHandle { input, error } => {
            draw_input_screen(frame, area, "register — choose a handle", input, error.as_deref())
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
        Screen::BackupPassphrase { input, confirm, stage, error } => {
            draw_backup_pass(frame, area, input, confirm, *stage, error.as_deref())
        }
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

fn style_accent() -> Style {
    Style::default()
        .fg(Color::Rgb(0x81, 0xa2, 0xbe))
        .add_modifier(Modifier::BOLD)
}

fn style_dim() -> Style {
    Style::default().fg(Color::Rgb(0x5a, 0x5a, 0x5a))
}

fn style_err() -> Style {
    Style::default().fg(Color::Rgb(0xcc, 0x66, 0x66))
}

fn style_ok() -> Style {
    Style::default().fg(Color::Rgb(0xb5, 0xbd, 0x68))
}

fn draw_centered(frame: &mut Frame, area: Rect, title: &str, subtitle: &str) {
    let block = Block::default().borders(Borders::ALL).title(" conr ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let p = Paragraph::new(vec![
        Line::from(Span::styled(title, style_accent())),
        Line::from(""),
        Line::from(Span::styled(subtitle, style_dim())),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(p, inner);
}

fn draw_welcome_menu(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    let entries = app.menu_entries();
    let n_saved = app.saved_accounts.len();
    let last = app.reminders.handle.as_deref();

    let mut all = vec![
        Line::from(Span::styled("conr", style_accent())),
        Line::from(Span::styled(
            "terminal-native · privacy-first · chronological",
            style_dim(),
        )),
        Line::from(""),
    ];

    if n_saved > 0 {
        all.push(Line::from(Span::styled("saved accounts", style_dim())));
    }

    for (i, entry) in entries.iter().enumerate() {
        // Separator before the static actions when we have accounts above.
        if n_saved > 0 && i == n_saved {
            all.push(Line::from(Span::styled("  ────────────", style_dim())));
        }

        let label = match entry {
            MenuEntry::Account(h) => {
                if last == Some(h.as_str()) {
                    format!("@{h}  (last)")
                } else {
                    format!("@{h}")
                }
            }
            MenuEntry::Register => "Register new account".into(),
            MenuEntry::ManualLogin => "Log in with token".into(),
            MenuEntry::Quit => "Quit".into(),
        };

        if i == selected {
            all.push(Line::from(Span::styled(format!(" › {label}"), style_accent())));
        } else {
            all.push(Line::from(format!("   {label}")));
        }
    }

    if !app.status.is_empty() {
        all.push(Line::from(""));
        // status can be info or error — dim unless it looks like a failure
        let st = if app.status.contains("fail") || app.status.contains("could not") {
            style_err()
        } else {
            style_dim()
        };
        all.push(Line::from(Span::styled(&app.status, st)));
    }
    all.push(Line::from(""));
    all.push(Line::from(Span::styled(
        "j/k select · Enter open account / confirm",
        style_dim(),
    )));
    let block = Block::default().borders(Borders::ALL).title(" welcome ");
    frame.render_widget(
        Paragraph::new(all).block(block).alignment(Alignment::Left),
        area,
    );
}

fn draw_input_screen(frame: &mut Frame, area: Rect, title: &str, input: &str, error: Option<&str>) {
    let mut lines = vec![
        Line::from(Span::styled(title, style_accent())),
        Line::from(""),
        Line::from(format!("  @{input}█")),
        Line::from(""),
        Line::from(Span::styled(
            "lowercase letters, digits, underscore · 3–32 chars",
            style_dim(),
        )),
        Line::from(Span::styled("Enter submit · Esc back", style_dim())),
    ];
    if let Some(e) = error {
        lines.insert(4, Line::from(Span::styled(e, style_err())));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" conr ")),
        area,
    );
}

fn draw_save_ack(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled("before we show your token", style_accent())),
        Line::from(""),
        Line::from("You will get a 128-character secret. Anyone with it can use your account."),
        Line::from("There is NO server recovery. Save it in a password manager or offline."),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter when you are ready to see and save the token.",
            style_ok(),
        )),
        Line::from(Span::styled("Esc to cancel registration.", style_dim())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" save this ")),
        area,
    );
}

fn draw_token_block(frame: &mut Frame, area: Rect, token: &str, export_status: &str, flash: u8) {
    let block_text = format_token_block(token);
    let path = crate::clipboard::token_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~/.local/share/conr/token-ONCE.txt".into());

    let mut lines = vec![
        Line::from(Span::styled(
            "save this token — you need it to unlock",
            style_accent(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "RELIABLE: open this file in another window and copy from there:",
            style_ok(),
        )),
        Line::from(Span::styled(format!("  {path}"), style_accent())),
        Line::from(Span::styled(
            "  (mode 0600 · delete after you save it somewhere safe)",
            style_dim(),
        )),
        Line::from(""),
    ];
    for line in block_text.lines() {
        lines.push(Line::from(Span::styled(
            format!("    {line}"),
            Style::default()
                .fg(Color::Rgb(0xc5, 0xc8, 0xc6))
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    if !export_status.is_empty() {
        let msg = if flash > 0 {
            format!("exported · {export_status}")
        } else {
            format!("last export · {export_status}")
        };
        lines.push(Line::from(Span::styled(msg, style_ok())));
    }
    lines.push(Line::from(Span::styled(
        "c = re-copy (clipboard + file) · Enter = I saved it, continue",
        style_dim(),
    )));
    lines.push(Line::from(Span::styled(
        "tip: mouse select works now; or: cat ~/.local/share/conr/token-ONCE.txt",
        style_dim(),
    )));
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" identity token "),
        ),
        area,
    );
}

fn draw_welcome_anim(frame: &mut Frame, area: Rect, frame_idx: u8) {
    let dots = ".".repeat((frame_idx as usize % 4) + 1);
    let art = match frame_idx % 6 {
        0 => "    ·  conr  ·",
        1 => "   ··  conr  ··",
        2 => "  ··· welcome ···",
        3 => " ····  conr  ····",
        4 => "····· you're in ·····",
        _ => "      conr      ",
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(art, style_accent())),
        Line::from(""),
        Line::from(Span::styled(
            format!("welcome to conr{dots}"),
            style_dim(),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_unlock(frame: &mut Frame, area: Rect, input: &str, error: Option<&str>, can_reveal: bool) {
    let shown = if input.len() > 32 {
        format!("{}… ({} chars)", &input[..32], input.len())
    } else {
        format!("{input}█")
    };
    let mut lines = vec![
        Line::from(Span::styled("unlock conr", style_accent())),
        Line::from("Paste your token to prove you saved it."),
        Line::from(""),
        Line::from(format!("  {shown}")),
        Line::from(""),
        Line::from(Span::styled(
            "whitespace/symbols stripped · need exactly 128 alphanumerics",
            style_dim(),
        )),
    ];
    if let Some(e) = error {
        lines.push(Line::from(Span::styled(e, style_err())));
    }
    if can_reveal {
        lines.push(Line::from(Span::styled(
            "F1 = view token again (humans are dumb)",
            style_dim(),
        )));
    }
    lines.push(Line::from(Span::styled("Enter unlock", style_dim())));
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" confirm ")),
        area,
    );
}

fn draw_backup_prompt(frame: &mut Frame, area: Rect, selected: usize) {
    let items = [
        "Yes — encrypt a local backup with a passphrase",
        "No — keychain / password manager only",
    ];
    let mut lines = vec![
        Line::from(Span::styled("optional local backup", style_accent())),
        Line::from("Stored under ~/.local/share/conr/backup/ — never sent to the server."),
        Line::from(""),
    ];
    for (i, t) in items.iter().enumerate() {
        if i == selected {
            lines.push(Line::from(Span::styled(format!(" › {t}"), style_accent())));
        } else {
            lines.push(Line::from(format!("   {t}")));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("j/k · Enter", style_dim())));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL)),
        area,
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
    let label = if stage == 0 {
        "passphrase"
    } else {
        "confirm passphrase"
    };
    let val = if stage == 0 { input } else { confirm };
    let masked: String = "*".repeat(val.chars().count());
    let mut lines = vec![
        Line::from(Span::styled("backup passphrase", style_accent())),
        Line::from("This never leaves your machine. We cannot reset it."),
        Line::from(""),
        Line::from(format!("  {label}: {masked}█")),
    ];
    if let Some(e) = error {
        lines.push(Line::from(Span::styled(e, style_err())));
    }
    lines.push(Line::from(Span::styled("Enter continue · Esc skip backup", style_dim())));
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_login(
    frame: &mut Frame,
    area: Rect,
    handle: &str,
    token_input: &str,
    focus: usize,
    error: Option<&str>,
) {
    let h = if focus == 0 {
        format!("  handle: @{handle}█")
    } else {
        format!("  handle: @{handle}")
    };
    let tok_disp = if token_input.len() > 24 {
        format!("{}… ({} chars)", &token_input[..24], token_input.len())
    } else {
        token_input.to_string()
    };
    let t = if focus == 1 {
        format!("  token:  {tok_disp}█")
    } else {
        format!("  token:  {tok_disp}")
    };
    let mut lines = vec![
        Line::from(Span::styled("log in", style_accent())),
        Line::from(""),
        Line::from(h),
        Line::from(t),
        Line::from(""),
        Line::from(Span::styled("Tab switch field · Enter submit · Esc back", style_dim())),
    ];
    if let Some(e) = error {
        lines.insert(5, Line::from(Span::styled(e, style_err())));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" conr ")),
        area,
    );
}

fn draw_feed(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    let handle = app
        .session
        .as_ref()
        .map(|s| format!("@{}", s.handle))
        .unwrap_or_else(|| "—".into());
    let conn = if app.ws_connected {
        "● connected"
    } else {
        "○ offline"
    };
    let status = Paragraph::new(Line::from(vec![
        Span::styled(handle, style_accent()),
        Span::raw(" │ FEED │ "),
        Span::styled(conn, if app.ws_connected { style_ok() } else { style_dim() }),
        Span::raw(" │ "),
        Span::styled(&app.status, style_dim()),
    ]));
    frame.render_widget(status, chunks[0]);

    let items: Vec<ListItem> = if app.posts.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  no posts yet — press n to write one",
            style_dim(),
        )))]
    } else {
        app.posts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let selected = i == app.selected;
                let time = p.created_at.format("%H:%M").to_string();
                let header = format!("@{} · {time}", p.author_handle);
                let style = if selected {
                    Style::default()
                        .bg(Color::Rgb(0x28, 0x2a, 0x2e))
                        .fg(Color::Rgb(0xc5, 0xc8, 0xc6))
                } else {
                    Style::default()
                };
                ListItem::new(vec![
                    Line::from(Span::styled(
                        header,
                        if selected {
                            style_accent()
                        } else {
                            style_dim()
                        },
                    )),
                    Line::from(Span::styled(format!("  {}", p.content), style)),
                    Line::from(""),
                ])
            })
            .collect()
    };

    let list = List::new(items).block(Block::default().borders(Borders::TOP | Borders::BOTTOM));
    frame.render_widget(list, chunks[1]);

    let hints = Paragraph::new(Span::styled(
        " j/k  n new  r refresh  ? help  Shift+L switch/logout  q quit ",
        style_dim(),
    ));
    frame.render_widget(hints, chunks[2]);
}

fn draw_composer(frame: &mut Frame, area: Rect, textarea: &tui_textarea::TextArea, error: Option<&str>) {
    let popup = centered_rect(80, 40, area);
    frame.render_widget(Clear, popup);
    let content = textarea.lines().join("\n");
    let count = content.chars().count();
    let title = format!(" Compose [{count}/{POST_MAX_LEN}] ");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(style_accent());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);

    frame.render_widget(textarea, chunks[0]);
    let mut foot = vec![Line::from(Span::styled(
        "Ctrl+S submit · Esc cancel",
        style_dim(),
    ))];
    if let Some(e) = error {
        foot.insert(0, Line::from(Span::styled(e, style_err())));
    }
    frame.render_widget(Paragraph::new(foot), chunks[1]);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 50, area);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled("help", style_accent())),
        Line::from(""),
        Line::from("j/k     scroll feed"),
        Line::from("n       new post"),
        Line::from("r       refresh feed"),
        Line::from("Shift+L logout → account picker"),
        Line::from("q       quit"),
        Line::from("?       this help"),
        Line::from(""),
        Line::from(Span::styled(
            "welcome: saved accounts at top · Enter logs in (no typing)",
            style_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("Esc close", style_dim())),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" ? ")),
        popup,
    );
}

fn draw_reminder(frame: &mut Frame, area: Rect, message: &str) {
    let lines = vec![
        Line::from(Span::styled("token reminder", style_accent())),
        Line::from(""),
        Line::from(message),
        Line::from(""),
        Line::from("The server cannot resend your token. Keep it in a password manager."),
        Line::from(""),
        Line::from(Span::styled("Enter dismiss", style_ok())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL)),
        area,
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
