# claude-meter

A tiny Windows tray app, written in Rust, that always shows how much of your Claude plan's usage you have left: the 5-hour session limit and the weekly limit. It works whether you chat in the Claude desktop app or on claude.ai.

> **Status: work in progress.** Milestone 1 (sign-in, API client, `--once` command) is done. The tray icon and floating badge are next.

## Goals
- Under 10 MB of RAM and about 0% CPU when idle: no async runtime and no GUI framework.
- One-time sign-in in a small claude.ai login window. The session is stored in Windows Credential Manager, never in a file.

## Build
Requires Rust (`winget install Rustlang.Rustup`) and the WebView2 runtime (built into Windows 11).

```bash
cargo test
cargo run -- --once     # sign in if needed, print current usage once
cargo run -- --logout   # forget the saved session
```

## How it works
claude.ai's Settings → Usage page reads `GET /api/organizations/{org}/usage?via=plan`. claude-meter calls the same endpoint with your session cookie. It's an undocumented endpoint, so parsing is tolerant and may need updates if claude.ai changes it.

| Module | Role |
| --- | --- |
| `model.rs` | Pure types and formatting (levels, countdowns, backoff), unit-tested |
| `api.rs` | HTTP client (`ureq`) and tolerant JSON parsing |
| `login.rs` | One-time WebView2 sign-in window that captures the session cookie |
| `secrets.rs` | Windows Credential Manager storage |
| `config.rs` | Settings in `%APPDATA%\claude-meter\config.toml` |

Not affiliated with Anthropic.
