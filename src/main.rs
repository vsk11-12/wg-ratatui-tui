use anyhow::Result;
use crossterm::{
    event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf, process::Command, time::Duration};
use tokio::sync::mpsc;

// ==========================================
// Dynamic Theme System
// ==========================================

fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        ) {
            return Color::Rgb(r, g, b);
        }
    }
    Color::Reset
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub border: Option<String>,
    pub title: Option<String>,
    pub active: Option<String>,
    pub inactive: Option<String>,
    pub text: Option<String>,
    pub highlight_bg: Option<String>,
    pub highlight_fg: Option<String>,
    pub accent: Option<String>,
    pub default_tag: Option<String>,
    pub search: Option<String>,
    pub status: Option<String>,
}

impl ThemeConfig {
    pub fn default_mocha() -> Self {
        Self {
            border: Some("#cba6f7".into()),       // Lavender
            title: Some("#f5c2e7".into()),        // Pink
            active: Some("#a6e3a1".into()),       // Green
            inactive: Some("#f38ba8".into()),     // Red
            text: Some("#cdd6f4".into()),         // Text
            highlight_bg: Some("#313244".into()), // Surface1
            highlight_fg: Some("#f9e2af".into()), // Yellow
            accent: Some("#89b4fa".into()),       // Blue
            default_tag: Some("#fab387".into()),   // Peach
            search: Some("#74c7ec".into()),       // Sapphire
            status: Some("#a6adc8".into()),       // Subtext0
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub border: Color,
    pub title: Color,
    pub active: Color,
    pub inactive: Color,
    pub text: Color,
    pub highlight_bg: Color,
    pub highlight_fg: Color,
    pub accent: Color,
    pub default_tag: Color,
    pub search: Color,
    pub status: Color,
}

impl Theme {
    pub fn mocha() -> Self {
        let mut theme = Self {
            border: Color::Reset,
            title: Color::Reset,
            active: Color::Reset,
            inactive: Color::Reset,
            text: Color::Reset,
            highlight_bg: Color::Reset,
            highlight_fg: Color::Reset,
            accent: Color::Reset,
            default_tag: Color::Reset,
            search: Color::Reset,
            status: Color::Reset,
        };
        theme.apply_config(ThemeConfig::default_mocha());
        theme
    }

    pub fn apply_config(&mut self, config: ThemeConfig) {
        if let Some(hex) = config.border { self.border = parse_hex_color(&hex); }
        if let Some(hex) = config.title { self.title = parse_hex_color(&hex); }
        if let Some(hex) = config.active { self.active = parse_hex_color(&hex); }
        if let Some(hex) = config.inactive { self.inactive = parse_hex_color(&hex); }
        if let Some(hex) = config.text { self.text = parse_hex_color(&hex); }
        if let Some(hex) = config.highlight_bg { self.highlight_bg = parse_hex_color(&hex); }
        if let Some(hex) = config.highlight_fg { self.highlight_fg = parse_hex_color(&hex); }
        if let Some(hex) = config.accent { self.accent = parse_hex_color(&hex); }
        if let Some(hex) = config.default_tag { self.default_tag = parse_hex_color(&hex); }
        if let Some(hex) = config.search { self.search = parse_hex_color(&hex); }
        if let Some(hex) = config.status { self.status = parse_hex_color(&hex); }
    }

    pub fn load_or_default() -> Self {
        let mut theme = Self::mocha();

        if let Some(config_dir) = dirs::config_dir().or_else(|| dirs::home_dir().map(|h| h.join(".config"))) {
            let app_config_dir = config_dir.join("wg-tui");
            let theme_file = app_config_dir.join("theme.json");

            if theme_file.exists() {
                if let Ok(content) = fs::read_to_string(&theme_file) {
                    if let Ok(config) = serde_json::from_str::<ThemeConfig>(&content) {
                        theme.apply_config(config);
                    }
                }
            } else {
                let _ = fs::create_dir_all(&app_config_dir);
                if let Ok(default_json) = serde_json::to_string_pretty(&ThemeConfig::default_mocha()) {
                    let _ = fs::write(&theme_file, default_json);
                }
            }
        }

        theme
    }
}

// ==========================================
// Data Structures
// ==========================================

#[derive(Clone, Debug)]
pub struct VpnConfig {
    pub name: String,
    pub conn_type: String,
    pub is_active: bool,
    pub is_default: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IpInfo {
    pub ip: String,
    pub city: String,
    pub country: String,
    pub org: String,
}

pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    VpnListUpdated(Vec<VpnConfig>),
    IpInfoUpdated(Option<IpInfo>),
    PingResult(Option<u128>),
    StatusMsg(String),
    DownloadsUpdated(Vec<PathBuf>),
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ActiveTab {
    Connections = 0,
    Import = 1,
    PublicIp = 2,
    Help = 3,
}

pub struct App {
    pub vpns: Vec<VpnConfig>,
    pub filtered_vpns: Vec<VpnConfig>,
    pub list_state: ListState,
    pub active_tab: ActiveTab,
    pub search_query: String,
    pub is_searching: bool,
    pub status_msg: String,
    pub ip_info: Option<IpInfo>,
    pub is_fetching_ip: bool,
    pub ping_ms: Option<u128>,
    pub should_quit: bool,
    pub show_quit_popup: bool,
    pub show_help_popup: bool,
    pub event_tx: mpsc::UnboundedSender<AppEvent>,

    pub downloaded_files: Vec<PathBuf>,
    pub download_list_state: ListState,
    pub theme: Theme,
}

impl App {
    pub fn new(event_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self {
            vpns: Vec::new(),
            filtered_vpns: Vec::new(),
            list_state: ListState::default(),
            active_tab: ActiveTab::Connections,
            search_query: String::new(),
            is_searching: false,
            status_msg: String::from("Initialized. Loading connections..."),
            ip_info: None,
            is_fetching_ip: false,
            ping_ms: None,
            should_quit: false,
            show_quit_popup: false,
            show_help_popup: false,
            event_tx,
            downloaded_files: Vec::new(),
            download_list_state: ListState::default(),
            theme: Theme::load_or_default(),
        }
    }

    pub fn filter_vpns(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_vpns = self.vpns.clone();
        } else {
            let q = self.search_query.to_lowercase();
            self.filtered_vpns = self
                .vpns
                .iter()
                .filter(|v| v.name.to_lowercase().contains(&q))
                .cloned()
                .collect();
        }

        if self.filtered_vpns.is_empty() {
            self.list_state.select(None);
        } else {
            let selected = self.list_state.selected().unwrap_or(0);
            if selected >= self.filtered_vpns.len() {
                self.list_state.select(Some(self.filtered_vpns.len() - 1));
            } else {
                self.list_state.select(Some(selected));
            }
        }
    }

    pub fn select_next(&mut self) {
        match self.active_tab {
            ActiveTab::Connections => {
                if self.filtered_vpns.is_empty() {
                    return;
                }
                let i = match self.list_state.selected() {
                    Some(i) => (i + 1) % self.filtered_vpns.len(),
                    None => 0,
                };
                self.list_state.select(Some(i));
            }
            ActiveTab::Import => {
                if self.downloaded_files.is_empty() {
                    return;
                }
                let i = match self.download_list_state.selected() {
                    Some(i) => (i + 1) % self.downloaded_files.len(),
                    None => 0,
                };
                self.download_list_state.select(Some(i));
            }
            _ => {}
        }
    }

    pub fn select_prev(&mut self) {
        match self.active_tab {
            ActiveTab::Connections => {
                if self.filtered_vpns.is_empty() {
                    return;
                }
                let i = match self.list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.filtered_vpns.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.list_state.select(Some(i));
            }
            ActiveTab::Import => {
                if self.downloaded_files.is_empty() {
                    return;
                }
                let i = match self.download_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.downloaded_files.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.download_list_state.select(Some(i));
            }
            _ => {}
        }
    }
}

// ==========================================
// Async Tasks
// ==========================================

fn fetch_downloads_async(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        if let Some(home) = dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads"))) {
            let mut files = Vec::new();
            if let Ok(entries) = fs::read_dir(home) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension() {
                            if ext == "conf" || ext == "wireguard" {
                                files.push(path);
                            }
                        }
                    }
                }
            }
            let _ = tx.send(AppEvent::DownloadsUpdated(files));
        }
    });
}

fn fetch_vpns_async(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let active_output = Command::new("nmcli")
            .args(["-t", "-f", "NAME", "connection", "show", "--active"])
            .output()
            .ok();

        let active_names: Vec<String> = active_output
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let priority_output = Command::new("nmcli")
            .args(["-t", "-f", "NAME,AUTOCONNECT-PRIORITY", "connection", "show"])
            .output()
            .ok();

        let mut priorities = std::collections::HashMap::new();
        if let Some(out) = priority_output {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 {
                    if let Ok(p) = parts[1].parse::<i32>() {
                        priorities.insert(parts[0].to_string(), p);
                    }
                }
            }
        }

        let all_output = Command::new("nmcli")
            .args(["-t", "-f", "NAME,TYPE", "connection", "show"])
            .output()
            .ok();

        let mut vpns = Vec::new();
        if let Some(out) = all_output {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2
                    && (parts[1].contains("wireguard") || parts[1].contains("vpn"))
                {
                    let name = parts[0].to_string();
                    let conn_type = parts[1].to_string();
                    let is_active = active_names.contains(&name);
                    let prio = priorities.get(&name).cloned().unwrap_or(0);
                    let is_default = prio >= 100;

                    vpns.push(VpnConfig {
                        name,
                        conn_type,
                        is_active,
                        is_default,
                    });
                }
            }
        }
        let _ = tx.send(AppEvent::VpnListUpdated(vpns));
    });
}

fn import_wireguard_file_async(path: PathBuf, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let path_str = path.to_string_lossy();
        let _ = tx.send(AppEvent::StatusMsg(format!("Importing {}...", path_str)));

        let output = Command::new("nmcli")
            .args(["connection", "import", "type", "wireguard", "file", &path_str])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let _ = tx.send(AppEvent::StatusMsg(format!("Successfully imported {}", path_str)));
                fetch_vpns_async(tx.clone());
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let _ = tx.send(AppEvent::StatusMsg(format!("Import failed: {}", err)));
            }
            Err(e) => {
                let _ = tx.send(AppEvent::StatusMsg(format!("Execution error: {}", e)));
            }
        }
    });
}

fn set_default_vpn_async(target_vpn: VpnConfig, all_vpns: Vec<VpnConfig>, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::StatusMsg(format!("Setting {} as default...", target_vpn.name)));

        for vpn in all_vpns {
            if vpn.name == target_vpn.name {
                let _ = Command::new("nmcli")
                    .args(["connection", "modify", &vpn.name, "connection.autoconnect", "yes", "connection.autoconnect-priority", "100"])
                    .output();
            } else {
                let _ = Command::new("nmcli")
                    .args(["connection", "modify", &vpn.name, "connection.autoconnect", "no", "connection.autoconnect-priority", "0"])
                    .output();
            }
        }

        let _ = tx.send(AppEvent::StatusMsg(format!("Default VPN set to {}", target_vpn.name)));
        fetch_vpns_async(tx);
    });
}

fn toggle_vpn_async(vpn: VpnConfig, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let action = if vpn.is_active { "down" } else { "up" };
        let _ = tx.send(AppEvent::StatusMsg(format!(
            "Toggling {} {}...",
            vpn.name, action
        )));

        let _ = Command::new("nmcli")
            .args(["connection", action, &vpn.name])
            .output();

        let _ = tx.send(AppEvent::StatusMsg(format!(
            "Successfully toggled {}",
            vpn.name
        )));
        fetch_vpns_async(tx.clone());
        ping_latency_async(tx);
    });
}

fn fetch_ip_info_async(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build();

        if let Ok(client) = client {
            if let Ok(res) = client.get("https://ipapi.co/json/").send().await {
                if let Ok(info) = res.json::<IpInfo>().await {
                    let _ = tx.send(AppEvent::IpInfoUpdated(Some(info)));
                    return;
                }
            }
        }
        let _ = tx.send(AppEvent::IpInfoUpdated(None));
    });
}

fn ping_latency_async(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let output = Command::new("ping")
            .args(["-c", "1", "-W", "2", "1.1.1.1"])
            .output()
            .ok();

        if let Some(out) = output {
            if out.status.success() {
                let _ = tx.send(AppEvent::PingResult(Some(start.elapsed().as_millis())));
                return;
            }
        }
        let _ = tx.send(AppEvent::PingResult(None));
    });
}

// ==========================================
// Helper Layout Utilities
// ==========================================

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width),
            Constraint::Fill(1),
        ])
        .split(vertical[1])[1]
}

// ==========================================
// Main Function & Application Loop
// ==========================================

#[tokio::main]
async fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(tx.clone());

    fetch_vpns_async(tx.clone());
    fetch_downloads_async(tx.clone());
    fetch_ip_info_async(tx.clone());
    ping_latency_async(tx.clone());

    let event_tx = tx.clone();
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let _ = event_tx.send(AppEvent::Tick);
                }
                maybe_event = reader.next() => {
                    if let Some(Ok(CrosstermEvent::Key(key))) = maybe_event {
                        let _ = event_tx.send(AppEvent::Key(key));
                    }
                }
            }
        }
    });

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        if let Some(event) = rx.recv().await {
            match event {
                AppEvent::Tick => {}
                AppEvent::Key(key) => handle_key_input(&mut app, key),
                AppEvent::VpnListUpdated(vpns) => {
                    app.vpns = vpns;
                    app.filter_vpns();
                }
                AppEvent::DownloadsUpdated(files) => {
                    app.downloaded_files = files;
                    if !app.downloaded_files.is_empty() && app.download_list_state.selected().is_none() {
                        app.download_list_state.select(Some(0));
                    }
                }
                AppEvent::StatusMsg(msg) => {
                    app.status_msg = msg;
                }
                AppEvent::IpInfoUpdated(info) => {
                    app.ip_info = info;
                    app.is_fetching_ip = false;
                }
                AppEvent::PingResult(res) => {
                    app.ping_ms = res;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn handle_key_input(app: &mut App, key: KeyEvent) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    if app.show_quit_popup {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.should_quit = true,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                app.show_quit_popup = false;
            }
            _ => {}
        }
        return;
    }

    if app.show_help_popup {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                app.show_help_popup = false;
            }
            _ => {}
        }
        return;
    }

    if app.is_searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.is_searching = false,
            KeyCode::Backspace => {
                app.search_query.pop();
                app.filter_vpns();
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.filter_vpns();
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.show_quit_popup = true,
        KeyCode::Char('?') => app.show_help_popup = true,
        KeyCode::Char('/') => app.is_searching = true,
        KeyCode::Char('1') => app.active_tab = ActiveTab::Connections,
        KeyCode::Char('2') => app.active_tab = ActiveTab::Import,
        KeyCode::Char('3') => app.active_tab = ActiveTab::PublicIp,
        KeyCode::Char('4') => app.active_tab = ActiveTab::Help,
        KeyCode::Tab => {
            app.active_tab = match app.active_tab {
                ActiveTab::Connections => ActiveTab::Import,
                ActiveTab::Import => ActiveTab::PublicIp,
                ActiveTab::PublicIp => ActiveTab::Help,
                ActiveTab::Help => ActiveTab::Connections,
            };
        }
        KeyCode::Char('r') => {
            app.theme = Theme::load_or_default();
            app.status_msg = "Reloaded config & refreshing status...".to_string();
            fetch_vpns_async(app.event_tx.clone());
            fetch_downloads_async(app.event_tx.clone());
            ping_latency_async(app.event_tx.clone());
        }
        KeyCode::Char('i') => {
            app.is_fetching_ip = true;
            app.status_msg = "Fetching IP info...".to_string();
            fetch_ip_info_async(app.event_tx.clone());
        }
        KeyCode::Char('d') => {
            if app.active_tab == ActiveTab::Connections {
                if let Some(i) = app.list_state.selected() {
                    if let Some(vpn) = app.filtered_vpns.get(i).cloned() {
                        set_default_vpn_async(vpn, app.vpns.clone(), app.event_tx.clone());
                    }
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
        KeyCode::Char('h') => {
            app.active_tab = match app.active_tab {
                ActiveTab::Connections => ActiveTab::Help,
                ActiveTab::Import => ActiveTab::Connections,
                ActiveTab::PublicIp => ActiveTab::Import,
                ActiveTab::Help => ActiveTab::PublicIp,
            };
        }
        KeyCode::Char('l') | KeyCode::Enter => match app.active_tab {
            ActiveTab::Connections => {
                if let Some(i) = app.list_state.selected() {
                    if let Some(vpn) = app.filtered_vpns.get(i).cloned() {
                        toggle_vpn_async(vpn, app.event_tx.clone());
                    }
                }
            }
            ActiveTab::Import => {
                if let Some(i) = app.download_list_state.selected() {
                    if let Some(file) = app.downloaded_files.get(i).cloned() {
                        import_wireguard_file_async(file, app.event_tx.clone());
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }
}

// ==========================================
// Ratatui UI Rendering
// ==========================================

fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);

    match app.active_tab {
        ActiveTab::Connections => render_connections_tab(f, chunks[1], app),
        ActiveTab::Import => render_import_tab(f, chunks[1], app),
        ActiveTab::PublicIp => render_ip_tab(f, chunks[1], app),
        ActiveTab::Help => render_help_tab(f, chunks[1], app),
    }

    draw_controls_bar(f, app, chunks[2]);

    if app.show_quit_popup {
        draw_quit_popup(f, app);
    }

    if app.show_help_popup {
        draw_help_popup(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let make_tab = |num: &str, label: &str, tab: ActiveTab| {
        if app.active_tab == tab {
            Span::styled(
                format!(" [{num}: {label}] "),
                Style::default().fg(app.theme.active).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" {num}: {label} "),
                Style::default().fg(app.theme.text),
            )
        }
    };

    let title_text = Line::from(vec![
        Span::styled(" View: ", Style::default().fg(app.theme.title).add_modifier(Modifier::BOLD)),
        make_tab("1", "Connections", ActiveTab::Connections),
        Span::raw("│"),
        make_tab("2", "Import", ActiveTab::Import),
        Span::raw("│"),
        make_tab("3", "GeoIP", ActiveTab::PublicIp),
        Span::raw("│"),
        make_tab("4", "Help", ActiveTab::Help),
    ]);

    let header = Paragraph::new(title_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border))
            .title(Span::styled(" ⚡ WireGuard / VPN Manager ", Style::default().fg(app.theme.title))),
    );
    f.render_widget(header, area);
}

fn render_connections_tab(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);

    let search_title = if app.is_searching {
        Span::styled(
            format!(" Search Filter: '{}' [Esc: Clear] ", app.search_query),
            Style::default().fg(app.theme.search).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!(" Filter (Press '/' to search, 'd' to set Default) | Active: {} ", app.search_query),
            Style::default().fg(app.theme.title),
        )
    };

    let border_color = if app.is_searching { app.theme.search } else { app.theme.border };
    let search_bar = Paragraph::new(format!(" / {}", app.search_query))
        .style(Style::default().fg(app.theme.text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(search_title),
        );
    f.render_widget(search_bar, chunks[0]);

    let items: Vec<ListItem> = app
        .filtered_vpns
        .iter()
        .map(|vpn| {
            let (status_symbol, status_color) = if vpn.is_active {
                ("● Active", app.theme.active)
            } else {
                ("○ Inactive", app.theme.inactive)
            };

            let default_span = if vpn.is_default {
                Span::styled(" ★ [DEFAULT]", Style::default().fg(app.theme.default_tag).add_modifier(Modifier::BOLD))
            } else {
                Span::raw("")
            };

            let line = Line::from(vec![
                Span::styled(format!(" {:<26} ", vpn.name), Style::default().fg(app.theme.text).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:<14} ", vpn.conn_type), Style::default().fg(app.theme.accent)),
                Span::styled(status_symbol, Style::default().fg(status_color)),
                default_span,
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(
                    format!(" VPN Profiles ({}/{}) ", app.filtered_vpns.len(), app.vpns.len()),
                    Style::default().fg(app.theme.title),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.highlight_bg)
                .fg(app.theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, chunks[1], &mut app.list_state);

    if !app.filtered_vpns.is_empty() {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(app.theme.border));

        let mut scrollbar_state = ScrollbarState::new(app.filtered_vpns.len().saturating_sub(1))
            .position(app.list_state.selected().unwrap_or(0));

        f.render_stateful_widget(
            scrollbar,
            chunks[1].inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn render_import_tab(f: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .downloaded_files
        .iter()
        .map(|file| {
            let file_name = file.file_name().unwrap_or_default().to_string_lossy();
            let file_stem = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();

            let is_installed = app
                .vpns
                .iter()
                .any(|vpn| vpn.name.to_lowercase() == file_stem);

            let (status_text, style) = if is_installed {
                ("✔ Installed", Style::default().fg(app.theme.active))
            } else {
                ("○ Ready to Import", Style::default().fg(app.theme.text))
            };

            let line = Line::from(vec![
                Span::styled(" 📄 ", Style::default().fg(app.theme.accent)),
                Span::styled(format!("{:<32} ", file_name), Style::default().fg(app.theme.text)),
                Span::styled(status_text, style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(" Import Configurations from ~/Downloads (Press Enter) ", Style::default().fg(app.theme.title))),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.highlight_bg)
                .fg(app.theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut app.download_list_state);

    if !app.downloaded_files.is_empty() {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(app.theme.border));

        let mut scrollbar_state = ScrollbarState::new(app.downloaded_files.len().saturating_sub(1))
            .position(app.download_list_state.selected().unwrap_or(0));

        f.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn render_ip_tab(f: &mut Frame, area: Rect, app: &App) {
    let content = if app.is_fetching_ip {
        vec![Line::from(Span::styled("  Fetching GeoIP information...", Style::default().fg(app.theme.highlight_fg)))]
    } else if let Some(info) = &app.ip_info {
        vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("  Public IP : ", Style::default().fg(app.theme.title).add_modifier(Modifier::BOLD)),
                Span::styled(&info.ip, Style::default().fg(app.theme.active).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("  City      : ", Style::default().fg(app.theme.title)),
                Span::styled(&info.city, Style::default().fg(app.theme.text)),
            ]),
            Line::from(vec![
                Span::styled("  Country   : ", Style::default().fg(app.theme.title)),
                Span::styled(&info.country, Style::default().fg(app.theme.text)),
            ]),
            Line::from(vec![
                Span::styled("  ISP / Org : ", Style::default().fg(app.theme.title)),
                Span::styled(&info.org, Style::default().fg(app.theme.accent)),
            ]),
            Line::from(""),
            Line::from(Span::styled("  (Press 'i' to force refresh location)", Style::default().fg(app.theme.status))),
        ]
    } else {
        vec![
            Line::from(Span::styled("  IP details unavailable.", Style::default().fg(app.theme.inactive))),
            Line::from(Span::styled("  (Press 'i' to retry)", Style::default().fg(app.theme.status))),
        ]
    };

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(" Public IP & Routing Details ", Style::default().fg(app.theme.title))),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn render_help_tab(f: &mut Frame, area: Rect, app: &App) {
    let key_style = Style::default().fg(app.theme.highlight_fg).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(app.theme.text);
    let section_style = Style::default().fg(app.theme.active).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

    let content = vec![
        Line::from(Span::styled(" Application Controls", section_style)),
        Line::from(vec![Span::styled("  1 / 2 / 3 / 4    ", key_style), Span::styled("Switch between views", desc_style)]),
        Line::from(vec![Span::styled("  j / k or ↓ / ↑  ", key_style), Span::styled("Navigate through configuration lists", desc_style)]),
        Line::from(vec![Span::styled("  Enter / l       ", key_style), Span::styled("Toggle active VPN or install WireGuard config file", desc_style)]),
        Line::from(vec![Span::styled("  d               ", key_style), Span::styled("Mark highlighted connection as Default autoconnect", desc_style)]),
        Line::from(vec![Span::styled("  /               ", key_style), Span::styled("Filter connections list", desc_style)]),
        Line::from(vec![Span::styled("  r               ", key_style), Span::styled("Hot-reload theme.json, refresh NetworkManager & scan Downloads", desc_style)]),
        Line::from(vec![Span::styled("  i               ", key_style), Span::styled("Query public GeoIP details", desc_style)]),
        Line::from(vec![Span::styled("  ?               ", key_style), Span::styled("Toggle full keybindings overlay", desc_style)]),
        Line::from(vec![Span::styled("  q / Esc         ", key_style), Span::styled("Prompt quit application", desc_style)]),
    ];

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(" User Manual & Shortcuts ", Style::default().fg(app.theme.title))),
        );

    f.render_widget(paragraph, area);
}

fn draw_controls_bar(f: &mut Frame, app: &App, area: Rect) {
    let ping_str = match app.ping_ms {
        Some(ms) => format!("Ping: {}ms", ms),
        None => "Ping: --".to_string(),
    };

    let key_style = Style::default().fg(app.theme.title).add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(app.theme.text);

    let controls_text = Line::from(vec![
        Span::styled(
            format!("[{}]  ", app.status_msg),
            Style::default().fg(app.theme.status),
        ),
        Span::styled(format!("[{}]  ", ping_str), Style::default().fg(app.theme.accent)),
        Span::styled("[1/2/3/4]", key_style),
        Span::styled(" Views │ ", dim_style),
        Span::styled("[r]", key_style),
        Span::styled(" Reload Theme │ ", dim_style),
        Span::styled("[?]", key_style),
        Span::styled(" Manual │ ", dim_style),
        Span::styled("[/]", key_style),
        Span::styled(" Search │ ", dim_style),
        Span::styled("[Enter]", key_style),
        Span::styled(" Toggle │ ", dim_style),
        Span::styled("[q]", key_style),
        Span::styled(" Quit", dim_style),
    ]);

    let bar = Paragraph::new(controls_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border))
            .title(Span::styled(" Status & Quick Shortcuts ", Style::default().fg(app.theme.title))),
    );
    f.render_widget(bar, area);
}

fn draw_quit_popup(f: &mut Frame, app: &App) {
    let popup_area = centered_rect(46, 7, f.area());
    f.render_widget(Clear, popup_area);

    let popup_block = Block::default()
        .title(Span::styled(" Quit Confirmation ", Style::default().fg(app.theme.title)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border));

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Are you sure you want to exit WireGuard Manager?",
            Style::default().fg(app.theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" [Y] Yes ", Style::default().fg(app.theme.active).add_modifier(Modifier::BOLD)),
            Span::raw("    "),
            Span::styled(" [N] No ", Style::default().fg(app.theme.inactive).add_modifier(Modifier::BOLD)),
        ]),
    ];

    let popup_paragraph = Paragraph::new(content)
        .alignment(Alignment::Center)
        .block(popup_block);

    f.render_widget(popup_paragraph, popup_area);
}

fn draw_help_popup(f: &mut Frame, app: &App) {
    let popup_area = centered_rect(60, 18, f.area());
    f.render_widget(Clear, popup_area);

    let popup_block = Block::default()
        .title(Span::styled(" Quick Help Overlay (Press ? or Esc to close) ", Style::default().fg(app.theme.title)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border));

    let key_style = Style::default().fg(app.theme.highlight_fg).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(app.theme.text);

    let content = vec![
        Line::from(vec![Span::styled("  j / k or ↓ / ↑  ", key_style), Span::styled("Move cursor up/down", desc_style)]),
        Line::from(vec![Span::styled("  l / Enter      ", key_style), Span::styled("Toggle VPN or import config file", desc_style)]),
        Line::from(vec![Span::styled("  h / Tab        ", key_style), Span::styled("Switch navigation tabs", desc_style)]),
        Line::from(vec![Span::styled("  1 / 2 / 3 / 4  ", key_style), Span::styled("Directly switch active tab", desc_style)]),
        Line::from(vec![Span::styled("  d              ", key_style), Span::styled("Set selected VPN as default profile", desc_style)]),
        Line::from(vec![Span::styled("  /              ", key_style), Span::styled("Filter connections by name", desc_style)]),
        Line::from(vec![Span::styled("  r              ", key_style), Span::styled("Hot-reload theme.json & refresh statuses", desc_style)]),
        Line::from(vec![Span::styled("  i              ", key_style), Span::styled("Fetch public IP & GeoIP details", desc_style)]),
        Line::from(vec![Span::styled("  ?              ", key_style), Span::styled("Toggle this modal dialog", desc_style)]),
        Line::from(vec![Span::styled("  q / Esc        ", key_style), Span::styled("Quit application", desc_style)]),
    ];

    let popup_paragraph = Paragraph::new(content).block(popup_block);

    f.render_widget(popup_paragraph, popup_area);
}
