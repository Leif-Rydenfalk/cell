// cell-cli/src/tui_monitor.rs
// SPDX-License-Identifier: MIT

use anyhow::Result;
use cell_sdk::discovery::{Discovery, CellNode};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};
use std::io::{stdout, Stdout};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub struct App {
    nodes: Vec<CellNode>,
    selected_index: usize,
    scroll_offset: u16,
    logs: Vec<String>,
    running: bool,
}

impl App {
    fn new() -> Self {
        Self {
            nodes: vec![],
            selected_index: 0,
            scroll_offset: 0,
            logs: vec!["Waiting for telemetry...".to_string()],
            running: true,
        }
    }

    fn on_tick(&mut self, nodes: Vec<CellNode>) {
        self.nodes = nodes;
        if self.selected_index >= self.nodes.len() && !self.nodes.is_empty() {
            self.selected_index = self.nodes.len() - 1;
        }
    }

    fn next(&mut self) {
        if !self.nodes.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.nodes.len();
        }
    }

    fn previous(&mut self) {
        if !self.nodes.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.nodes.len() - 1;
            } else {
                self.selected_index -= 1;
            }
        }
    }
}

pub async fn run_dashboard() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let res = run_app(&mut stdout).await;

    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen)?;
    res
}

async fn run_app(terminal: &mut Stdout) -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(terminal))?;
    let mut app = App::new();
    let tick_rate = Duration::from_millis(500);
    let mut last_tick = Instant::now();

    // Background discovery task
    let (tx, mut rx) = mpsc::channel(1);
    tokio::spawn(async move {
        loop {
            let nodes = Discovery::scan().await;
            if tx.send(nodes).await.is_err() { break; }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => app.running = false,
                        KeyCode::Down | KeyCode::Char('j') => app.next(),
                        KeyCode::Up | KeyCode::Char('k') => app.previous(),
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            if let Ok(nodes) = rx.try_recv() {
                app.on_tick(nodes);
            }
            last_tick = Instant::now();
        }

        if !app.running {
            break;
        }
    }
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Body
            Constraint::Length(3), // Footer
        ])
        .split(f.size());

    // --- Header ---
    let title = Paragraph::new(vec![
        Line::from(vec![
            Span::raw(" Cell Substrate Monitor "),
            Span::styled("v0.4.0", Style::default().fg(Color::Cyan)),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Green)))
    .alignment(Alignment::Center);
    f.render_widget(title, chunks[0]);

    // --- Body (Split: List vs Details) ---
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    // 1. Cell List
    let items: Vec<ListItem> = app
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let style = if i == app.selected_index {
                Style::default().fg(Color::Black).bg(Color::White)
            } else if !node.status.is_alive {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };

            let status_icon = if node.status.is_alive { "●" } else { "○" };
            let loc = if node.lan_address.is_some() { "LAN" } else { "LCL" };
            
            ListItem::new(format!("{} {:<16} [{}]", status_icon, node.name, loc)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title(" Cells ").borders(Borders::ALL));
    f.render_widget(list, body_chunks[0]);

    // 2. Details Pane
    let selected_node = app.nodes.get(app.selected_index);
    let details_text = if let Some(node) = selected_node {
        let lat_local = node.status.local_latency.map(|d| format!("{:.2}ms", d.as_secs_f64()*1000.0)).unwrap_or_else(|| "N/A".into());
        let lat_lan = node.status.lan_latency.map(|d| format!("{:.2}ms", d.as_secs_f64()*1000.0)).unwrap_or_else(|| "N/A".into());
        
        let path = node.local_socket.as_ref().map(|p| p.to_string_lossy()).unwrap_or("Remote".into());
        let addr = node.lan_address.as_deref().unwrap_or("N/A");

        vec![
            Line::from(vec![Span::styled("Name: ", Style::default().fg(Color::Yellow)), Span::raw(&node.name)]),
            Line::from(vec![Span::styled("ID:   ", Style::default().fg(Color::Yellow)), Span::raw(node.instance_id.to_string())]),
            Line::from(""),
            Line::from(vec![Span::styled("Socket: ", Style::default().fg(Color::Blue)), Span::raw(path)]),
            Line::from(vec![Span::styled("LAN IP: ", Style::default().fg(Color::Blue)), Span::raw(addr)]),
            Line::from(""),
            Line::from(vec![Span::styled("Latency (Local): ", Style::default().fg(Color::Magenta)), Span::raw(lat_local)]),
            Line::from(vec![Span::styled("Latency (LAN):   ", Style::default().fg(Color::Magenta)), Span::raw(lat_lan)]),
            Line::from(""),
            Line::from(Span::styled("Press 'k' to kill (Not Impl in TUI)", Style::default().fg(Color::DarkGray))),
        ]
    } else {
        vec![Line::from("No cells found.")]
    };

    let details = Paragraph::new(details_text)
        .block(Block::default().title(" Inspector ").borders(Borders::ALL));
    f.render_widget(details, body_chunks[1]);

    // --- Footer ---
    let help = Paragraph::new("Q: Quit | ↑/↓: Select | Cell Substrate Runtime")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(help, chunks[2]);
}