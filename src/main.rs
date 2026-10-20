use anyhow::{anyhow, Context, Result};
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
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::process::Command;
use tokio::sync::mpsc;
// ============================================================
// Configuration / theme
// ============================================================

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
            border: Some("#cba6f7".into()),
            title: Some("#f5c2e7".into()),
            active: Some("#a6e3a1".into()),
            inactive: Some("#f38ba8".into()),
            text: Some("#cdd6f4".into()),
            highlight_bg: Some("#313244".into()),
            highlight_fg: Some("#f9e2af".into()),
            accent: Some("#89b4fa".into()),
            default_tag: Some("#fab387".into()),
            search: Some("#74c7ec".into()),
            status: Some("#a6adc8".into()),
        }
    }
}

fn parse_hex_color(value: &str) -> Result<Color> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(anyhow!("expected a six-digit hex color, got {value:?}"));
    }
    let r = u8::from_str_radix(&hex[0..2], 16)?;
    let g = u8::from_str_radix(&hex[2..4], 16)?;
    let b = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Color::Rgb(r, g, b))
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
        let c = ThemeConfig::default_mocha();
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
        theme.apply_config(c);
        theme
    }

    fn set_if_valid(slot: &mut Color, value: Option<String>) -> Option<String> {
        let value = value?;
        match parse_hex_color(&value) {
            Ok(color) => {
                *slot = color;
                None
            }
            Err(e) => Some(e.to_string()),
        }
    }

    pub fn apply_config(&mut self, config: ThemeConfig) {
        let _ = Self::set_if_valid(&mut self.border, config.border);
        let _ = Self::set_if_valid(&mut self.title, config.title);
        let _ = Self::set_if_valid(&mut self.active, config.active);
        let _ = Self::set_if_valid(&mut self.inactive, config.inactive);
        let _ = Self::set_if_valid(&mut self.text, config.text);
        let _ = Self::set_if_valid(&mut self.highlight_bg, config.highlight_bg);
        let _ = Self::set_if_valid(&mut self.highlight_fg, config.highlight_fg);
        let _ = Self::set_if_valid(&mut self.accent, config.accent);
        let _ = Self::set_if_valid(&mut self.default_tag, config.default_tag);
        let _ = Self::set_if_valid(&mut self.search, config.search);
        let _ = Self::set_if_valid(&mut self.status, config.status);
    }

    pub fn load() -> (Self, Option<String>) {
        let mut theme = Self::mocha();
        let Some(config_dir) =
            dirs::config_dir().or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        else {
            return (theme, None);
        };

        let app_dir = config_dir.join("wg-tui");
        let file = app_dir.join("theme.json");

        if !file.exists() {
            let result = (|| -> Result<()> {
                fs::create_dir_all(&app_dir)?;
                fs::write(
                    &file,
                    serde_json::to_string_pretty(&ThemeConfig::default_mocha())?,
                )?;
                Ok(())
            })();
            if let Err(e) = result {
                return (theme, Some(format!("Could not create theme: {e}")));
            }
            return (theme, None);
        }

        match fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))
            .and_then(|s| serde_json::from_str::<ThemeConfig>(&s).context("parsing theme.json"))
        {
            Ok(config) => {
                theme.apply_config(config);
                (theme, None)
            }
            Err(e) => (theme, Some(format!("Theme error: {e}"))),
        }
    }
}

// ============================================================
// Models
// ============================================================

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionType {
    WireGuard,
    OpenVpn,
    Vpn,
    Other,
}

impl ConnectionType {
    fn from_nmcli(value: &str) -> Self {
        let lower = value.to_ascii_lowercase();
        if lower.contains("wireguard") {
            Self::WireGuard
        } else if lower.contains("openvpn") {
            Self::OpenVpn
        } else if lower == "vpn" || lower.ends_with("-vpn") {
            Self::Vpn
        } else {
            Self::Other
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::WireGuard => "WireGuard",
            Self::OpenVpn => "OpenVPN",
            Self::Vpn => "VPN",
            Self::Other => "Other",
        }
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Failed,
}

impl ConnectionState {
    fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Inactive",
            Self::Connecting => "Connecting",
            Self::Connected => "Active",
            Self::Disconnecting => "Disconnecting",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Clone, Debug)]
struct WireGuardStats {
    interface: String,
    #[allow(dead_code)]
    public_key: Option<String>,
    endpoint: Option<String>,
    latest_handshake: Option<SystemTime>,
    rx_bytes: u64,
    tx_bytes: u64,
}

#[derive(Clone, Debug)]
struct VpnConnection {
    name: String,
    conn_type: ConnectionType,
    interface: Option<String>,
    state: ConnectionState,
    autoconnect: bool,
    autoconnect_priority: i32,
    is_default: bool,
    wg: Option<WireGuardStats>,
}

#[derive(Clone, Debug)]
struct IpInfo {
    ip: String,
    city: Option<String>,
    country: Option<String>,
    org: Option<String>,
}

#[derive(Clone, Debug)]
struct Diagnostics {
    ip: Option<IpInfo>,
    latency_ms: Option<f64>,
    route_v4: Option<String>,
    route_v6: Option<String>,
    dns_servers: Vec<String>,
    dns_ok: bool,
    dns_leak: bool,
    ipv6_ok: bool,
    kill_switch: KillSwitchStatus,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KillSwitchStatus {
    Enabled,
    Disabled,
    Unknown,
}

impl KillSwitchStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Enabled => "ON",
            Self::Disabled => "OFF",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum Operation {
    Refreshing,
    Connecting(String),
    Disconnecting(String),
    Importing(PathBuf),
    SettingDefault(String),
    RemovingDefault(String),
    RefreshingDiagnostics,
}

impl Operation {
    fn label(&self) -> String {
        match self {
            Self::Refreshing => "Refreshing".into(),
            Self::Connecting(n) => format!("Connecting {n}"),
            Self::Disconnecting(n) => format!("Disconnecting {n}"),
            Self::Importing(p) => format!("Importing {}", p.display()),
            Self::SettingDefault(n) => format!("Setting default {n}"),
            Self::RemovingDefault(n) => format!("Removing default {n}"),
            Self::RefreshingDiagnostics => "Refreshing diagnostics".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveTab {
    Connections,
    Import,
    Diagnostics,
    Help,
}

impl ActiveTab {
    fn next(self) -> Self {
        match self {
            Self::Connections => Self::Import,
            Self::Import => Self::Diagnostics,
            Self::Diagnostics => Self::Help,
            Self::Help => Self::Connections,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Connections => Self::Help,
            Self::Import => Self::Connections,
            Self::Diagnostics => Self::Import,
            Self::Help => Self::Diagnostics,
        }
    }
}

// ============================================================
// Events
// ============================================================

enum AppEvent {
    Key(KeyEvent),
    ConnectionsUpdated(Vec<VpnConnection>),
    WireGuardStatsUpdated(Vec<WireGuardStats>),
    DownloadsUpdated(Vec<PathBuf>),
    DiagnosticsUpdated(Diagnostics),
    #[allow(dead_code)]
    OperationStarted(Operation),
    OperationFinished(Result<String, String>),
    Error(String),
}

// ============================================================
// Command layer
// ============================================================

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

// async fn run_command( ---Version2
//     program: &'static str,
//     args: Vec<String>,
//     timeout: Duration,
// ) -> Result<CommandOutput> {
//     tokio::task::spawn_blocking(move || {
//         let mut command = Command::new(program);
//         command
//             .args(&args)
//             .stdin(Stdio::null())
//             .stdout(Stdio::piped())
//             .stderr(Stdio::piped());

//         let child = command
//             .spawn()
//             .with_context(|| format!("could not start {program}"))?;

//         let start = std::time::Instant::now();
//         let mut child = child;

//         loop {
//             if let Some(status) = child.try_wait()? {
//                 let output = child.wait_with_output()?;
//                 return Ok(CommandOutput {
//                     stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
//                     stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
//                     success: status.success(),
//                 });
//             }

//             if start.elapsed() >= timeout {
//                 let _ = child.kill();
//                 let _ = child.wait();
//                 return Err(anyhow!("{program} timed out after {}s", timeout.as_secs()));
//             }

//             std::thread::sleep(Duration::from_millis(25));
//         }
//     })
//     .await
//     .context("command worker failed")?
// }

// async fn run_command(
//     program: &'static str,
//     args: Vec<String>,
//     timeout_duration: Duration,
// ) -> Result<CommandOutput> {
//     let mut command = Command::new(program);
//     command
//         .args(&args)
//         .stdin(Stdio::null())
//         .stdout(Stdio::piped())
//         .stderr(Stdio::piped());

//     let mut child = command
//         .spawn()
//         .with_context(|| format!("could not start {program}"))?;

//     // Asynchronously await completion or timeout without blocking worker threads
//     let timeout_future = tokio::time::sleep(timeout_duration);

//     tokio::select! {
//         //.await
//         output_res = child.wait_with_output() => {
//             let output = output_res?;
//             Ok(CommandOutput {
//                 stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
//                 stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
//                 success: output.status.success(),
//             })
//         }
//         _ = timeout_future => {
//             let _ = child.kill().await;
//             Err(anyhow!("{program} timed out after {}s", timeout_duration.as_secs()))
//         }
//     }
// }

async fn run_command(
    program: &'static str,
    args: Vec<String>,
    timeout_duration: Duration,
) -> Result<CommandOutput> {
    let mut command = Command::new(program);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {program}"))?;

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let status;
    let timeout_future = tokio::time::sleep(timeout_duration);

    tokio::select! {
        status_res = child.wait() => {
            status = status_res?;
        }
        _ = timeout_future => {
            let _ = child.kill().await;
            return Err(anyhow!("{program} timed out after {}s", timeout_duration.as_secs()));
        }
    }

    let stdout = if let Some(mut out) = stdout_handle {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut out, &mut buf).await?;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    };

    let stderr = if let Some(mut err) = stderr_handle {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await?;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    };

    Ok(CommandOutput {
        stdout,
        stderr,
        success: status.success(),
    })
}

async fn nmcli(args: Vec<String>) -> Result<String> {
    let out = run_command("nmcli", args, Duration::from_secs(5)).await?;
    if out.success {
        Ok(out.stdout)
    } else {
        let err = if out.stderr.trim().is_empty() {
            out.stdout
        } else {
            out.stderr
        };
        Err(anyhow!("nmcli failed: {}", clean_error(&err)))
    }
}

async fn wg_dump() -> Result<String> {
    let out = run_command(
        "wg",
        vec!["show".into(), "all".into(), "dump".into()],
        Duration::from_secs(3),
    )
    .await?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(anyhow!("wg failed: {}", clean_error(&out.stderr)))
    }
}

fn clean_error(value: &str) -> String {
    value.trim().replace('\n', " ")
}

fn parse_nmcli_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ':' {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }

    if escaped {
        current.push('\\');
    }
    fields.push(current);
    fields
}

// ============================================================
// NetworkManager service
// ============================================================

async fn fetch_connections() -> Result<Vec<VpnConnection>> {
    let all = nmcli(vec![
        "-t".into(),
        "--escape".into(),
        "yes".into(),
        "-f".into(),
        "NAME,TYPE,DEVICE,AUTOCONNECT,AUTOCONNECT-PRIORITY".into(),
        "connection".into(),
        "show".into(),
    ])
    .await?;

    let active = nmcli(vec![
        "-t".into(),
        "--escape".into(),
        "yes".into(),
        "-f".into(),
        "NAME,TYPE,DEVICE".into(),
        "connection".into(),
        "show".into(),
        "--active".into(),
    ])
    .await?;

    let mut active_devices: HashMap<String, String> = HashMap::new();
    for line in active.lines().filter(|l| !l.trim().is_empty()) {
        let f = parse_nmcli_fields(line);
        if f.len() >= 3 {
            active_devices.insert(f[0].clone(), f[2].clone());
        }
    }

    let wg = fetch_wireguard_stats().await.unwrap_or_default();
    let mut connections = Vec::new();

    for line in all.lines().filter(|l| !l.trim().is_empty()) {
        let f = parse_nmcli_fields(line);
        if f.len() < 5 {
            continue;
        }

        let name = f[0].clone();
        let conn_type = ConnectionType::from_nmcli(&f[1]);
        if !matches!(
            conn_type,
            ConnectionType::WireGuard | ConnectionType::OpenVpn | ConnectionType::Vpn
        ) {
            continue;
        }

        let interface = active_devices.get(&name).cloned().filter(|d| d != "--");
        let autoconnect = matches!(f[3].to_ascii_lowercase().as_str(), "yes" | "true");
        let priority = f[4].parse::<i32>().unwrap_or(0);
        let is_default = autoconnect && priority > 0;

        let state = if active_devices.contains_key(&name) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        };

        let wg_stats = interface
            .as_deref()
            .and_then(|dev| wg.iter().find(|s| s.interface == dev).cloned());

        connections.push(VpnConnection {
            name,
            conn_type,
            interface,
            state,
            autoconnect,
            autoconnect_priority: priority,
            is_default,
            wg: wg_stats,
        });
    }

    connections.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(connections)
}

async fn connect_vpn(name: &str) -> Result<()> {
    nmcli(vec![
        "connection".into(),
        "up".into(),
        "id".into(),
        name.into(),
    ])
    .await
    .map(|_| ())
}

async fn disconnect_vpn(name: &str) -> Result<()> {
    nmcli(vec![
        "connection".into(),
        "down".into(),
        "id".into(),
        name.into(),
    ])
    .await
    .map(|_| ())
}

async fn import_wireguard(path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("configuration path is not valid UTF-8"))?;

    nmcli(vec![
        "connection".into(),
        "import".into(),
        "type".into(),
        "wireguard".into(),
        "file".into(),
        path.into(),
    ])
    .await
    .map(|_| ())
}

async fn set_default_vpn(name: &str) -> Result<()> {
    nmcli(vec![
        "connection".into(),
        "modify".into(),
        name.into(),
        "connection.autoconnect".into(),
        "yes".into(),
        "connection.autoconnect-priority".into(),
        "100".into(),
    ])
    .await
    .map(|_| ())
}

async fn unset_default_vpn(name: &str) -> Result<()> {
    nmcli(vec![
        "connection".into(),
        "modify".into(),
        name.into(),
        "connection.autoconnect".into(),
        "no".into(),
        // "connection.autoconnect-priority".into(),
        // "100".into(),
    ])
    .await
    .map(|_| ())
}

// ============================================================
// WireGuard service
// ============================================================

async fn fetch_wireguard_stats() -> Result<Vec<WireGuardStats>> {
    let output = wg_dump().await?;
    let mut interfaces: HashMap<String, WireGuardStats> = HashMap::new();

    for line in output.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();

        if fields.len() == 5 {
            interfaces.insert(
                fields[0].to_string(),
                WireGuardStats {
                    interface: fields[0].to_string(),
                    public_key: Some(fields[2].to_string()),
                    endpoint: None,
                    latest_handshake: None,
                    rx_bytes: 0,
                    tx_bytes: 0,
                },
            );
            continue;
        }

        if fields.len() >= 9 {
            let Some(stats) = interfaces.get_mut(fields[0]) else {
                continue;
            };

            if stats.endpoint.is_none() && fields[3] != "(none)" {
                stats.endpoint = Some(fields[3].to_string());
            }

            if let Ok(seconds) = fields[5].parse::<u64>() {
                if seconds > 0 {
                    stats.latest_handshake = Some(UNIX_EPOCH + Duration::from_secs(seconds));
                }
            }

            stats.rx_bytes = stats
                .rx_bytes
                .saturating_add(fields[6].parse::<u64>().unwrap_or(0));
            stats.tx_bytes = stats
                .tx_bytes
                .saturating_add(fields[7].parse::<u64>().unwrap_or(0));
        }
    }

    Ok(interfaces.into_values().collect())
}

// ============================================================
// Diagnostics
// ============================================================

async fn fetch_ip_info() -> Result<IpInfo> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()?;

    let response = client
        .get("https://ipapi.co/json/")
        .send()
        .await?
        .error_for_status()?;

    #[derive(Deserialize)]
    struct ApiInfo {
        ip: String,
        city: Option<String>,
        country: Option<String>,
        org: Option<String>,
    }

    let info = response.json::<ApiInfo>().await?;
    Ok(IpInfo {
        ip: info.ip,
        city: info.city,
        country: info.country,
        org: info.org,
    })
}

async fn ping_latency() -> Result<f64> {
    let out = run_command(
        "ping",
        vec![
            "-n".into(),
            "-c".into(),
            "1".into(),
            "-W".into(),
            "2".into(),
            "1.1.1.1".into(),
        ],
        Duration::from_secs(3),
    )
    .await?;

    if !out.success {
        return Err(anyhow!("ping failed: {}", clean_error(&out.stderr)));
    }

    for line in out.stdout.lines() {
        if let Some(pos) = line.find("time=") {
            let value = &line[pos + 5..];
            let end = value
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(value.len());
            if let Ok(ms) = value[..end].parse::<f64>() {
                return Ok(ms);
            }
        }
    }

    Err(anyhow!("ping response did not contain RTT"))
}

async fn route_for(target: &str, ipv6: bool) -> Result<String> {
    let mut args = Vec::new();
    if ipv6 {
        args.push("-6".into());
    }
    args.extend(["route".into(), "get".into(), target.into()]);

    let out = run_command("ip", args, Duration::from_secs(2)).await?;
    if !out.success {
        return Err(anyhow!("route lookup failed: {}", clean_error(&out.stderr)));
    }

    Ok(out.stdout.trim().replace('\n', " "))
}

async fn dns_servers() -> Result<Vec<String>> {
    let out = run_command("resolvectl", vec!["dns".into()], Duration::from_secs(2)).await?;

    if !out.success {
        return Err(anyhow!("resolvectl failed"));
    }

    let mut servers = Vec::new();
    for token in out.stdout.split_whitespace() {
        if token.parse::<std::net::IpAddr>().is_ok() {
            servers.push(token.to_string());
        }
    }
    servers.sort();
    servers.dedup();
    Ok(servers)
}

async fn dns_leak_check(vpn_interfaces: &[String]) -> Result<bool> {
    let out = run_command("resolvectl", vec!["status".into()], Duration::from_secs(2)).await?;

    if !out.success {
        return Ok(false);
    }

    let mut current_link = String::new();
    let mut leak = false;

    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Link ") {
            current_link = trimmed
                .split_once('(')
                .and_then(|(_, rhs)| rhs.split_once(')'))
                .map(|(iface, _)| iface.trim().to_string())
                .unwrap_or_default();
            continue;
        }

        if trimmed.starts_with("DNS Servers:") || trimmed.starts_with("DNS Servers ") {
            let values = trimmed.split_once(':').map(|(_, rhs)| rhs).unwrap_or("");
            let is_vpn_link = vpn_interfaces.iter().any(|iface| current_link == *iface);
            if !is_vpn_link
                && values
                    .split_whitespace()
                    .any(|v| v.parse::<std::net::IpAddr>().is_ok())
            {
                leak = true;
            }
        }
    }

    Ok(leak)
}

async fn detect_kill_switch() -> KillSwitchStatus {
    if let Ok(out) = run_command(
        "nft",
        vec!["list".into(), "ruleset".into()],
        Duration::from_secs(3),
    )
    .await
    {
        if out.success {
            let lower = out.stdout.to_ascii_lowercase();
            if lower.contains("kill-switch")
                || lower.contains("killswitch")
                || (lower.contains("wireguard") && lower.contains("drop"))
            {
                return KillSwitchStatus::Enabled;
            }
        }
    }

    if let Ok(out) = run_command("iptables-save", Vec::new(), Duration::from_secs(3)).await {
        if out.success {
            let lower = out.stdout.to_ascii_lowercase();
            if lower.contains("kill-switch")
                || lower.contains("killswitch")
                || (lower.contains("wireguard") && lower.contains("drop"))
            {
                return KillSwitchStatus::Enabled;
            }
        }
    }

    KillSwitchStatus::Unknown
}

async fn collect_diagnostics() -> Result<Diagnostics> {
    let connections = fetch_connections().await.unwrap_or_default();
    let vpn_interfaces = connections
        .iter()
        .filter(|v| v.state == ConnectionState::Connected)
        .filter_map(|v| v.interface.clone())
        .collect::<Vec<_>>();

    let (ip, latency, route_v4, route_v6, dns, dns_leak, kill_switch) = tokio::join!(
        fetch_ip_info(),
        ping_latency(),
        route_for("1.1.1.1", false),
        route_for("2606:4700:4700::1111", true),
        dns_servers(),
        dns_leak_check(&vpn_interfaces),
        detect_kill_switch(),
    );

    let dns_servers = dns.unwrap_or_default();
    let dns_leak = dns_leak.unwrap_or(false);
    let ipv6_ok = route_v6.is_ok();

    Ok(Diagnostics {
        ip: ip.ok(),
        latency_ms: latency.ok(),
        route_v4: route_v4.ok(),
        route_v6: route_v6.ok(),
        dns_ok: !dns_servers.is_empty(),
        dns_leak,
        dns_servers,
        ipv6_ok,
        kill_switch,
    })
}

// ============================================================
// Filesystem service
// ============================================================

fn scan_downloads() -> Result<Vec<PathBuf>> {
    let home = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .ok_or_else(|| anyhow!("could not locate Downloads directory"))?;

    let mut files = Vec::new();
    for entry in fs::read_dir(&home)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let is_config = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("conf") || ext.eq_ignore_ascii_case("wireguard"))
            .unwrap_or(false);

        if is_config {
            files.push(path);
        }
    }

    files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .cmp(
                &b.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
            )
    });

    Ok(files)
}

fn write_log(message: &str) {
    let Some(base) =
        dirs::data_local_dir().or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))
    else {
        return;
    };

    let dir = base.join("wg-tui");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }

    let file = dir.join("wg-tui.log");
    let line = format!(
        "{} {}\n",
        chrono_like_timestamp(),
        message.replace('\n', " ")
    );

    if let Ok(meta) = fs::metadata(&file) {
        if meta.len() > 1_048_576 {
            let _ = fs::rename(&file, dir.join("wg-tui.log.1"));
        }
    }

    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(file) {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

fn chrono_like_timestamp() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".into(),
    }
}

// ============================================================
// App state
// ============================================================

struct App {
    connections: Vec<VpnConnection>,
    filtered: Vec<usize>,
    list_state: ListState,

    downloads: Vec<PathBuf>,
    download_state: ListState,

    tab: ActiveTab,
    search: String,
    searching: bool,

    diagnostics: Option<Diagnostics>,
    diagnostics_loading: bool,

    status: String,
    status_is_error: bool,
    operation: Option<Operation>,

    show_quit: bool,
    show_help: bool,
    show_details: bool,

    should_quit: bool,
    theme: Theme,
    tx: mpsc::UnboundedSender<AppEvent>,
}

impl App {
    fn new(tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        let (theme, theme_error) = Theme::load();

        Self {
            connections: Vec::new(),
            filtered: Vec::new(),
            list_state: ListState::default(),
            downloads: Vec::new(),
            download_state: ListState::default(),
            tab: ActiveTab::Connections,
            search: String::new(),
            searching: false,
            diagnostics: None,
            diagnostics_loading: false,
            status: theme_error.unwrap_or_else(|| "Loading connections...".into()),
            status_is_error: false,
            operation: None,
            show_quit: false,
            show_help: false,
            show_details: false,
            should_quit: false,
            theme,
            tx,
        }
    }

    fn selected_connection(&self) -> Option<VpnConnection> {
        let selected = self.list_state.selected()?;
        let source_index = *self.filtered.get(selected)?;
        self.connections.get(source_index).cloned()
    }

    fn rebuild_filter(&mut self) {
        let q = self.search.trim().to_lowercase();

        self.filtered = self
            .connections
            .iter()
            .enumerate()
            .filter(|(_, vpn)| {
                q.is_empty()
                    || vpn.name.to_lowercase().contains(&q)
                    || vpn
                        .interface
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
                    || vpn.conn_type.label().to_lowercase().contains(&q)
                    || vpn
                        .wg
                        .as_ref()
                        .and_then(|w| w.endpoint.as_deref())
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
            })
            .map(|(i, _)| i)
            .collect();

        match self.list_state.selected() {
            Some(i) if i < self.filtered.len() => {}
            _ => {
                self.list_state
                    .select((!self.filtered.is_empty()).then_some(0));
            }
        }
    }

    fn selected_download(&self) -> Option<PathBuf> {
        self.download_state
            .selected()
            .and_then(|i| self.downloads.get(i).cloned())
    }

    fn next(&mut self) {
        match self.tab {
            ActiveTab::Connections => {
                if self.filtered.is_empty() {
                    return;
                }
                let i = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some((i + 1) % self.filtered.len()));
            }
            ActiveTab::Import => {
                if self.downloads.is_empty() {
                    return;
                }
                let i = self.download_state.selected().unwrap_or(0);
                self.download_state
                    .select(Some((i + 1) % self.downloads.len()));
            }
            _ => {}
        }
    }

    fn previous(&mut self) {
        match self.tab {
            ActiveTab::Connections => {
                if self.filtered.is_empty() {
                    return;
                }
                let i = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some(if i == 0 {
                    self.filtered.len() - 1
                } else {
                    i - 1
                }));
            }
            ActiveTab::Import => {
                if self.downloads.is_empty() {
                    return;
                }
                let i = self.download_state.selected().unwrap_or(0);
                self.download_state.select(Some(if i == 0 {
                    self.downloads.len() - 1
                } else {
                    i - 1
                }));
            }
            _ => {}
        }
    }

    fn busy(&self) -> bool {
        self.operation.is_some()
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_is_error = error;
    }
}

// ============================================================
// Background operations
// ============================================================

fn spawn_refresh(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        match fetch_connections().await {
            Ok(vpns) => {
                let _ = tx.send(AppEvent::ConnectionsUpdated(vpns));
            }
            Err(e) => {
                let _ = tx.send(AppEvent::Error(format!("Refresh failed: {e}")));
            }
        }

        match scan_downloads() {
            Ok(files) => {
                let _ = tx.send(AppEvent::DownloadsUpdated(files));
            }
            Err(e) => {
                let _ = tx.send(AppEvent::Error(format!("Download scan failed: {e}")));
            }
        }
    });
}

fn spawn_diagnostics(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        match collect_diagnostics().await {
            Ok(diagnostics) => {
                let _ = tx.send(AppEvent::DiagnosticsUpdated(diagnostics));
            }
            Err(e) => {
                let _ = tx.send(AppEvent::Error(format!("Diagnostics failed: {e}")));
            }
        }
    });
}

fn spawn_operation(operation: Operation, tx: mpsc::UnboundedSender<AppEvent>) {
    write_log(&format!("START: {}", operation.label()));
    tokio::spawn(async move {
        let result = match &operation {
            Operation::Connecting(name) => connect_vpn(name)
                .await
                .map(|_| format!("Connected to {name}")),
            Operation::Disconnecting(name) => disconnect_vpn(name)
                .await
                .map(|_| format!("Disconnected from {name}")),
            Operation::Importing(path) => import_wireguard(path)
                .await
                .map(|_| format!("Imported {}", path.display())),

            Operation::SettingDefault(name) => set_default_vpn(name)
                .await
                .map(|_| format!("Default VPN set to {name}")),
            Operation::RemovingDefault(name) => unset_default_vpn(name)
                .await
                .map(|_| format!("Default VPN {name} removed")),

            Operation::Refreshing | Operation::RefreshingDiagnostics => {
                Ok("Operation complete".into())
            }
        };

        let _ = tx.send(AppEvent::OperationFinished(
            result.map_err(|e| e.to_string()),
        ));

        spawn_refresh(tx.clone());
        spawn_diagnostics(tx);
    });
}

// ============================================================
// Input
// ============================================================

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    if app.show_quit {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.should_quit = true,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                app.show_quit = false
            }
            _ => {}
        }
        return;
    }

    if app.show_help {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
        ) {
            app.show_help = false;
        }
        return;
    }

    if app.show_details {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter) {
            app.show_details = false;
        }
        return;
    }

    if app.searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.searching = false,
            KeyCode::Backspace => {
                app.search.pop();
                app.rebuild_filter();
            }
            KeyCode::Char(c) => {
                app.search.push(c);
                app.rebuild_filter();
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.show_quit = true,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char('/') if app.tab == ActiveTab::Connections => app.searching = true,

        KeyCode::Char('1') => app.tab = ActiveTab::Connections,
        KeyCode::Char('2') => app.tab = ActiveTab::Import,
        KeyCode::Char('3') => app.tab = ActiveTab::Diagnostics,
        KeyCode::Char('4') => app.tab = ActiveTab::Help,

        KeyCode::Tab => app.tab = app.tab.next(),
        KeyCode::BackTab => app.tab = app.tab.prev(),

        KeyCode::Char('h') => app.tab = app.tab.prev(),
        KeyCode::Char('l') if app.tab != ActiveTab::Connections => app.tab = app.tab.next(),

        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),

        KeyCode::Char('r') if !app.busy() => {
            let (theme, error) = Theme::load();
            app.theme = theme;
            if let Some(error) = error {
                app.set_status(error, true);
            } else {
                app.set_status("Refreshing...", false);
            }
            spawn_refresh(app.tx.clone());
            spawn_diagnostics(app.tx.clone());
        }

        KeyCode::Char('i') if !app.busy() => {
            app.diagnostics_loading = true;
            app.set_status("Refreshing diagnostics...", false);
            spawn_diagnostics(app.tx.clone());
        }

        KeyCode::Char('d') if app.tab == ActiveTab::Connections && !app.busy() => {
            if let Some(vpn) = app.selected_connection() {
                app.operation = Some(Operation::SettingDefault(vpn.name.clone()));
                app.set_status(format!("Setting {} as default...", vpn.name), false);
                spawn_operation(Operation::SettingDefault(vpn.name), app.tx.clone());
            }
        }

        KeyCode::Char('D') if app.tab == ActiveTab::Connections && !app.busy() => {
            if let Some(vpn) = app.selected_connection() {
                app.operation = Some(Operation::RemovingDefault(vpn.name.clone()));
                app.set_status(format!("Removing {} as default ...", vpn.name), false);
                spawn_operation(Operation::RemovingDefault(vpn.name), app.tx.clone());
            }
        }

        KeyCode::Char(' ') if app.tab == ActiveTab::Connections => {
            if app.selected_connection().is_some() {
                app.show_details = true;
            }
        }

        KeyCode::Enter | KeyCode::Char('l') if !app.busy() => match app.tab {
            ActiveTab::Connections => {
                if let Some(vpn) = app.selected_connection() {
                    let operation = if vpn.state == ConnectionState::Connected {
                        Operation::Disconnecting(vpn.name.clone())
                    } else {
                        Operation::Connecting(vpn.name.clone())
                    };
                    app.operation = Some(operation.clone());
                    app.set_status(format!("{}...", operation.label()), false);
                    spawn_operation(operation, app.tx.clone());
                }
            }
            ActiveTab::Import => {
                if let Some(path) = app.selected_download() {
                    let operation = Operation::Importing(path.clone());
                    app.operation = Some(operation.clone());
                    app.set_status(format!("Importing {}...", path.display()), false);
                    spawn_operation(operation, app.tx.clone());
                }
            }
            ActiveTab::Diagnostics => spawn_diagnostics(app.tx.clone()),
            ActiveTab::Help => {}
        },

        _ => {}
    }
}

// ============================================================
// Rendering helpers
// ============================================================

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height.min(area.height.saturating_sub(2))),
            Constraint::Fill(1),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width.min(area.width.saturating_sub(2))),
            Constraint::Fill(1),
        ])
        .split(vertical[1])[1]
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn handshake_age(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "--".into();
    };

    match SystemTime::now().duration_since(time) {
        Ok(age) if age.as_secs() < 60 => format!("{}s ago", age.as_secs()),
        Ok(age) if age.as_secs() < 3600 => format!("{}m ago", age.as_secs() / 60),
        Ok(age) => format!("{}h ago", age.as_secs() / 3600),
        Err(_) => "--".into(),
    }
}

// ============================================================
// Rendering
// ============================================================

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

    match app.tab {
        ActiveTab::Connections => render_connections(f, app, chunks[1]),
        ActiveTab::Import => render_import(f, app, chunks[1]),
        ActiveTab::Diagnostics => render_diagnostics(f, app, chunks[1]),
        ActiveTab::Help => render_help(f, app, chunks[1]),
    }

    draw_status(f, app, chunks[2]);

    if app.show_quit {
        draw_quit(f, app);
    }
    if app.show_help {
        draw_help_popup(f, app);
    }
    if app.show_details {
        draw_details_popup(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let tab = |num: &str, label: &str, active: bool| {
        if active {
            Span::styled(
                format!(" [{num}: {label}] "),
                Style::default()
                    .fg(app.theme.active)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" {num}: {label} "),
                Style::default().fg(app.theme.text),
            )
        }
    };

    let title = Line::from(vec![
        Span::styled(
            " View: ",
            Style::default()
                .fg(app.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        tab("1", "Connections", app.tab == ActiveTab::Connections),
        Span::raw("│"),
        tab("2", "Import", app.tab == ActiveTab::Import),
        Span::raw("│"),
        tab("3", "Diagnostics", app.tab == ActiveTab::Diagnostics),
        Span::raw("│"),
        tab("4", "Help", app.tab == ActiveTab::Help),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " ⚡ WireGuard / VPN Manager ",
            Style::default().fg(app.theme.title),
        ));

    f.render_widget(Paragraph::new(title).block(block), area);
}

fn render_connections(f: &mut Frame, app: &mut App, area: Rect) {
    // Vertical responsive layout with search bar, connection list, and bottom details panel
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Search bar
            Constraint::Min(6),     // Connection List
            Constraint::Length(10), // Connection Details (at the bottom)
        ])
        .split(area);

    let border = if app.searching {
        app.theme.search
    } else {
        app.theme.border
    };

    let search_title = if app.searching {
        format!(" Search: {} ", app.search)
    } else {
        " Search (/) ".into()
    };

    let search = Paragraph::new(format!(" / {}", app.search))
        .style(Style::default().fg(app.theme.text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(Span::styled(
                    search_title,
                    Style::default().fg(app.theme.title),
                )),
        );
    f.render_widget(search, chunks[0]);

    let items = app
        .filtered
        .iter()
        .filter_map(|&idx| app.connections.get(idx))
        .map(|vpn| {
            let (symbol, color) = match vpn.state {
                ConnectionState::Connected => ("● Active", app.theme.active),
                ConnectionState::Connecting | ConnectionState::Disconnecting => {
                    ("⟳ Busy", app.theme.highlight_fg)
                }
                ConnectionState::Failed => ("✗ Failed", app.theme.inactive),
                ConnectionState::Disconnected => ("○ Inactive", app.theme.inactive),
            };

            let default = if vpn.is_default {
                Span::styled(
                    " ★ DEFAULT",
                    Style::default()
                        .fg(app.theme.default_tag)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            };

            let interface = vpn.interface.as_deref().unwrap_or("--");
            let line = Line::from(vec![
                Span::styled(
                    format!(" {:<28} ", truncate(&vpn.name, 28)),
                    Style::default()
                        .fg(app.theme.text)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<10} ", vpn.conn_type.label()),
                    Style::default().fg(app.theme.accent),
                ),
                Span::styled(format!("{symbol:<13}"), Style::default().fg(color)),
                Span::styled(
                    format!(" {interface:<7}"),
                    Style::default().fg(app.theme.status),
                ),
                default,
            ]);

            ListItem::new(line)
        })
        .collect::<Vec<_>>();

    let title = format!(
        " VPN Profiles ({}/{}) ",
        app.filtered.len(),
        app.connections.len()
    );

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(title, Style::default().fg(app.theme.title))),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.highlight_bg)
                .fg(app.theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, chunks[1], &mut app.list_state);

    if !app.filtered.is_empty() {
        let mut state = ScrollbarState::new(app.filtered.len().saturating_sub(1))
            .position(app.list_state.selected().unwrap_or(0));
        f.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(app.theme.border)),
            chunks[1].inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut state,
        );
    }

    render_connection_details_bottom(f, app, chunks[2]);
}

fn render_connection_details_bottom(f: &mut Frame, app: &App, area: Rect) {
    let Some(vpn) = app.selected_connection() else {
        let p = Paragraph::new(Span::styled(
            " No VPN selected. Use j/k to navigate.",
            Style::default().fg(app.theme.status),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(
                    " Connection Details ",
                    Style::default().fg(app.theme.title),
                )),
        );
        f.render_widget(p, area);
        return;
    };

    let state_color = if vpn.state == ConnectionState::Connected {
        app.theme.active
    } else {
        app.theme.inactive
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(" Status: ", Style::default().fg(app.theme.title)),
        Span::styled(
            format!("{:<12} ", vpn.state.label()),
            Style::default().fg(state_color),
        ),
        Span::styled(" Type: ", Style::default().fg(app.theme.title)),
        Span::styled(
            format!("{:<10} ", vpn.conn_type.label()),
            Style::default().fg(app.theme.accent),
        ),
        Span::styled(" Interface: ", Style::default().fg(app.theme.title)),
        Span::styled(
            format!("{:<8} ", vpn.interface.as_deref().unwrap_or("--")),
            Style::default().fg(app.theme.text),
        ),
        Span::styled(" Autoconnect: ", Style::default().fg(app.theme.title)),
        Span::styled(
            format!("{:<5} ", if vpn.autoconnect { "yes" } else { "no" }),
            Style::default().fg(app.theme.text),
        ),
        Span::styled(" Priority: ", Style::default().fg(app.theme.title)),
        Span::styled(
            format!("{} ", vpn.autoconnect_priority),
            Style::default().fg(app.theme.text),
        ),
    ])];

    if let Some(wg) = &vpn.wg {
        lines.push(Line::from(vec![
            Span::styled(" Endpoint: ", Style::default().fg(app.theme.title)),
            Span::styled(
                format!("{:<22} ", wg.endpoint.as_deref().unwrap_or("--")),
                Style::default().fg(app.theme.text),
            ),
            Span::styled(" Handshake: ", Style::default().fg(app.theme.title)),
            Span::styled(
                format!("{:<10} ", handshake_age(wg.latest_handshake)),
                Style::default().fg(app.theme.text),
            ),
            Span::styled(" RX: ", Style::default().fg(app.theme.title)),
            Span::styled(
                format!("{:<8} ", format_bytes(wg.rx_bytes)),
                Style::default().fg(app.theme.text),
            ),
            Span::styled(" TX: ", Style::default().fg(app.theme.title)),
            Span::styled(
                format!("{}", format_bytes(wg.tx_bytes)),
                Style::default().fg(app.theme.text),
            ),
        ]));
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border))
            .title(Span::styled(
                " Connection Details ",
                Style::default().fg(app.theme.title),
            )),
    );

    f.render_widget(p, area);
}

fn render_import(f: &mut Frame, app: &mut App, area: Rect) {
    let items = app
        .downloads
        .iter()
        .map(|file| {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            let installed = app.connections.iter().any(|vpn| {
                vpn.name.eq_ignore_ascii_case(
                    file.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref(),
                )
            });

            let status = if installed {
                Span::styled("✔ Installed", Style::default().fg(app.theme.active))
            } else {
                Span::styled("○ Ready", Style::default().fg(app.theme.text))
            };

            ListItem::new(Line::from(vec![
                Span::styled(" 📄 ", Style::default().fg(app.theme.accent)),
                Span::styled(
                    format!("{:<42} ", truncate(&name, 42)),
                    Style::default().fg(app.theme.text),
                ),
                status,
            ]))
        })
        .collect::<Vec<_>>();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(
                    " WireGuard Configurations in ~/Downloads ",
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

    f.render_stateful_widget(list, area, &mut app.download_state);

    if !app.downloads.is_empty() {
        let mut state = ScrollbarState::new(app.downloads.len().saturating_sub(1))
            .position(app.download_state.selected().unwrap_or(0));
        f.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(app.theme.border)),
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut state,
        );
    }
}

fn render_diagnostics(f: &mut Frame, app: &App, area: Rect) {
    let Some(d) = &app.diagnostics else {
        let text = if app.diagnostics_loading {
            " Collecting diagnostics..."
        } else {
            " Diagnostics unavailable. Press i or Enter to retry."
        };

        f.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(app.theme.highlight_fg))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(app.theme.border))
                        .title(Span::styled(
                            " Network Diagnostics ",
                            Style::default().fg(app.theme.title),
                        )),
                ),
            area,
        );
        return;
    };

    let ip = d.ip.as_ref();
    let ip_value = ip.map(|v| v.ip.as_str()).unwrap_or("--");
    let country = ip.and_then(|v| v.country.as_deref()).unwrap_or("--");
    let city = ip.and_then(|v| v.city.as_deref()).unwrap_or("--");
    let org = ip.and_then(|v| v.org.as_deref()).unwrap_or("--");

    let latency = d
        .latency_ms
        .map(|v| format!("{v:.1} ms"))
        .unwrap_or_else(|| "--".into());

    let v4 = d.route_v4.as_deref().unwrap_or("unavailable");
    let v6 = d.route_v6.as_deref().unwrap_or("unavailable");

    let dns = if d.dns_servers.is_empty() {
        "none detected".into()
    } else {
        d.dns_servers.join(", ")
    };

    let dns_style = if d.dns_leak {
        app.theme.inactive
    } else if d.dns_ok {
        app.theme.active
    } else {
        app.theme.inactive
    };

    let ipv6_style = if d.ipv6_ok {
        app.theme.active
    } else {
        app.theme.inactive
    };

    let kill_style = match d.kill_switch {
        KillSwitchStatus::Enabled => app.theme.active,
        KillSwitchStatus::Disabled => app.theme.inactive,
        KillSwitchStatus::Unknown => app.theme.status,
    };

    let lines = vec![
        Line::from(Span::styled(
            " Public identity",
            Style::default()
                .fg(app.theme.active)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(" IP           ", Style::default().fg(app.theme.title)),
            Span::styled(ip_value, Style::default().fg(app.theme.active)),
        ]),
        Line::from(vec![
            Span::styled(" Country      ", Style::default().fg(app.theme.title)),
            Span::styled(country, Style::default().fg(app.theme.text)),
        ]),
        Line::from(vec![
            Span::styled(" City         ", Style::default().fg(app.theme.title)),
            Span::styled(city, Style::default().fg(app.theme.text)),
        ]),
        Line::from(vec![
            Span::styled(" ISP / Org    ", Style::default().fg(app.theme.title)),
            Span::styled(org, Style::default().fg(app.theme.accent)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Connectivity",
            Style::default()
                .fg(app.theme.active)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(" Latency      ", Style::default().fg(app.theme.title)),
            Span::styled(latency, Style::default().fg(app.theme.accent)),
        ]),
        Line::from(vec![
            Span::styled(" IPv4 route   ", Style::default().fg(app.theme.title)),
            Span::styled(truncate(v4, 70), Style::default().fg(app.theme.text)),
        ]),
        Line::from(vec![
            Span::styled(" IPv6 route   ", Style::default().fg(app.theme.title)),
            Span::styled(truncate(v6, 70), Style::default().fg(ipv6_style)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " DNS / firewall",
            Style::default()
                .fg(app.theme.active)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(" DNS          ", Style::default().fg(app.theme.title)),
            Span::styled(
                if d.dns_leak {
                    "POTENTIAL LEAK"
                } else if d.dns_ok {
                    "configured"
                } else {
                    "none detected"
                },
                Style::default().fg(dns_style),
            ),
        ]),
        Line::from(vec![
            Span::styled(" Servers      ", Style::default().fg(app.theme.title)),
            Span::styled(truncate(&dns, 70), Style::default().fg(app.theme.text)),
        ]),
        Line::from(vec![
            Span::styled(" IPv6         ", Style::default().fg(app.theme.title)),
            Span::styled(
                if d.ipv6_ok {
                    "available"
                } else {
                    "unavailable"
                },
                Style::default().fg(ipv6_style),
            ),
        ]),
        Line::from(vec![
            Span::styled(" Kill switch  ", Style::default().fg(app.theme.title)),
            Span::styled(d.kill_switch.label(), Style::default().fg(kill_style)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border))
                .title(Span::styled(
                    " Network Diagnostics ",
                    Style::default().fg(app.theme.title),
                )),
        ),
        area,
    );
}

fn render_help(f: &mut Frame, app: &App, area: Rect) {
    let key = Style::default()
        .fg(app.theme.highlight_fg)
        .add_modifier(Modifier::BOLD);
    let text = Style::default().fg(app.theme.text);
    let section = Style::default()
        .fg(app.theme.active)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

    let lines = vec![
        Line::from(Span::styled(" Navigation", section)),
        Line::from(vec![
            Span::styled("  1 / 2 / 3 / 4  ", key),
            Span::styled("Switch views", text),
        ]),
        Line::from(vec![
            Span::styled("  Tab / h / l    ", key),
            Span::styled("Move between views", text),
        ]),
        Line::from(vec![
            Span::styled("  j / k / ↑ / ↓  ", key),
            Span::styled("Move through lists", text),
        ]),
        Line::from(vec![
            Span::styled("  Space          ", key),
            Span::styled("Open popup details modal", text),
        ]),
        Line::from(""),
        Line::from(Span::styled(" VPN operations", section)),
        Line::from(vec![
            Span::styled("  Enter / l      ", key),
            Span::styled("Connect / disconnect or import", text),
        ]),
        Line::from(vec![
            Span::styled("  d              ", key),
            Span::styled("Set selected VPN as preferred autoconnect profile", text),
        ]),
        Line::from(vec![
            Span::styled(" Shift + d        ", key),
            Span::styled("Unset selected VPN as preferred autoconnect profile", text),
        ]),
        Line::from(vec![
            Span::styled("  /              ", key),
            Span::styled("Search VPNs", text),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.theme.border))
                    .title(Span::styled(
                        " User Manual & Shortcuts ",
                        Style::default().fg(app.theme.title),
                    )),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let status_color = if app.status_is_error {
        app.theme.inactive
    } else {
        app.theme.status
    };

    let operation = app
        .operation
        .as_ref()
        .map(|o| format!("{}  ", o.label()))
        .unwrap_or_default();

    let line = Line::from(vec![
        Span::styled(
            format!("[{}{}] ", operation, app.status),
            Style::default().fg(status_color),
        ),
        Span::styled(
            "[Enter]",
            Style::default()
                .fg(app.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Action  ", Style::default().fg(app.theme.text)),
        Span::styled(
            "[Space]",
            Style::default()
                .fg(app.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Details Popup  ", Style::default().fg(app.theme.text)),
        Span::styled(
            "[r]",
            Style::default()
                .fg(app.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Refresh  ", Style::default().fg(app.theme.text)),
        Span::styled(
            "[?]",
            Style::default()
                .fg(app.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Help  ", Style::default().fg(app.theme.text)),
        Span::styled(
            "[q]",
            Style::default()
                .fg(app.theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Quit", Style::default().fg(app.theme.text)),
    ]);

    f.render_widget(
        Paragraph::new(line)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.theme.border))
                    .title(Span::styled(
                        " Status ",
                        Style::default().fg(app.theme.title),
                    )),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_quit(f: &mut Frame, app: &App) {
    let area = centered_rect(48, 7, f.area());
    f.render_widget(Clear, area);

    let content = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Are you sure you want to quit?",
            Style::default()
                .fg(app.theme.text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("[Y] Yes", Style::default().fg(app.theme.active)),
            Span::raw("     "),
            Span::styled("[N] No", Style::default().fg(app.theme.inactive)),
        ]),
    ])
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border))
            .title(Span::styled(" Quit ", Style::default().fg(app.theme.title))),
    );

    f.render_widget(content, area);
}

fn draw_help_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(72, 20, f.area());
    f.render_widget(Clear, area);

    let text = vec![
        Line::from(Span::styled(
            "Quick Help",
            Style::default()
                .fg(app.theme.active)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Enter / l  connect, disconnect, or import"),
        Line::from("Space       open detailed modal popup"),
        Line::from("d           set preferred autoconnect VPN"),
        Line::from("shift+d     disable autoconnect to VPN"),
        Line::from("/           search connections"),
        Line::from("j/k         navigate lists"),
        Line::from("Tab         next view"),
        Line::from("r           reload theme + refresh"),
        Line::from("i           refresh diagnostics"),
        Line::from("? / Esc     close help"),
        Line::from("q           quit"),
    ];

    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.theme.border))
                    .title(Span::styled(" Help ", Style::default().fg(app.theme.title))),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_details_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 18, f.area());
    f.render_widget(Clear, area);

    let Some(vpn) = app.selected_connection() else {
        return;
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Name         ", Style::default().fg(app.theme.title)),
            Span::styled(&vpn.name, Style::default().fg(app.theme.text)),
        ]),
        Line::from(vec![
            Span::styled("Status       ", Style::default().fg(app.theme.title)),
            Span::styled(vpn.state.label(), Style::default().fg(app.theme.active)),
        ]),
        Line::from(vec![
            Span::styled("Type         ", Style::default().fg(app.theme.title)),
            Span::styled(vpn.conn_type.label(), Style::default().fg(app.theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("Interface    ", Style::default().fg(app.theme.title)),
            Span::styled(
                vpn.interface.as_deref().unwrap_or("--"),
                Style::default().fg(app.theme.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Autoconnect  ", Style::default().fg(app.theme.title)),
            Span::styled(
                if vpn.autoconnect { "yes" } else { "no" },
                Style::default().fg(app.theme.text),
            ),
        ]),
    ];

    if let Some(wg) = vpn.wg {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                "WireGuard",
                Style::default()
                    .fg(app.theme.active)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(
                "Endpoint     {}",
                wg.endpoint.as_deref().unwrap_or("--")
            )),
            Line::from(format!(
                "Handshake    {}",
                handshake_age(wg.latest_handshake)
            )),
            Line::from(format!("RX           {}", format_bytes(wg.rx_bytes))),
            Line::from(format!("TX           {}", format_bytes(wg.tx_bytes))),
        ]);
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.theme.border))
                    .title(Span::styled(
                        " Connection Details Modal ",
                        Style::default().fg(app.theme.title),
                    )),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}

// ============================================================
// Terminal guard / main loop
// ============================================================

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = TerminalGuard::enter()?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(tx.clone());

    spawn_refresh(tx.clone());
    spawn_diagnostics(tx.clone());

    let monitor_tx = tx.clone();
    tokio::spawn(async move {
        let mut wg_interval = tokio::time::interval(Duration::from_secs(2));
        let mut diagnostics_interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            tokio::select! {
                _ = wg_interval.tick() => {
                    match fetch_wireguard_stats().await {
                        Ok(stats) => {
                            let _ = monitor_tx.send(AppEvent::WireGuardStatsUpdated(stats));
                        }
                        Err(e) => write_log(&format!("WireGuard monitor: {e}")),
                    }
                }
                _ = diagnostics_interval.tick() => {
                    match collect_diagnostics().await {
                        Ok(diagnostics) => {
                            let _ = monitor_tx.send(AppEvent::DiagnosticsUpdated(diagnostics));
                        }
                        Err(e) => write_log(&format!("Diagnostics monitor: {e}")),
                    }
                }
            }
        }
    });

    let event_tx = tx.clone();
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        loop {
            match reader.next().await {
                Some(Ok(CrosstermEvent::Key(key))) => {
                    if event_tx.send(AppEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = event_tx.send(AppEvent::Error(format!("Input error: {e}")));
                    break;
                }
                None => break,
            }
        }
    });

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        let Some(event) = rx.recv().await else {
            break;
        };

        match event {
            AppEvent::Key(key) => handle_key(&mut app, key),

            AppEvent::ConnectionsUpdated(connections) => {
                app.connections = connections;
                app.rebuild_filter();
            }

            AppEvent::WireGuardStatsUpdated(stats) => {
                for vpn in &mut app.connections {
                    vpn.wg = vpn
                        .interface
                        .as_deref()
                        .and_then(|iface| stats.iter().find(|s| s.interface == iface).cloned());
                }
                app.rebuild_filter();
            }

            AppEvent::DownloadsUpdated(files) => {
                app.downloads = files;
                if app.downloads.is_empty() {
                    app.download_state.select(None);
                } else if app
                    .download_state
                    .selected()
                    .map(|i| i >= app.downloads.len())
                    .unwrap_or(true)
                {
                    app.download_state.select(Some(0));
                }
            }

            AppEvent::DiagnosticsUpdated(diagnostics) => {
                app.diagnostics = Some(diagnostics);
                app.diagnostics_loading = false;
            }

            AppEvent::OperationFinished(result) => {
                app.operation = None;
                match result {
                    Ok(message) => {
                        write_log(&message);
                        app.set_status(message, false);
                    }
                    Err(error) => {
                        write_log(&format!("ERROR: {error}"));
                        app.set_status(error, true);
                    }
                }
            }

            AppEvent::Error(error) => {
                write_log(&format!("ERROR: {error}"));
                app.operation = None;
                app.diagnostics_loading = false;
                app.set_status(error, true);
            }

            AppEvent::OperationStarted(operation) => {
                app.operation = Some(operation);
            }
        }

        if app.should_quit {
            break;
        }
    }

    terminal.show_cursor()?;
    Ok(())
}
