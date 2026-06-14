# Install Prerequisites

Pushin needs Node, Rust, and the platform-specific Tauri build dependencies.

## macOS

```bash
xcode-select --install
brew install node
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Windows

Install:

- Microsoft C++ Build Tools with "Desktop development with C++"
- WebView2 Runtime, if it is not already present
- Rust from [rustup.rs](https://rustup.rs/)
- Node 18 or newer from [nodejs.org](https://nodejs.org/)

## Linux

Debian/Ubuntu:

```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install Node 18 or newer through your package manager or `nvm`.

## Verify

```bash
node --version
cargo --version
```
