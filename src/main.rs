mod app;
mod config;
mod latency;
mod subscribe;
mod vmess;
mod xray;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

use app::{App, SortColumn, SortDirection};
use latency::{test_all_latencies, LatencyResult, TestType};
use subscribe::fetch_subscription;
use vmess::LatencyStatus;
use xray::{restart_xray_service, save_config_with_path, DEFAULT_XRAY_CONFIG_PATH};

/// Subscription Manager - A TUI tool for managing vmess nodes
#[derive(Parser)]
#[command(name = "subman")]
#[command(about = "A TUI tool for managing vmess subscription nodes")]
struct Args {
    /// Number of parallel latency tests
    #[arg(short, long, default_value = "10")]
    parallel: usize,

    /// Path to xray config file
    #[arg(short, long, default_value = DEFAULT_XRAY_CONFIG_PATH)]
    config: String,
}

/// Pad a string to target width, accounting for wide characters (CJK)
fn pad_string(s: &str, target_width: usize) -> String {
    let display_width: usize = s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    if display_width >= target_width {
        s.to_string()
    } else {
        let padding = target_width - display_width;
        format!("{}{}", s, " ".repeat(padding))
    }
}

/// Calculate display width of a string (CJK chars count as 2)
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run
    let mut app = App::new(args.parallel, args.config);
    let result = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    // Channel for receiving latency results
    let (latency_tx, mut latency_rx) = mpsc::channel::<LatencyResult>(100);

    loop {
        // Draw UI
        terminal.draw(|f| ui(f, app))?;

        // Check for latency results
        while let Ok(result) = latency_rx.try_recv() {
            let test_type = result.test_type;
            app.update_latency(result);
            let tested = app
                .sorted_nodes
                .iter()
                .filter(|n| match test_type {
                    TestType::Http => n.node.http_latency.is_tested(),
                    TestType::Tcp => n.node.tcp_latency.is_tested(),
                })
                .count();
            let total = app.sorted_nodes.len();
            if tested == total {
                app.testing = false;
                app.current_test_type = None;
                let type_name = match test_type {
                    TestType::Http => "HTTP",
                    TestType::Tcp => "TCP",
                };
                app.set_status(format!("{type_name} latency test completed"));
                app.resort();
                app.save_config();
            } else {
                let type_name = match test_type {
                    TestType::Http => "HTTP",
                    TestType::Tcp => "TCP",
                };
                app.set_status(format!(
                    "Testing {type_name} latency... ({tested}/{total})"
                ));
            }
        }

        // Poll for events with timeout
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Handle error popup - any key closes it
                if app.error_message.is_some() {
                    app.clear_error();
                    continue;
                }

                // Handle input mode
                if app.input_mode {
                    match key.code {
                        KeyCode::Enter => {
                            app.confirm_input();
                        }
                        KeyCode::Esc => {
                            app.exit_input_mode();
                        }
                        KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_buffer.pop();
                        }
                        _ => {}
                    }
                    continue;
                }

                // Handle Ctrl+C
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if app.testing {
                        app.cancel_test();
                    } else {
                        app.should_quit = true;
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                        app.should_quit = true;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.select_previous();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.select_next();
                    }
                    KeyCode::Char('s') => {
                        // Cycle sort column
                        app.cycle_sort();
                    }
                    KeyCode::Char('S') => {
                        // Toggle sort direction
                        app.toggle_sort_direction();
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') => {
                        if !app.testing && !app.refreshing {
                            app.enter_input_mode();
                        }
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        if !app.refreshing && !app.testing {
                            if let Some(url) = app.subscribe_url.clone() {
                                app.refreshing = true;
                                // Clear list first
                                app.clear_nodes();
                                app.set_status("Refreshing subscription...");
                                terminal.draw(|f| ui(f, app))?;

                                match fetch_subscription(&url).await {
                                    Ok(nodes) => {
                                        let count = nodes.len();
                                        app.set_nodes(nodes);
                                        app.save_config();
                                        app.set_status(format!(
                                            "Loaded {count} nodes. Press t for TCP, T for HTTP test."
                                        ));
                                    }
                                    Err(e) => {
                                        app.set_error(format!("{e}"));
                                        app.set_status("Failed to refresh subscription");
                                    }
                                }
                                app.refreshing = false;
                            } else {
                                app.set_status("No subscription URL. Press U to set one.");
                            }
                        }
                    }
                    KeyCode::Char('t') => {
                        // TCP test (lowercase t)
                        if !app.testing && !app.refreshing && !app.sorted_nodes.is_empty() {
                            app.testing = true;
                            app.current_test_type = Some(TestType::Tcp);
                            app.clear_tcp_latencies();
                            app.set_status("Starting TCP latency test...");

                            let nodes = app.get_nodes_clone();
                            let tx = latency_tx.clone();
                            let parallel = app.parallel_count;
                            let cancel_flag = app.get_cancel_flag();

                            tokio::spawn(async move {
                                test_all_latencies(nodes, tx, parallel, TestType::Tcp, cancel_flag)
                                    .await;
                            });
                        }
                    }
                    KeyCode::Char('T') => {
                        // HTTP test (uppercase T)
                        if !app.testing && !app.refreshing && !app.sorted_nodes.is_empty() {
                            app.testing = true;
                            app.current_test_type = Some(TestType::Http);
                            app.clear_http_latencies();
                            app.set_status("Starting HTTP latency test...");

                            let nodes = app.get_nodes_clone();
                            let tx = latency_tx.clone();
                            let parallel = app.parallel_count;
                            let cancel_flag = app.get_cancel_flag();

                            tokio::spawn(async move {
                                test_all_latencies(nodes, tx, parallel, TestType::Http, cancel_flag)
                                    .await;
                            });
                        }
                    }
                    KeyCode::Enter => {
                        if !app.refreshing {
                            if let (Some(node), Some(original_index)) =
                                (app.selected_node().cloned(), app.selected_original_index())
                            {
                                let node_name = node.display_name();
                                app.set_status(format!("Applying {node_name}..."));
                                terminal.draw(|f| ui(f, app))?;

                                let config_path = app.xray_config_path.clone();
                                match save_config_with_path(&node, &config_path) {
                                    Ok(()) => match restart_xray_service() {
                                        Ok(result) => {
                                            app.set_active_node(original_index);
                                            app.set_status(format!(
                                                "Applied {node_name} - xray restarted (PID: {} -> {})",
                                                result.old_pid, result.new_pid
                                            ));
                                        }
                                        Err(e) => {
                                            app.set_status(format!(
                                                "Config saved but failed to restart xray: {e}"
                                            ));
                                        }
                                    },
                                    Err(e) => {
                                        app.set_status(format!("Failed to save config: {e}"));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Node list
            Constraint::Length(3), // Status bar
        ])
        .split(f.area());

    // Calculate maximum name width from all nodes
    let name_max_width = app
        .sorted_nodes
        .iter()
        .map(|n| display_width(&n.node.display_name()))
        .max()
        .unwrap_or(10)
        .max(10); // Minimum width of 10

    let addr_max_width = 25;

    // Sort indicator for column
    let sort_indicator = |col: SortColumn| -> &'static str {
        if app.sort_column == col {
            match app.sort_direction {
                SortDirection::Ascending => "▲",
                SortDirection::Descending => "▼",
            }
        } else {
            " "
        }
    };

    // Build header row - format matches data rows exactly
    // Data row format: marker(2) + name(width) + "  " + addr(width) + "  " + port(5) + "  " + tcp(8) + "  " + http(8)
    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let header = Line::from(vec![
        // Marker column - use for Name sort indicator
        Span::styled(format!("{} ", sort_indicator(SortColumn::Name)), header_style),
        // Name + gap + Address + gap + Port + gap
        Span::styled(
            format!(
                "{}  {}  {:>5}  ",
                pad_string("Name", name_max_width),
                pad_string("Address", addr_max_width),
                "Port"
            ),
            header_style,
        ),
        // TCP column (7 chars + 1 sort indicator = 8 total)
        Span::styled(format!("{:>7}{}", "TCP", sort_indicator(SortColumn::Tcp)), header_style),
        Span::styled("  ", header_style),
        // HTTP column (7 chars + 1 sort indicator = 8 total)
        Span::styled(format!("{:>7}{}", "HTTP", sort_indicator(SortColumn::Http)), header_style),
    ]);

    // Build list items (header + nodes)
    let mut items: Vec<ListItem> = vec![ListItem::new(header)];

    items.extend(app.sorted_nodes.iter().enumerate().map(|(i, indexed)| {
        let node = &indexed.node;
        let original_index = indexed.original_index;
        let name = node.display_name();
        let addr = &node.add;
        let port = node.get_port();

        // Check if this is the active node
        let is_active = app.active_node_index == Some(original_index);

        // Format latency with color
        let (tcp_text, tcp_style) = match node.tcp_latency {
            LatencyStatus::Success(ms) => (format!("{ms}ms"), Style::default()),
            LatencyStatus::TimedOut => ("timeout".to_string(), Style::default().fg(Color::Red)),
            LatencyStatus::NotTested => ("--".to_string(), Style::default()),
        };
        let (http_text, http_style) = match node.http_latency {
            LatencyStatus::Success(ms) => (format!("{ms}ms"), Style::default()),
            LatencyStatus::TimedOut => ("timeout".to_string(), Style::default().fg(Color::Red)),
            LatencyStatus::NotTested => ("--".to_string(), Style::default()),
        };

        // Pad name and address to align columns
        let padded_name = pad_string(&name, name_max_width);
        let padded_addr = pad_string(addr, addr_max_width);

        // Active node marker
        let marker = if is_active { "● " } else { "  " };

        let base_style = if i == app.selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };

        // Build line with different styles for latency values
        let line = Line::from(vec![
            Span::styled(
                marker,
                if is_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                },
            ),
            Span::styled(
                format!("{padded_name}  {padded_addr}  {port:>5}  "),
                base_style,
            ),
            Span::styled(format!("{tcp_text:>8}"), tcp_style.patch(base_style)),
            Span::styled("  ", base_style),
            Span::styled(format!("{http_text:>8}"), http_style.patch(base_style)),
        ]);

        ListItem::new(line)
    }));

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Subscription Manager ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray));

    // Use ListState to track selection (offset by 1 for header)
    let mut list_state = ListState::default();
    list_state.select(Some(app.selected + 1)); // +1 because header is item 0

    f.render_stateful_widget(list, chunks[0], &mut list_state);

    // Status bar with help
    let help_text = if app.testing {
        " Ctrl+C:Cancel Test "
    } else {
        " ↑↓:Select  Enter:Apply  R:Refresh  t:TCP  T:HTTP  s:Sort  S:Reverse  U:URL  Q:Quit "
    };
    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = status_block.inner(chunks[1]);
    f.render_widget(status_block, chunks[1]);

    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    let status = Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Green));
    f.render_widget(status, status_chunks[0]);

    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(ratatui::layout::Alignment::Right);
    f.render_widget(help, status_chunks[1]);

    // Input dialog
    if app.input_mode {
        let area = f.area();
        let dialog_width = 60.min(area.width.saturating_sub(4));
        let dialog_height = 5;
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;

        let dialog_area =
            ratatui::layout::Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);

        f.render_widget(Clear, dialog_area);

        let input_block = Block::default()
            .title(" Enter Subscription URL (Enter to confirm, Esc to cancel) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner_area = input_block.inner(dialog_area);
        f.render_widget(input_block, dialog_area);

        let input =
            Paragraph::new(app.input_buffer.as_str()).style(Style::default().fg(Color::White));
        f.render_widget(input, inner_area);

        // Show cursor at end of input
        f.set_cursor_position((
            inner_area.x + app.input_buffer.len() as u16,
            inner_area.y,
        ));
    }

    // Error dialog
    if let Some(error) = &app.error_message {
        let area = f.area();
        let dialog_width = 50.min(area.width.saturating_sub(4));
        let dialog_height = 7;
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;

        let dialog_area =
            ratatui::layout::Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);

        f.render_widget(Clear, dialog_area);

        let error_block = Block::default()
            .title(" Error (Press any key to close) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));

        let inner_area = error_block.inner(dialog_area);
        f.render_widget(error_block, dialog_area);

        let error_text = Paragraph::new(error.as_str())
            .style(Style::default().fg(Color::Red))
            .wrap(ratatui::widgets::Wrap { trim: true });
        f.render_widget(error_text, inner_area);
    }
}
