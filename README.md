# subman

A TUI (Terminal User Interface) tool for managing vmess subscription nodes.

![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)
![License](https://img.shields.io/badge/license-MIT-blue.svg)

## Features

- 📥 **Subscription Management** - Fetch and parse vmess subscription URLs
- ⚡ **Latency Testing** - TCP connection test and HTTP proxy test with parallel execution
- 🔄 **Xray Integration** - Automatically generate xray config and restart the service
- 📊 **Sorting** - Sort nodes by name, TCP latency, or HTTP latency
- 💾 **Persistence** - Save subscription URL, nodes, and latency results to config file
- 🎨 **Beautiful TUI** - Clean terminal interface built with ratatui

## Installation

### Prerequisites

- Rust 1.70 or later
- [xray](https://github.com/XTLS/Xray-core) installed and running as a service

### Build from source

```bash
git clone https://github.com/caiguanhao/subman.git
cd subman
cargo build --release
```

The binary will be available at `target/release/subman`.

## Usage

```bash
subman [OPTIONS]
```

### Options

| Option | Description | Default |
|--------|-------------|---------|
| `-p, --parallel <N>` | Number of parallel latency tests | 10 |
| `-c, --config <PATH>` | Path to xray config file | `/opt/homebrew/etc/xray/config.json` |

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `↑` / `k` | Move selection up |
| `↓` / `j` | Move selection down |
| `Enter` | Apply selected node (save config & restart xray) |
| `r` / `R` | Refresh subscription |
| `t` | Run TCP latency test |
| `T` | Run HTTP latency test |
| `s` | Cycle sort column (None → TCP → HTTP → Name) |
| `S` | Toggle sort direction |
| `u` / `U` | Set subscription URL |
| `q` / `Q` | Quit |
| `Ctrl+C` | Cancel ongoing test / Quit |

## Configuration

Configuration is stored at `~/.config/subman.json` and includes:

- Subscription URL
- Cached nodes with latency results
- Sort preferences

## How It Works

1. **Subscription Fetching**: Downloads base64-encoded vmess subscription content and parses `vmess://` links
2. **TCP Latency Test**: Direct TCP connection to each node's address and port
3. **HTTP Latency Test**: Starts a temporary xray instance for each node and tests HTTP connectivity through the SOCKS5 proxy
4. **Applying Nodes**: Generates xray config and sends SIGHUP to reload the service

## Screenshots

```
┌─ Subscription Manager ───────────────────────────────────────────────────┐
│   Name                  Address                   Port      TCP     HTTP │
│ ● Node-US-1             us1.example.com           443       45ms    320ms│
│   Node-JP-2             jp2.example.com           443       120ms   450ms│
│   Node-SG-3             sg3.example.com           8443      85ms    380ms│
└──────────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────────────────┐
│ Loaded 3 nodes. Press R to refresh, t/T to test.                         │
└──────────────────────────────────────────────────────────────────────────┘
```

## License

MIT License - see [LICENSE](LICENSE) for details.

