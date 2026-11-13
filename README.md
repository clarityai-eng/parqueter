# Parqueter

# What's this project and how to use it
A fast, native Parquet file viewer built with Rust and [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the GPU-accelerated UI framework from the [Zed](https://zed.dev) editor team).

## Screenshot

![Parqueter screenshot](resources/screenshot.png)

## Install

### macOS and Linux

```bash
curl -fsSL https://raw.githubusercontent.com/clarityai-eng/parqueter/master/install.sh | sh
```

On macOS this installs `Parqueter.app` into `~/Applications`, so it shows up in Spotlight and Launchpad and can open `.parquet` files from Finder. It also links the `parqueter` command into `~/.local/bin`. On Linux it installs the `parqueter` binary into `~/.local/bin`.

To make Parqueter the default app for `.parquet` files: in Finder, select any `.parquet` file, open **Get Info** (Cmd+I), choose Parqueter under **Open with**, and click **Change All…**.

> **Why a script and not a DMG?** The app is not signed with an Apple Developer ID, so a browser download would trigger the "Apple could not verify…" Gatekeeper dialog. Files downloaded with `curl` are not quarantined, so the script avoids it.

### Manual download

Take the latest build for your platform in [Releases](https://github.com/clarityai-eng/parqueter/releases).


# Development
## Requirements

- **Rust nightly** (GPUI requires unstable features)
- **Linux**: `libxkbcommon-dev`, `libxkbcommon-x11-dev`
- **macOS**: Xcode + command line tools
- **Windows**: Win32/DirectWrite (untested)

## Build & Run

```bash
# Install system dependencies (Linux)
sudo apt-get install -y libxkbcommon-dev libxkbcommon-x11-dev

# Build
cargo build --release

# Run
./target/release/parqueter path/to/file.parquet
```

On macOS without full Xcode (Command Line Tools only), GPUI cannot precompile its Metal shaders. Build with runtime shader compilation instead:

```bash
cargo build --release --features gpui_platform/runtime_shaders
```

To build the `.app` bundle locally: `scripts/bundle-macos.sh target/release/parqueter 0.0.0-dev target/bundle`

Note: file paths with spaces must be quoted: `./target/release/parqueter "path/to/my file.parquet"`

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Arrow Up/Down | Scroll 20 rows |
| Page Up/Down | Scroll 200 rows |
| Arrow Left/Right | Scroll horizontally |
| Ctrl+C | Copy selected cell to clipboard |
| Ctrl+O | Open file picker |
| Double-click | Select cell + copy to clipboard |

## Architecture

```
src/
├── main.rs   — CLI argument parsing, GPUI app launch
├── data.rs   — DataSource trait, LazyParquetSource with LRU cache
└── ui.rs     — GPUI views, virtualized table rendering, scrollbars
```

**Key design decisions:**

- **`DataSource` trait** abstracts data access — the UI doesn't know if data comes from memory or disk
- **`LazyParquetSource`** reads Parquet row groups on demand, caches them in an LRU (10 batches), and prefetches N±1 on cache miss
- **Column virtualization** — only columns overlapping the viewport are rendered (typically 3-5 of 137)
- **Row virtualization** — GPUI's `uniform_list` renders only visible rows
- **Deferred loading during drag** — scrollbar dragging skips I/O for instant response, loads data after 100ms idle or on release

## License

MIT
