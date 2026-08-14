# ⚡ wg-tui — WireGuard & VPN Manager TUI

An asynchronous Terminal User Interface (TUI) for managing NetworkManager WireGuard and VPN connections, built with **Rust**, **Ratatui**, and **Tokio**.

<img width="678" height="355" alt="image" src="https://github.com/user-attachments/assets/a78ec667-e996-46a2-a4bb-4dd8dd32d243" />


---

## ✨ Features

* **NetworkManager Integration**: Connect, disconnect, and switch WireGuard/VPN profiles directly using `nmcli`.
* **Instant Profile Filtering**: Quick real-time search and filter through connection lists (`/`).
* **Automatic Config Import**: Scans your `~/Downloads` directory for `.conf` and `.wireguard` files to import with one press.
* **GeoIP & Routing Details**: View your public IP address, city, country, and ISP (`i`).
* **Live Latency Ping**: Monitored real-time ping latency check to `1.1.1.1`.
* **Default Profile Setting**: Set high-priority autoconnect default VPN profiles with `d`.
* **Custom Theme System**: Comes pre-configured with a Catppuccin Mocha theme that can be reloaded on the fly (`r`).
* **Full Keyboard Navigation**: Clean keybindings, modal overlays for quit confirmation, and dynamic status bars.

---

## 📋 Prerequisites

Before running `wg-tui`, make sure your Linux system has the following dependencies available:

* **NetworkManager** (`nmcli` command-line utility)
* **iputils** (`ping` utility)
* **Rust toolchain** (Cargo 1.70+)

---

## 🚀 Installation & Building

1. **Clone the repository:**
   ```bash
   git clone https://github.com/your-username/wg-tui.git
   cd wg-tui
   ```

2. **Build and run:**
   ```bash
   cargo run --release
   ```

3. **Install system-wide (optional):**
   ```bash
   cargo install --path .
   ```

---

## ⌨️ Shortcuts & Keybindings

| Key | Action |
| --- | --- |
| `1` / `2` / `3` / `4` | Switch directly between **Connections**, **Import**, **GeoIP**, and **Help** tabs |
| `j` / `k` or `↓` / `↑` | Scroll through configuration list |
| `Enter` / `l` | Toggle active VPN state or import selected `.conf` file |
| `d` | Mark selected VPN profile as default autoconnect |
| `/` | Search and filter connections |
| `r` | Hot-reload `theme.json` and refresh active connection lists |
| `i` | Fetch / force refresh public GeoIP details |
| `?` | Toggle quick help modal overlay |
| `q` / `Esc` | Open quit confirmation dialog |

---

## 🎨 Theme Customization

`wg-tui` automatically generates a JSON theme configuration file upon first launch at:

```text
~/.config/wg-tui/theme.json
```

You can customize hex colors for borders, titles, active/inactive items, highlights, and search bars. Press `r` in the app to hot-reload your modifications without restarting.

### Default Configuration Structure

```json
{
  "border": "#cba6f7",
  "title": "#f5c2e7",
  "active": "#a6e3a1",
  "inactive": "#f38ba8",
  "text": "#cdd6f4",
  "highlight_bg": "#313244",
  "highlight_fg": "#f9e2af",
  "accent": "#89b4fa",
  "default_tag": "#fab387",
  "search": "#74c7ec",
  "status": "#a6adc8"
}
```

---

## 📄 License

Distributed under the MIT License.
