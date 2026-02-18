//! Ratatui TUI frontend for the poker client.
//!
//! Pure UI module: terminal lifecycle, rendering, and input → command mapping.
//! All game state lives in [`crate::game_state`] and all networking in
//! [`crate::net_client`]. This module has no networking dependencies.

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::io::{self, Stdout};

use poker_core::game_state::{ClientGameState, GameEvent, LogCategory, RAISE_PRESETS, RaisePreset};
use poker_core::protocol::{CardInfo, ClientMessage, PlayerAction, PlayerInfo};

// ---------------------------------------------------------------------------
// UserIntent — result of processing user input
// ---------------------------------------------------------------------------

/// The result of processing a user input event.
#[derive(Debug)]
pub enum UserIntent {
    /// No action needed (e.g. the event was purely cosmetic).
    None,
    /// The user wants to quit / close the application.
    Quit,
    /// The user wants to send a message to the server.
    Send(ClientMessage),
    /// Local feedback message (e.g. validation error). The event loop should
    /// route this through [`ClientController::add_message`].
    Feedback(String, LogCategory),
}

// ---------------------------------------------------------------------------
// TUI-only state
// ---------------------------------------------------------------------------

/// UI-layer state that lives alongside (but separate from) the game state.
struct TuiState {
    /// Raise amount input buffer
    raise_input: String,
    /// Raise input cursor position
    raise_cursor: usize,
    /// Currently selected control button index
    selected_button: usize,
    /// Selected raise preset (if any)
    selected_raise_preset: Option<RaisePreset>,
    /// True if All-In preset is selected
    pending_all_in: bool,
    /// Show help popup
    show_help: bool,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            raise_input: String::new(),
            raise_cursor: 0,
            selected_button: 0,
            selected_raise_preset: None,
            pending_all_in: false,
            show_help: false,
        }
    }
}

impl TuiState {
    fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.raise_cursor.saturating_sub(1);
        self.raise_cursor = self.clamp_cursor(cursor_moved_left);
    }

    fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.raise_cursor.saturating_add(1);
        self.raise_cursor = self.clamp_cursor(cursor_moved_right);
    }

    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.raise_input.insert(index, new_char);
        self.move_cursor_right();
    }

    fn byte_index(&self) -> usize {
        self.raise_input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.raise_cursor)
            .unwrap_or(self.raise_input.len())
    }

    fn delete_char(&mut self) {
        let is_not_cursor_leftmost = self.raise_cursor != 0;
        if is_not_cursor_leftmost {
            let current_index = self.raise_cursor;
            let from_left_to_current_index = current_index - 1;
            let before_char_to_delete = self.raise_input.chars().take(from_left_to_current_index);
            let after_char_to_delete = self.raise_input.chars().skip(current_index);
            self.raise_input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();
        }
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.raise_input.chars().count())
    }

    fn reset_cursor(&mut self) {
        self.raise_cursor = 0;
    }

    fn set_raise_input(&mut self, amount: u32) {
        self.raise_input = amount.to_string();
        self.raise_cursor = self.raise_input.chars().count();
    }

    fn clear_raise_input(&mut self) {
        self.raise_input.clear();
        self.reset_cursor();
    }

    fn parse_raise_amount(&self) -> Option<u32> {
        let input = self.raise_input.trim();
        if input.is_empty() {
            None
        } else {
            input.parse::<u32>().ok()
        }
    }
}

// ---------------------------------------------------------------------------
// Button model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum ActionButton {
    FoldCheck,
    Call,
    Raise,
    Start,
}

#[derive(Clone, Copy, Debug)]
enum ControlButton {
    Preset(RaisePreset),
    Action(ActionButton),
}

fn action_buttons(gs: &ClientGameState) -> Vec<ActionButton> {
    if !gs.game_started {
        return vec![ActionButton::Start];
    }
    if !gs.is_our_turn {
        return Vec::new();
    }

    let mut buttons = Vec::new();
    if gs.has_action(PlayerAction::Check) || gs.has_action(PlayerAction::Fold) {
        buttons.push(ActionButton::FoldCheck);
    }
    if gs.has_action(PlayerAction::Call) {
        buttons.push(ActionButton::Call);
    }
    if gs.has_action(PlayerAction::Raise) || gs.has_action(PlayerAction::AllIn) {
        buttons.push(ActionButton::Raise);
    }
    buttons
}

fn control_rows(gs: &ClientGameState) -> Vec<Vec<ControlButton>> {
    let mut rows = Vec::new();
    let show_presets = gs.is_our_turn
        && (gs.has_action(PlayerAction::Raise) || gs.has_action(PlayerAction::AllIn));
    if show_presets {
        rows.push(
            RAISE_PRESETS
                .iter()
                .copied()
                .map(ControlButton::Preset)
                .collect(),
        );
    }

    let actions: Vec<ControlButton> = action_buttons(gs)
        .into_iter()
        .map(ControlButton::Action)
        .collect();
    if !actions.is_empty() {
        rows.push(actions);
    }

    rows
}

fn control_button_count(gs: &ClientGameState) -> usize {
    control_rows(gs).iter().map(|row| row.len()).sum()
}

fn control_button_at(gs: &ClientGameState, index: usize) -> ControlButton {
    let mut remaining = index;
    for row in control_rows(gs) {
        if remaining < row.len() {
            return row[remaining];
        }
        remaining = remaining.saturating_sub(row.len());
    }
    ControlButton::Action(ActionButton::Start)
}

fn selected_row_col(tui: &TuiState, rows: &[Vec<ControlButton>]) -> Option<(usize, usize)> {
    if rows.is_empty() {
        return None;
    }

    let mut remaining = tui.selected_button;
    for (row_index, row) in rows.iter().enumerate() {
        if remaining < row.len() {
            return Some((row_index, remaining));
        }
        remaining = remaining.saturating_sub(row.len());
    }

    Some((rows.len() - 1, rows.last()?.len().saturating_sub(1)))
}

fn row_start_index(rows: &[Vec<ControlButton>], row: usize) -> usize {
    rows.iter().take(row).map(|r| r.len()).sum()
}

fn clamp_selected_button(tui: &mut TuiState, gs: &ClientGameState) {
    let max = control_button_count(gs);
    if max == 0 {
        tui.selected_button = 0;
    } else if tui.selected_button >= max {
        tui.selected_button = max - 1;
    }
}

// ---------------------------------------------------------------------------
// Control activation (maps button press → ClientMessage)
// ---------------------------------------------------------------------------

fn select_raise_preset(tui: &mut TuiState, gs: &ClientGameState, preset: RaisePreset) {
    tui.selected_raise_preset = Some(preset);
    tui.pending_all_in = matches!(preset, RaisePreset::AllIn);
    let amount = preset.amount(gs);
    if amount > 0 {
        tui.set_raise_input(amount);
    } else {
        tui.clear_raise_input();
    }
}

fn handle_control_activation(
    tui: &mut TuiState,
    gs: &ClientGameState,
    button: ControlButton,
) -> UserIntent {
    match button {
        ControlButton::Preset(preset) => {
            select_raise_preset(tui, gs, preset);
            UserIntent::None
        }
        ControlButton::Action(ActionButton::Start) => UserIntent::Send(ClientMessage::StartGame),
        ControlButton::Action(ActionButton::FoldCheck) => match gs.fold_or_check() {
            Some(msg) => UserIntent::Send(msg),
            None => UserIntent::None,
        },
        ControlButton::Action(ActionButton::Call) => match gs.call() {
            Some(msg) => UserIntent::Send(msg),
            None => UserIntent::None,
        },
        ControlButton::Action(ActionButton::Raise) => {
            if tui.pending_all_in {
                match gs.raise(0, true) {
                    Ok(msg) => return UserIntent::Send(msg),
                    Err(e) => {
                        return UserIntent::Feedback(e, LogCategory::Error);
                    }
                }
            }
            let amount = match tui.parse_raise_amount() {
                Some(value) => value,
                None => {
                    return UserIntent::Feedback(
                        "Enter a raise amount first".to_string(),
                        LogCategory::Error,
                    );
                }
            };
            match gs.raise(amount, false) {
                Ok(msg) => UserIntent::Send(msg),
                Err(e) => UserIntent::Feedback(e, LogCategory::Error),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API — Tui struct
// ---------------------------------------------------------------------------

/// Owns the ratatui terminal and all UI-layer state.
///
/// The client orchestrator ([`crate::client`]) drives this struct:
/// call [`Tui::render`] each frame, [`Tui::poll_and_handle_input`] to
/// process keyboard events, and [`Tui::on_actions_changed`] when the
/// game state's available actions change.
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: TuiState,
}

impl Tui {
    /// Set up the terminal (raw mode, alternate screen) and return a ready `Tui`.
    pub fn setup() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            state: TuiState::default(),
        })
    }

    /// Restore the terminal to its original state.
    pub fn teardown(&mut self) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Draw the current frame. Automatically clamps the selected button index.
    pub fn render(&mut self, gs: &ClientGameState) -> io::Result<()> {
        clamp_selected_button(&mut self.state, gs);
        self.terminal.draw(|f| ui(f, gs, &self.state))?;
        Ok(())
    }

    /// Poll for a keyboard event and, if one is available, translate it into
    /// a [`UserIntent`]. This never blocks — returns [`UserIntent::None`]
    /// immediately when no event is pending.
    pub fn poll_and_handle_input(&mut self, gs: &ClientGameState) -> io::Result<UserIntent> {
        if !event::poll(std::time::Duration::from_millis(0))? {
            return Ok(UserIntent::None);
        }
        let Event::Key(key) = event::read()? else {
            return Ok(UserIntent::None);
        };
        if key.kind != KeyEventKind::Press {
            return Ok(UserIntent::None);
        }
        Ok(self.handle_key_event(key, gs))
    }

    /// Notify the UI that the set of available game actions changed.
    ///
    /// Re-clamps the selected button and, if a new turn started, resets the
    /// raise input to the minimum raise.
    pub fn on_actions_changed(&mut self, gs: &ClientGameState) {
        clamp_selected_button(&mut self.state, gs);
        if gs.is_our_turn && gs.min_raise > 0 {
            self.state.selected_raise_preset = None;
            self.state.pending_all_in = false;
            self.state.set_raise_input(gs.min_raise);
        }
    }

    // -- private -----------------------------------------------------------

    fn handle_key_event(&mut self, key: KeyEvent, gs: &ClientGameState) -> UserIntent {
        let tui = &mut self.state;
        match key.code {
            KeyCode::Esc => {
                if tui.show_help {
                    tui.show_help = false;
                    UserIntent::None
                } else {
                    UserIntent::Quit
                }
            }
            KeyCode::F(1) => {
                tui.show_help = !tui.show_help;
                UserIntent::None
            }
            KeyCode::Enter => {
                if tui.show_help {
                    return UserIntent::None;
                }
                let total = control_button_count(gs);
                if total > 0 {
                    let button = control_button_at(gs, tui.selected_button);
                    return handle_control_activation(tui, gs, button);
                }
                UserIntent::None
            }
            KeyCode::Char(c) => {
                if !tui.show_help && c.is_ascii_digit() {
                    tui.enter_char(c);
                    tui.selected_raise_preset = None;
                    tui.pending_all_in = false;
                }
                UserIntent::None
            }
            KeyCode::Backspace => {
                if !tui.show_help {
                    tui.delete_char();
                    tui.selected_raise_preset = None;
                    tui.pending_all_in = false;
                }
                UserIntent::None
            }
            KeyCode::Left => {
                if !tui.show_help {
                    let rows = control_rows(gs);
                    if let Some((row, col)) = selected_row_col(tui, &rows) {
                        let row_len = rows[row].len();
                        if row_len > 0 {
                            let new_col = (col + row_len - 1) % row_len;
                            tui.selected_button = row_start_index(&rows, row) + new_col;
                        }
                    }
                }
                UserIntent::None
            }
            KeyCode::Right => {
                if !tui.show_help {
                    let rows = control_rows(gs);
                    if let Some((row, col)) = selected_row_col(tui, &rows) {
                        let row_len = rows[row].len();
                        if row_len > 0 {
                            let new_col = (col + 1) % row_len;
                            tui.selected_button = row_start_index(&rows, row) + new_col;
                        }
                    }
                }
                UserIntent::None
            }
            KeyCode::Up => {
                if !tui.show_help {
                    let rows = control_rows(gs);
                    if rows.len() > 1
                        && let Some((row, col)) = selected_row_col(tui, &rows)
                    {
                        let new_row = row.saturating_sub(1);
                        let new_col = col.min(rows[new_row].len().saturating_sub(1));
                        tui.selected_button = row_start_index(&rows, new_row) + new_col;
                    }
                }
                UserIntent::None
            }
            KeyCode::Down => {
                if !tui.show_help {
                    let rows = control_rows(gs);
                    if rows.len() > 1
                        && let Some((row, col)) = selected_row_col(tui, &rows)
                    {
                        let new_row = (row + 1).min(rows.len() - 1);
                        let new_col = col.min(rows[new_row].len().saturating_sub(1));
                        tui.selected_button = row_start_index(&rows, new_row) + new_col;
                    }
                }
                UserIntent::None
            }
            _ => UserIntent::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn ui(frame: &mut Frame, gs: &ClientGameState, tui: &TuiState) {
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // Main content
            Constraint::Length(5), // Controls
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    // Main content area split horizontally
    let content_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(20), // Players + Info
            Constraint::Min(70),    // Game board + messages
        ])
        .split(main_layout[0]);

    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(65), // Players list
            Constraint::Percentage(35), // Info
        ])
        .split(content_layout[0]);

    // Left panel: Players + Info
    render_players_panel(frame, gs, left_layout[0]);
    render_actions_panel(frame, gs, left_layout[1]);

    // Right panel: Game board and messages
    let middle_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Game board
            Constraint::Min(5),    // Messages
        ])
        .split(content_layout[1]);

    render_game_board(frame, gs, middle_layout[0]);
    render_messages(frame, gs, middle_layout[1]);

    // Controls bar
    let cursor_position = render_controls_bar(frame, gs, tui, main_layout[1]);
    if let Some((cursor_x, cursor_y)) = cursor_position {
        #[allow(clippy::cast_possible_truncation)]
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    // Status bar
    let status_color = if gs.connected {
        Color::Green
    } else {
        Color::Red
    };
    let status_text = if gs.connected {
        "● Connected"
    } else {
        "● Disconnected"
    };
    let mut status_spans = vec![
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::raw(" | "),
        Span::styled("F1", Style::default().fg(Color::Cyan).bold()),
        Span::raw(": Help | "),
        Span::styled("ESC", Style::default().fg(Color::Cyan).bold()),
        Span::raw(": Quit"),
    ];
    if !gs.our_name.is_empty() {
        status_spans.push(Span::raw(" | You: "));
        status_spans.push(Span::styled(
            gs.our_name.as_str(),
            Style::default().fg(Color::Cyan),
        ));
    }
    let status = Paragraph::new(Line::from(status_spans));
    frame.render_widget(status, main_layout[2]);

    // Help popup
    if tui.show_help {
        render_help_popup(frame);
    }
}

fn render_players_panel(frame: &mut Frame, gs: &ClientGameState, area: Rect) {
    let my_id = gs.our_player_id;

    let items: Vec<ListItem> = gs
        .players
        .iter()
        .map(|PlayerInfo { id, name, chips }| {
            let is_me = *id == my_id;
            let is_dealer = *id == gs.dealer_id;

            let mut spans = vec![];
            if is_dealer {
                spans.push(Span::styled("(D) ", Style::default().fg(Color::Yellow)));
            } else {
                spans.push(Span::raw("  "));
            }

            let name_style = if is_me {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default().fg(Color::White)
            };

            spans.push(Span::styled(name.to_string(), name_style));
            spans.push(Span::styled(
                format!(" ${}", chips),
                Style::default().fg(Color::Green),
            ));

            ListItem::new(Line::from(spans))
        })
        .collect();

    let players_list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue))
            .title(" Players ")
            .title_style(Style::default().fg(Color::Blue).bold()),
    );

    frame.render_widget(players_list, area);
}

fn render_game_board(frame: &mut Frame, gs: &ClientGameState, area: Rect) {
    let mut lines = vec![];

    // Stage and pot
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {} ", gs.stage),
            Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
        ),
        Span::raw("  "),
        Span::styled("Pot: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("${}", gs.pot),
            Style::default().fg(Color::Green).bold(),
        ),
        Span::raw("  "),
        Span::styled("Hand: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("#{}", gs.hand_number),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(""));

    // Community cards
    let community_str = if gs.community_cards.is_empty() {
        "[ ? ] [ ? ] [ ? ] [ ? ] [ ? ]".to_string()
    } else {
        let mut cards: Vec<String> = gs.community_cards.iter().map(format_card).collect();
        while cards.len() < 5 {
            cards.push("[ ? ]".to_string());
        }
        cards.join(" ")
    };

    lines.push(Line::from(vec![Span::styled(
        community_str,
        Style::default().fg(Color::White),
    )]));

    lines.push(Line::from(""));

    // Hole cards
    lines.push(Line::from(vec![Span::styled(
        "Your Cards:",
        Style::default().fg(Color::Gray),
    )]));

    let hole_str = if let Some(cards) = &gs.hole_cards {
        format!("{}  {}", format_card(&cards[0]), format_card(&cards[1]))
    } else {
        "[???]  [???]".to_string()
    };

    lines.push(Line::from(vec![Span::styled(
        hole_str,
        Style::default().fg(Color::Cyan).bold(),
    )]));

    let board = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta))
            .title(" Table ")
            .title_style(Style::default().fg(Color::Magenta).bold()),
    );

    frame.render_widget(board, area);
}

fn format_card(card: &CardInfo) -> String {
    format!("[ {}  {}  ]", card.rank_str(), card.suit_str())
}

/// Format a structured [`GameEvent`] into a human-readable string for the TUI log.
fn format_event(event: &GameEvent) -> String {
    match event {
        GameEvent::Welcome { message } => format!("🎰 {}", message),
        GameEvent::Joined {
            player_id,
            chips,
            player_count,
        } => format!(
            "✅ Joined! ID: {} | Chips: ${} | Players: {}",
            player_id, chips, player_count
        ),
        GameEvent::PlayerJoined { player_id, name } => {
            format!("👤 {} (#{}) joined", name, player_id)
        }
        GameEvent::PlayerLeft { name, .. } => format!("👋 {} left", name),
        GameEvent::Chat {
            player_name,
            message,
            ..
        } => format!("💬 {}: {}", player_name, message),
        GameEvent::GameStarted => "🎮 Game has started!".to_string(),
        GameEvent::NewHand {
            hand_number,
            dealer_id,
            small_blind_id,
            big_blind_id,
            small_blind,
            big_blind,
        } => format!(
            "🃏 Hand #{} | D:{} SB:{}(${}) BB:{}(${})",
            hand_number, dealer_id, small_blind_id, small_blind, big_blind_id, big_blind
        ),
        GameEvent::HoleCards { cards } => {
            format!("🎴 Your cards: {} {}", cards[0], cards[1])
        }
        GameEvent::CommunityCards { stage, cards } => {
            let cards_str: Vec<String> = cards.iter().map(|c| c.to_string()).collect();
            format!("🃏 {}: {}", stage.to_uppercase(), cards_str.join(" "))
        }
        GameEvent::YourTurn => "🎯 YOUR TURN!".to_string(),
        GameEvent::PlayerActed {
            name,
            action,
            amount,
            ..
        } => match amount {
            Some(amt) => format!("🎬 {} {}s ${}", name, action, amt),
            None => format!("🎬 {} {}s", name, action),
        },
        GameEvent::Showdown { hands } => {
            let mut lines = vec!["🎭 SHOWDOWN".to_string()];
            for (_player_id, name, cards, rank) in hands {
                lines.push(format!("   {}: {} {} - {}", name, cards[0], cards[1], rank));
            }
            lines.join("\n")
        }
        GameEvent::AllInShowdown { hands } => {
            let mut lines = vec!["🔥 ALL-IN SHOWDOWN! 🔥".to_string()];
            for (_player_id, name, cards, equity) in hands {
                lines.push(format!(
                    "   {}: {} {} → {:.1}%",
                    name, cards[0], cards[1], equity
                ));
            }
            lines.join("\n")
        }
        GameEvent::RoundWinner {
            name, amount, hand, ..
        } => format!("🏆 {} wins ${} with {}", name, amount, hand),
        GameEvent::PlayerEliminated { name, .. } => {
            format!("💀 {} eliminated!", name)
        }
        GameEvent::GameOver {
            winner_id,
            winner_name,
        } => format!("🎊 GAME OVER! {} (#{}) WINS! 🎊", winner_name, winner_id),
        GameEvent::Pong => "🏓 Pong!".to_string(),
        GameEvent::ServerError { message } => format!("❌ {}", message),
        GameEvent::Disconnected => "❌ Server disconnected".to_string(),
        GameEvent::ConnectionError { message } => format!("❌ Connection error: {}", message),
        GameEvent::Unknown { raw } => format!("📨 {}", raw),
        GameEvent::Text { text, .. } => text.clone(),
        GameEvent::BlindsIncreased {
            small_blind,
            big_blind,
        } => format!("📈 Blinds increased to {}/{}", small_blind, big_blind),
        GameEvent::TurnTimerStarted {
            name, timeout_secs, ..
        } => {
            format!("⏱ {} has {}s to act", name, timeout_secs)
        }
        GameEvent::PlayerSatOut { name, .. } => {
            format!("💤 {} is sitting out", name)
        }
        GameEvent::PlayerSatIn { name, .. } => {
            format!("✅ {} is back in", name)
        }
    }
}

fn render_messages(frame: &mut Frame, gs: &ClientGameState, area: Rect) {
    let messages: Vec<ListItem> = gs
        .events
        .iter()
        .rev()
        .take(area.height.saturating_sub(2) as usize)
        .rev()
        .map(|ev| {
            let style = match ev.category() {
                LogCategory::System => Style::default().fg(Color::Yellow),
                LogCategory::Chat => Style::default().fg(Color::Cyan),
                LogCategory::Action => Style::default().fg(Color::White),
                LogCategory::Winner => Style::default().fg(Color::Green).bold(),
                LogCategory::Error => Style::default().fg(Color::Red),
                LogCategory::Info => Style::default().fg(Color::Gray),
            };
            ListItem::new(Span::styled(format_event(ev), style))
        })
        .collect();

    let messages_list = List::new(messages).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Log ")
            .title_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(messages_list, area);
}

fn render_actions_panel(frame: &mut Frame, gs: &ClientGameState, area: Rect) {
    let mut lines = vec![];

    if gs.is_our_turn {
        lines.push(Line::from(vec![Span::styled(
            " 🎯 YOUR TURN!",
            Style::default().fg(Color::Yellow).bold().rapid_blink(),
        )]));

        lines.push(Line::from(""));

        let to_call = gs.current_bet.saturating_sub(gs.our_bet);
        if to_call > 0 {
            lines.push(Line::from(vec![
                Span::styled(" To call: ", Style::default().fg(Color::Gray)),
                Span::styled(format!("${}", to_call), Style::default().fg(Color::Red)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled(" Min raise: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("${}", gs.min_raise),
                Style::default().fg(Color::White),
            ),
        ]));
    } else if !gs.game_started {
        lines.push(Line::from(vec![Span::styled(
            " Waiting for game",
            Style::default().fg(Color::Gray),
        )]));
        lines.push(Line::from(vec![Span::styled(
            " to start...",
            Style::default().fg(Color::Gray),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " Select Start",
            Style::default().fg(Color::DarkGray),
        )]));
        lines.push(Line::from(vec![Span::styled(
            " ready!",
            Style::default().fg(Color::DarkGray),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            " Waiting for",
            Style::default().fg(Color::Gray),
        )]));
        lines.push(Line::from(vec![Span::styled(
            " other players...",
            Style::default().fg(Color::Gray),
        )]));
    }

    let actions = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Info ")
            .title_style(Style::default().fg(Color::Yellow).bold()),
    );

    frame.render_widget(actions, area);
}

fn action_label(gs: &ClientGameState, action: ActionButton) -> String {
    match action {
        ActionButton::FoldCheck => {
            if gs.has_action(PlayerAction::Check) {
                "Check".to_string()
            } else {
                "Fold".to_string()
            }
        }
        ActionButton::Call => "Call".to_string(),
        ActionButton::Raise => "Raise".to_string(),
        ActionButton::Start => "Start".to_string(),
    }
}

fn control_button_enabled(gs: &ClientGameState, button: ControlButton) -> bool {
    match button {
        ControlButton::Preset(_) => {
            gs.is_our_turn
                && (gs.has_action(PlayerAction::Raise) || gs.has_action(PlayerAction::AllIn))
        }
        ControlButton::Action(ActionButton::Start) => !gs.game_started,
        ControlButton::Action(ActionButton::FoldCheck) => {
            gs.is_our_turn
                && (gs.has_action(PlayerAction::Check) || gs.has_action(PlayerAction::Fold))
        }
        ControlButton::Action(ActionButton::Call) => {
            gs.is_our_turn && gs.has_action(PlayerAction::Call)
        }
        ControlButton::Action(ActionButton::Raise) => {
            gs.is_our_turn
                && (gs.has_action(PlayerAction::Raise) || gs.has_action(PlayerAction::AllIn))
        }
    }
}

fn render_controls_row(
    frame: &mut Frame,
    gs: &ClientGameState,
    tui: &TuiState,
    area: Rect,
    buttons: &[ControlButton],
    selected_offset: usize,
) -> Option<(u16, u16)> {
    let mut spans = Vec::with_capacity(buttons.len() * 2 + 1);
    let mut row_width = 0usize;
    let mut cursor_offset: Option<usize> = None;

    for (index, button) in buttons.iter().enumerate() {
        if index > 0 {
            row_width = row_width.saturating_add(1);
        }

        let selected_index = selected_offset + index;
        let is_selected = tui.selected_button == selected_index;
        let enabled = control_button_enabled(gs, *button);
        let mut style = if enabled {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let (label, input_offset) = match button {
            ControlButton::Preset(preset) => (preset.label().to_string(), None),
            ControlButton::Action(ActionButton::Raise) => {
                let input_value = if tui.raise_input.is_empty() {
                    " "
                } else {
                    tui.raise_input.as_str()
                };
                let label_core = format!(
                    "{} [{}]",
                    action_label(gs, ActionButton::Raise),
                    input_value
                );
                let input_start = label_core.find('[').unwrap_or(0) + 1;
                (label_core, Some(input_start))
            }
            ControlButton::Action(action) => (action_label(gs, *action), None),
        };

        let preset_selected = match button {
            ControlButton::Preset(preset) => tui.selected_raise_preset == Some(*preset),
            _ => false,
        };

        if preset_selected {
            style = style.fg(Color::Green).bold();
        }

        if is_selected {
            style = style.bg(Color::Blue).fg(Color::Black).bold();
        }

        let label_with_padding = format!(" {} ", label);
        if let Some(input_start) = input_offset
            && gs.is_our_turn
        {
            let cursor_pos = row_width.saturating_add(1 + input_start)
                + tui.raise_cursor.min(tui.raise_input.chars().count());
            cursor_offset = Some(cursor_pos);
        }

        row_width = row_width.saturating_add(label_with_padding.len());

        spans.push(Span::styled(label_with_padding, style));
        if index + 1 < buttons.len() {
            spans.push(Span::raw(" "));
        }
    }

    let pad = if area.width as usize > row_width {
        (area.width as usize - row_width) / 2
    } else {
        0
    };
    if pad > 0 {
        spans.insert(0, Span::raw(" ".repeat(pad)));
    }

    let row = Paragraph::new(Line::from(spans));
    frame.render_widget(row, area);

    cursor_offset.and_then(|offset| {
        let cursor_x = area.x.saturating_add((pad + offset) as u16);
        if cursor_x >= area.x + area.width {
            None
        } else {
            Some((cursor_x, area.y))
        }
    })
}

fn render_controls_bar(
    frame: &mut Frame,
    gs: &ClientGameState,
    tui: &TuiState,
    area: Rect,
) -> Option<(u16, u16)> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(" Controls ")
        .title_style(Style::default().fg(Color::Blue).bold());
    frame.render_widget(&block, area);

    let inner = block.inner(area);
    let rows_data = control_rows(gs);
    if rows_data.is_empty() {
        return None;
    }

    if inner.height < rows_data.len() as u16 {
        return None;
    }

    let row_constraints: Vec<Constraint> = (0..rows_data.len())
        .map(|_| Constraint::Length(1))
        .collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(inner);

    let mut cursor_position = None;
    let mut selected_offset = 0usize;
    for (row_index, row_buttons) in rows_data.iter().enumerate() {
        let row_cursor = render_controls_row(
            frame,
            gs,
            tui,
            rows[row_index],
            row_buttons,
            selected_offset,
        );
        if cursor_position.is_none() {
            cursor_position = row_cursor;
        }
        selected_offset = selected_offset.saturating_add(row_buttons.len());
    }

    cursor_position
}

fn render_help_popup(frame: &mut Frame) {
    let area = centered_rect(60, 80, frame.area());

    frame.render_widget(Clear, area);

    let help_text = Text::from(vec![
        Line::from(vec![Span::styled(
            "CONTROLS",
            Style::default().fg(Color::Yellow).bold(),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  NAVIGATION",
            Style::default().fg(Color::Cyan).bold(),
        )]),
        Line::from("  Left/Right    Move within row"),
        Line::from("  Up/Down       Move between rows"),
        Line::from("  Enter         Activate selected button"),
        Line::from("  0-9           Type raise amount"),
        Line::from("  Backspace     Edit raise amount"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  BUTTONS",
            Style::default().fg(Color::Cyan).bold(),
        )]),
        Line::from("  Presets       35%, 50%, 75%, 100%, All-In"),
        Line::from("  Actions       Fold/Check, Call, Raise [amount]"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  SYSTEM",
            Style::default().fg(Color::Cyan).bold(),
        )]),
        Line::from("  F1            Toggle this help"),
        Line::from("  ESC           Quit"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press ESC or F1 to close",
            Style::default().fg(Color::DarkGray),
        )]),
    ]);

    let help = Paragraph::new(help_text).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Help ")
            .title_style(Style::default().fg(Color::Cyan).bold())
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(help, area);
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
