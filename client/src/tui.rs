use crate::transfer::{self, Config, ProgressEvent};
use crossterm::event::{self, KeyCode, KeyEvent};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Modifier},
    text::{Line, Span},
    widgets::{Block, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState},
    Frame, Terminal,
};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Connecting,
    Ready,
    Uploading,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Queued,
    Uploading,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct WorkerState {
    pub chunk_index: Option<u64>,
    pub percent: u8,
    pub done: bool,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub status: FileStatus,
    pub progress_percent: u8,
    pub bytes_uploaded: u64,
    pub total_chunks: u64,
    pub upload_id: Option<String>,
    pub workers: Vec<WorkerState>,
    pub error_message: Option<String>,
}

pub enum TuiEvent {
    ConnectionSuccess,
    ConnectionFailed(String),
    Progress(ProgressEvent),
    Input(KeyEvent),
    Tick,
}

pub struct Theme {
    pub bg_base: Color,
    pub bg_surface: Color,
    pub bg_overlay: Color,
    pub bg_selection: Color,
    pub fg_default: Color,
    pub fg_muted: Color,
    pub fg_emphasis: Color,
    pub accent_primary: Color,
    pub accent_secondary: Color,
    pub status_success: Color,
    pub status_error: Color,
    pub status_info: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg_base: Color::Rgb(0x1C, 0x1A, 0x18),
            bg_surface: Color::Rgb(0x25, 0x22, 0x20),
            bg_overlay: Color::Rgb(0x2E, 0x2A, 0x26),
            bg_selection: Color::Rgb(0x3D, 0x2A, 0x18),
            fg_default: Color::Rgb(0xE8, 0xDD, 0xD0),
            fg_muted: Color::Rgb(0x7A, 0x6E, 0x64),
            fg_emphasis: Color::Rgb(0xF5, 0xED, 0xE3),
            accent_primary: Color::Rgb(0xE8, 0x73, 0x2A),
            accent_secondary: Color::Rgb(0xD4, 0xA8, 0x43),
            status_success: Color::Rgb(0x6A, 0x8F, 0x3D),
            status_error: Color::Rgb(0xCC, 0x33, 0x33),
            status_info: Color::Rgb(0x5E, 0x9E, 0x8F),
        }
    }
}

pub struct TuiApp {
    pub config: Config,
    pub state: AppState,
    pub queue: Vec<FileEntry>,
    pub selected_idx: usize,
    pub queue_state: TableState,
    pub show_workers: bool,
    pub worker_focused: bool,
    pub show_help: bool,
    pub show_add_popup: bool,
    pub show_quit_confirm: bool,
    pub add_input_path: String,
    pub spinner_frame: usize,
    pub toast: Option<(String, Instant)>,
    pub theme: Theme,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        );
    }
}

pub fn run(config: Config, initial_toast: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, event_rx) = mpsc::channel();
    let (progress_tx, progress_rx) = mpsc::channel();

    // Spawn tick thread
    let tx = event_tx.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(80));
        if tx.send(TuiEvent::Tick).is_err() {
            break;
        }
    });

    // Spawn crossterm input reader thread
    let tx = event_tx.clone();
    thread::spawn(move || loop {
        if event::poll(Duration::from_millis(50)).unwrap() {
            if let event::Event::Key(key) = event::read().unwrap() {
                if tx.send(TuiEvent::Input(key)).is_err() {
                    break;
                }
            }
        }
    });

    // Spawn progress event forwarder thread
    let tx = event_tx.clone();
    thread::spawn(move || {
        while let Ok(pe) = progress_rx.recv() {
            if tx.send(TuiEvent::Progress(pe)).is_err() {
                break;
            }
        }
    });

    // Run connection test
    let config_clone = config.clone();
    let tx = event_tx.clone();
    thread::spawn(move || {
        let server_addr = format!("{}:{}", config_clone.host, config_clone.port);
        match transfer::connect_and_auth(&server_addr, &config_clone.secret) {
            Ok(_) => {
                let _ = tx.send(TuiEvent::ConnectionSuccess);
            }
            Err(e) => {
                let _ = tx.send(TuiEvent::ConnectionFailed(e.to_string()));
            }
        }
    });

    let mut app = TuiApp {
        config,
        state: AppState::Connecting,
        queue: vec![],
        selected_idx: 0,
        queue_state: TableState::default(),
        show_workers: false,
        worker_focused: false,
        show_help: false,
        show_add_popup: false,
        show_quit_confirm: false,
        add_input_path: String::new(),
        spinner_frame: 0,
        toast: initial_toast.map(|t| (t, Instant::now())),
        theme: Theme::default(),
    };

    loop {
        // Clear expired toast messages
        if let Some((_, inst)) = &app.toast {
            if inst.elapsed() >= Duration::from_secs(3) {
                app.toast = None;
            }
        }

        // Draw TUI
        terminal.draw(|f| draw_ui(f, &mut app))?;

        // Handle events
        match event_rx.recv()? {
            TuiEvent::Tick => {
                app.spinner_frame = app.spinner_frame.wrapping_add(1);
            }
            TuiEvent::Input(key) => {
                if handle_input(&mut app, key, &progress_tx)? {
                    break;
                }
            }
            TuiEvent::ConnectionSuccess => {
                app.state = AppState::Ready;
                check_start_next_upload(&mut app, &progress_tx);
            }
            TuiEvent::ConnectionFailed(e) => {
                app.state = AppState::Error(e);
            }
            TuiEvent::Progress(pe) => {
                handle_progress(&mut app, pe, &progress_tx);
            }
        }
    }

    Ok(())
}

fn draw_ui(frame: &mut Frame, app: &mut TuiApp) {
    let area = frame.area();
    let theme = &app.theme;

    // Minimum size gate
    if area.width < 80 || area.height < 24 {
        let msg = "Terminal too small. Please resize.";
        let paragraph = Paragraph::new(msg)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(theme.status_error).add_modifier(Modifier::BOLD));

        let chunks = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ]).split(area);

        frame.render_widget(Block::default().style(Style::default().bg(theme.bg_base)), area);
        frame.render_widget(paragraph, chunks[1]);
        return;
    }

    // Set background for the entire screen
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg_base)), area);

    if let AppState::Connecting = app.state {
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spinner_char = spinner[app.spinner_frame % spinner.len()];
        let msg = format!("{} Connecting to {}:{}...", spinner_char, app.config.host, app.config.port);
        let paragraph = Paragraph::new(msg)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(theme.accent_primary).add_modifier(Modifier::BOLD));

        let chunks = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ]).split(area);

        frame.render_widget(paragraph, chunks[1]);
        return;
    }

    if let AppState::Error(ref err_msg) = app.state {
        let error_text = vec![
            Line::from(vec![Span::styled("Fatal Connection Error", Style::default().fg(theme.status_error).add_modifier(Modifier::BOLD))]),
            Line::from(""),
            Line::from(format!("Failed to connect to the server: {}", err_msg)),
            Line::from(""),
            Line::from(vec![Span::styled("Please verify that your configuration is correct:", Style::default().fg(theme.fg_muted))]),
            Line::from(format!("  Host: {}", app.config.host)),
            Line::from(format!("  Port: {}", app.config.port)),
            Line::from(""),
            Line::from(vec![Span::styled("Press 'q' to exit the application.", Style::default().fg(theme.accent_primary))]),
        ];
        let paragraph = Paragraph::new(error_text)
            .block(Block::bordered().border_style(Style::default().fg(theme.status_error)))
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(theme.fg_default));

        let chunks = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(11),
            Constraint::Fill(1),
        ]).split(area);
        let middle_layout = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(60),
            Constraint::Fill(1),
        ]).split(chunks[1]);

        frame.render_widget(paragraph, middle_layout[1]);
        return;
    }

    // Normal Layout
    let vertical_chunks = Layout::vertical([
        Constraint::Length(3), // Header
        Constraint::Fill(1),   // Queue + optional Worker panel
        Constraint::Length(1), // Footer
    ]).split(area);

    // 1. Render Header
    let title_line = Line::from(vec![
        Span::styled(" ParaFlow ", Style::default().fg(theme.fg_emphasis).add_modifier(Modifier::BOLD)),
        Span::styled("◉", Style::default().fg(theme.status_info)),
        Span::styled(format!(" Connected: {}:{} ", app.config.host, app.config.port), Style::default().fg(theme.fg_emphasis)),
    ]);

    let header_content = if let Some((msg, _)) = &app.toast {
        Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(theme.fg_muted)),
            Span::styled(msg, Style::default().fg(theme.accent_primary).add_modifier(Modifier::BOLD)),
        ])
    } else {
        make_stats_line(app, theme)
    };

    let header_block = Block::bordered()
        .title(title_line)
        .border_style(Style::default().fg(theme.fg_muted))
        .style(Style::default().bg(theme.bg_base));
    let header_p = Paragraph::new(header_content).block(header_block);
    frame.render_widget(header_p, vertical_chunks[0]);

    // 2. Render Main Body (Queue + optional Workers)
    let main_chunks = if app.show_workers {
        Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(2 + app.config.threads as u16),
        ]).split(vertical_chunks[1])
    } else {
        Layout::vertical([Constraint::Fill(1)]).split(vertical_chunks[1])
    };

    // Render Queue Table
    let table_block = Block::bordered()
        .title(" Queue ")
        .border_style(if app.show_add_popup || app.show_help || app.show_quit_confirm {
            Style::default().fg(theme.fg_muted)
        } else if app.worker_focused {
            Style::default().fg(theme.fg_muted)
        } else {
            Style::default().fg(theme.accent_primary)
        })
        .title_style(Style::default().fg(theme.fg_emphasis))
        .style(Style::default().bg(theme.bg_surface));

    let mut rows = vec![];
    for (idx, entry) in app.queue.iter().enumerate() {
        let is_selected = idx == app.selected_idx;
        let selector = if is_selected { "▶ " } else { "  " };

        let filename_span = if is_selected {
            Span::styled(format!("{}{}", selector, entry.name), Style::default().fg(theme.accent_primary).add_modifier(Modifier::BOLD))
        } else {
            Span::styled(format!("{}{}", selector, entry.name), Style::default().fg(theme.fg_default))
        };

        let size_str = format_size(entry.size);
        let size_span = Span::styled(size_str, Style::default().fg(theme.fg_muted));

        let status_span = match entry.status {
            FileStatus::Queued => Span::styled("Queued", Style::default().fg(theme.fg_muted)),
            FileStatus::Uploading => Span::styled("Uploading", Style::default().fg(theme.accent_primary).add_modifier(Modifier::BOLD)),
            FileStatus::Done => Span::styled("Done ✓", Style::default().fg(theme.status_success).add_modifier(Modifier::BOLD)),
            FileStatus::Failed => Span::styled("Failed", Style::default().fg(theme.status_error).add_modifier(Modifier::BOLD)),
        };

        let progress_bar = make_progress_bar(entry.progress_percent, &entry.status, theme);

        let row_style = if is_selected {
            Style::default().bg(theme.bg_selection)
        } else {
            Style::default()
        };

        rows.push(Row::new(vec![
            Cell::from(Line::from(filename_span)),
            Cell::from(Line::from(size_span)),
            Cell::from(Line::from(status_span)),
            Cell::from(progress_bar),
        ]).style(row_style));
    }

    let widths = [
        Constraint::Fill(1),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(25),
    ];
    let queue_table = Table::new(rows, widths).block(table_block);
    frame.render_stateful_widget(queue_table, main_chunks[0], &mut app.queue_state);

    // Render Worker panel
    if app.show_workers {
        if let Some(entry) = app.queue.get(app.selected_idx) {
            let worker_block = Block::bordered()
                .title(format!(" Workers — {} ", entry.name))
                .border_style(if app.worker_focused {
                    Style::default().fg(theme.accent_primary)
                } else {
                    Style::default().fg(theme.fg_muted)
                })
                .title_style(Style::default().fg(theme.fg_emphasis))
                .style(Style::default().bg(theme.bg_overlay));

            let mut worker_items = vec![];
            for (w_id, worker) in entry.workers.iter().enumerate() {
                let chunk_str = match worker.chunk_index {
                    Some(idx) => format!("chunk #{}", idx),
                    None => "Idle".to_string(),
                };

                let progress_bar = make_worker_progress_bar(worker.percent, worker.done, theme);

                let mut spans = vec![
                    Span::styled(format!("  W{}  ", w_id), Style::default().fg(theme.fg_emphasis)),
                    Span::styled(format!("{:<12}  ", chunk_str), Style::default().fg(theme.fg_muted)),
                ];
                spans.extend(progress_bar.spans);
                worker_items.push(ListItem::new(Line::from(spans)));
            }

            let worker_list = List::new(worker_items).block(worker_block);
            frame.render_widget(worker_list, main_chunks[1]);
        }
    }

    // 3. Render Footer
    let mut footer_spans = vec![];
    if app.show_add_popup {
        footer_spans.push(Span::styled(" [Esc]", Style::default().fg(theme.accent_primary)));
        footer_spans.push(Span::styled("dismiss  ", Style::default().fg(theme.fg_muted)));
        footer_spans.push(Span::styled("[Enter]", Style::default().fg(theme.accent_primary)));
        footer_spans.push(Span::styled("confirm path", Style::default().fg(theme.fg_muted)));
    } else if app.show_help {
        footer_spans.push(Span::styled(" [Esc]", Style::default().fg(theme.accent_primary)));
        footer_spans.push(Span::styled("close help", Style::default().fg(theme.fg_muted)));
    } else if app.show_quit_confirm {
        footer_spans.push(Span::styled(" Upload in progress. Quit? ", Style::default().fg(theme.status_error).add_modifier(Modifier::BOLD)));
        footer_spans.push(Span::styled("[y]", Style::default().fg(theme.status_success).add_modifier(Modifier::BOLD)));
        footer_spans.push(Span::styled("es / ", Style::default().fg(theme.fg_default)));
        footer_spans.push(Span::styled("[N]", Style::default().fg(theme.status_error).add_modifier(Modifier::BOLD)));
        footer_spans.push(Span::styled("o", Style::default().fg(theme.fg_default)));
    } else {
        footer_spans.push(Span::styled(" [a]", Style::default().fg(theme.accent_primary)));
        footer_spans.push(Span::styled("dd  ", Style::default().fg(theme.fg_muted)));

        let is_uploading = app.queue.get(app.selected_idx).map(|f| f.status == FileStatus::Uploading).unwrap_or(false);
        if is_uploading {
            footer_spans.push(Span::styled("[Enter]", Style::default().fg(theme.accent_primary)));
            if app.show_workers {
                footer_spans.push(Span::styled("hide workers  ", Style::default().fg(theme.fg_muted)));
            } else {
                footer_spans.push(Span::styled("workers  ", Style::default().fg(theme.fg_muted)));
            }
        }

        footer_spans.push(Span::styled("[q]", Style::default().fg(theme.accent_primary)));
        footer_spans.push(Span::styled("uit  ", Style::default().fg(theme.fg_muted)));
        footer_spans.push(Span::styled("[?]", Style::default().fg(theme.accent_primary)));
        footer_spans.push(Span::styled("help", Style::default().fg(theme.fg_muted)));
    }

    let footer_p = Paragraph::new(Line::from(footer_spans)).style(Style::default().bg(theme.bg_base));
    frame.render_widget(footer_p, vertical_chunks[2]);

    // 4. Render Popups if active
    if app.show_add_popup {
        let popup_area = get_popup_rect(60, 15, area);
        let input_text = format!("{}█", app.add_input_path);
        let input_paragraph = Paragraph::new(input_text)
            .block(Block::bordered()
                .title(" Add File Path ")
                .border_style(Style::default().fg(theme.accent_primary))
                .title_style(Style::default().fg(theme.fg_emphasis))
            )
            .style(Style::default().fg(theme.fg_default).bg(theme.bg_overlay));
        frame.render_widget(Clear, popup_area);
        frame.render_widget(input_paragraph, popup_area);
    } else if app.show_help {
        let popup_area = get_popup_rect(60, 50, area);
        let help_text = vec![
            Line::from(vec![Span::styled("ParaFlow TUI Keybindings", Style::default().fg(theme.fg_emphasis).add_modifier(Modifier::BOLD))]),
            Line::from(""),
            Line::from(vec![Span::styled("  a       ", Style::default().fg(theme.accent_primary)), Span::raw("Add file to queue")]),
            Line::from(vec![Span::styled("  Enter   ", Style::default().fg(theme.accent_primary)), Span::raw("Toggle workers panel (only when uploading)")]),
            Line::from(vec![Span::styled("  Esc     ", Style::default().fg(theme.accent_primary)), Span::raw("Dismiss popup or collapse workers panel")]),
            Line::from(vec![Span::styled("  j / ↓   ", Style::default().fg(theme.accent_primary)), Span::raw("Select next file")]),
            Line::from(vec![Span::styled("  k / ↑   ", Style::default().fg(theme.accent_primary)), Span::raw("Select previous file")]),
            Line::from(vec![Span::styled("  q       ", Style::default().fg(theme.accent_primary)), Span::raw("Quit (with confirmation if uploading)")]),
            Line::from(vec![Span::styled("  ?       ", Style::default().fg(theme.accent_primary)), Span::raw("Toggle help overlay")]),
            Line::from(""),
            Line::from(vec![Span::styled("Press Esc to close", Style::default().fg(theme.fg_muted).add_modifier(Modifier::ITALIC))]),
        ];
        let help_paragraph = Paragraph::new(help_text)
            .block(Block::bordered()
                .title(" Help ")
                .border_style(Style::default().fg(theme.accent_primary))
                .title_style(Style::default().fg(theme.fg_emphasis))
            )
            .style(Style::default().fg(theme.fg_default).bg(theme.bg_overlay));
        frame.render_widget(Clear, popup_area);
        frame.render_widget(help_paragraph, popup_area);
    }
}

fn make_stats_line(app: &TuiApp, theme: &Theme) -> Line<'static> {
    let queue_count = app.queue.iter().filter(|f| f.status == FileStatus::Queued).count();
    let uploading_count = app.queue.iter().filter(|f| f.status == FileStatus::Uploading).count();
    let done_count = app.queue.iter().filter(|f| f.status == FileStatus::Done).count();

    let total_bytes: u64 = app.queue.iter().map(|f| f.size).sum();
    let total_size_str = format_size(total_bytes);

    Line::from(vec![
        Span::styled("  Queue: ", Style::default().fg(theme.fg_muted)),
        Span::styled(format!("{} ", queue_count), Style::default().fg(theme.fg_emphasis).add_modifier(Modifier::BOLD)),
        Span::styled(" Uploading: ", Style::default().fg(theme.fg_muted)),
        Span::styled(format!("{} ", uploading_count), Style::default().fg(theme.fg_emphasis).add_modifier(Modifier::BOLD)),
        Span::styled(" Done: ", Style::default().fg(theme.fg_muted)),
        Span::styled(format!("{} ", done_count), Style::default().fg(theme.fg_emphasis).add_modifier(Modifier::BOLD)),
        Span::styled(" Total: ", Style::default().fg(theme.fg_muted)),
        Span::styled(total_size_str, Style::default().fg(theme.fg_emphasis).add_modifier(Modifier::BOLD)),
    ])
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn make_progress_bar(percent: u8, status: &FileStatus, theme: &Theme) -> Line<'static> {
    let width = 10;
    let filled = ((percent as usize * width) / 100).min(width);
    let empty = width - filled;

    let bar_color = match status {
        FileStatus::Uploading => theme.accent_primary,
        FileStatus::Done => theme.status_success,
        FileStatus::Failed => theme.status_error,
        FileStatus::Queued => theme.fg_muted,
    };

    Line::from(vec![
        Span::styled("[", Style::default().fg(theme.fg_muted)),
        Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
        Span::styled("░".repeat(empty), Style::default().fg(theme.fg_muted)),
        Span::styled("] ", Style::default().fg(theme.fg_muted)),
        Span::styled(format!("{:>3}%", percent), Style::default().fg(theme.fg_default)),
    ])
}

fn make_worker_progress_bar(percent: u8, done: bool, theme: &Theme) -> Line<'static> {
    if done {
        Line::from(vec![
            Span::styled("Done ✓", Style::default().fg(theme.status_success).add_modifier(Modifier::BOLD))
        ])
    } else {
        let width = 10;
        let filled = ((percent as usize * width) / 100).min(width);
        let empty = width - filled;
        Line::from(vec![
            Span::styled("[", Style::default().fg(theme.fg_muted)),
            Span::styled("█".repeat(filled), Style::default().fg(theme.accent_secondary)),
            Span::styled("░".repeat(empty), Style::default().fg(theme.fg_muted)),
            Span::styled("] ", Style::default().fg(theme.fg_muted)),
            Span::styled(format!("{:>3}%", percent), Style::default().fg(theme.fg_default)),
        ])
    }
}

fn get_popup_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ]).split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ]).split(popup_layout[1])[1]
}

fn handle_input(
    app: &mut TuiApp,
    key: KeyEvent,
    progress_tx: &Sender<ProgressEvent>,
) -> Result<bool, Box<dyn std::error::Error>> {
    // If quit confirmation is open
    if app.show_quit_confirm {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.show_quit_confirm = false;
            }
            _ => {}
        }
        return Ok(false);
    }

    // If help overlay is open
    if app.show_help {
        if key.code == KeyCode::Esc || key.code == KeyCode::Char('?') {
            app.show_help = false;
        }
        return Ok(false);
    }

    // If add popup is open
    if app.show_add_popup {
        match key.code {
            KeyCode::Enter => {
                let path_str = app.add_input_path.trim().to_string();
                app.show_add_popup = false;
                if !path_str.is_empty() {
                    let path = PathBuf::from(path_str);
                    if path.exists() {
                        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        match std::fs::metadata(&path) {
                            Ok(meta) => {
                                let file_entry = FileEntry {
                                    path,
                                    name,
                                    size: meta.len(),
                                    status: FileStatus::Queued,
                                    progress_percent: 0,
                                    bytes_uploaded: 0,
                                    total_chunks: 0,
                                    upload_id: None,
                                    workers: vec![],
                                    error_message: None,
                                };
                                app.queue.push(file_entry);

                                if app.queue.len() == 1 {
                                    app.selected_idx = 0;
                                    app.queue_state.select(Some(0));
                                }

                                check_start_next_upload(app, progress_tx);
                            }
                            Err(e) => {
                                app.toast = Some((format!("Failed to read metadata: {}", e), Instant::now()));
                            }
                        }
                    } else {
                        app.toast = Some((format!("File not found: {}", app.add_input_path), Instant::now()));
                    }
                }
            }
            KeyCode::Esc => {
                app.show_add_popup = false;
            }
            KeyCode::Backspace => {
                app.add_input_path.pop();
            }
            KeyCode::Char(c) => {
                app.add_input_path.push(c);
            }
            _ => {}
        }
        return Ok(false);
    }

    // Normal navigation & controls
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.queue.is_empty() && app.selected_idx + 1 < app.queue.len() {
                app.selected_idx += 1;
                app.queue_state.select(Some(app.selected_idx));
                let is_uploading = app.queue[app.selected_idx].status == FileStatus::Uploading;
                if !is_uploading {
                    app.show_workers = false;
                    app.worker_focused = false;
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !app.queue.is_empty() && app.selected_idx > 0 {
                app.selected_idx -= 1;
                app.queue_state.select(Some(app.selected_idx));
                let is_uploading = app.queue[app.selected_idx].status == FileStatus::Uploading;
                if !is_uploading {
                    app.show_workers = false;
                    app.worker_focused = false;
                }
            }
        }
        KeyCode::Enter => {
            if let Some(entry) = app.queue.get(app.selected_idx) {
                if entry.status == FileStatus::Uploading {
                    app.show_workers = !app.show_workers;
                    app.worker_focused = app.show_workers;
                }
            }
        }
        KeyCode::Char('a') => {
            app.show_add_popup = true;
            app.add_input_path = String::new();
        }
        KeyCode::Char('?') => {
            app.show_help = true;
        }
        KeyCode::Char('q') => {
            let any_uploading = app.queue.iter().any(|f| f.status == FileStatus::Uploading);
            if any_uploading {
                app.show_quit_confirm = true;
            } else {
                return Ok(true);
            }
        }
        KeyCode::Esc => {
            if app.show_workers {
                app.show_workers = false;
                app.worker_focused = false;
            }
        }
        _ => {}
    }

    Ok(false)
}

fn handle_progress(
    app: &mut TuiApp,
    pe: ProgressEvent,
    progress_tx: &Sender<ProgressEvent>,
) {
    match pe {
        ProgressEvent::Started { file_id, upload_id, total_chunks, total_size: _ } => {
            if let Some(entry) = app.queue.get_mut(file_id) {
                entry.status = FileStatus::Uploading;
                entry.upload_id = Some(upload_id);
                entry.total_chunks = total_chunks;
                entry.workers = vec![
                    WorkerState { chunk_index: None, percent: 0, done: false };
                    app.config.threads
                ];
            }
        }
        ProgressEvent::WorkerProgress { file_id, worker_id, chunk_index, percent } => {
            if let Some(entry) = app.queue.get_mut(file_id) {
                if worker_id < entry.workers.len() {
                    entry.workers[worker_id] = WorkerState {
                        chunk_index: Some(chunk_index),
                        percent,
                        done: false,
                    };
                }
            }
        }
        ProgressEvent::ChunkDone { file_id, worker_id, chunk_index, bytes_done } => {
            if let Some(entry) = app.queue.get_mut(file_id) {
                if worker_id < entry.workers.len() {
                    entry.workers[worker_id].done = true;
                    entry.workers[worker_id].percent = 100;
                    entry.workers[worker_id].chunk_index = Some(chunk_index);
                }
                entry.bytes_uploaded += bytes_done;
                let percent = ((entry.bytes_uploaded as f64 / entry.size as f64) * 100.0) as u8;
                entry.progress_percent = percent.min(100);
            }
        }
        ProgressEvent::Done { file_id } => {
            if let Some(entry) = app.queue.get_mut(file_id) {
                entry.status = FileStatus::Done;
                entry.progress_percent = 100;
                for w in entry.workers.iter_mut() {
                    w.done = true;
                    w.percent = 100;
                }
            }
            if app.selected_idx == file_id {
                app.show_workers = false;
                app.worker_focused = false;
            }
            app.state = AppState::Ready;
            check_start_next_upload(app, progress_tx);
        }
        ProgressEvent::Failed { file_id, error } => {
            if let Some(entry) = app.queue.get_mut(file_id) {
                entry.status = FileStatus::Failed;
                entry.error_message = Some(error.clone());
            }
            if app.selected_idx == file_id {
                app.show_workers = false;
                app.worker_focused = false;
            }
            app.toast = Some((format!("Upload failed: {}", error), Instant::now()));
            app.state = AppState::Ready;
            check_start_next_upload(app, progress_tx);
        }
    }
}

fn check_start_next_upload(
    app: &mut TuiApp,
    progress_tx: &Sender<ProgressEvent>,
) {
    let already_uploading = app.queue.iter().any(|f| f.status == FileStatus::Uploading);
    if already_uploading {
        return;
    }

    if let Some((idx, entry)) = app.queue.iter_mut().enumerate().find(|(_, f)| f.status == FileStatus::Queued) {
        entry.status = FileStatus::Uploading;
        app.state = AppState::Uploading;
        let file_id = idx;
        let path = entry.path.clone();
        let config = app.config.clone();
        let tx = progress_tx.clone();

        thread::spawn(move || {
            let _ = transfer::upload_file(file_id, path, config, tx);
        });
    }
}
