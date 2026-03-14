# ghost-code

Ghostty + Claude Code Telegram integration — notifications, streaming chat, and terminal injection.

## Architecture

```
src/
├── main.rs          # Entry point: dispatches to hook handlers or bot daemon
├── config.rs        # Config loading from .env
├── telegram.rs      # Telegram Bot API client
├── usage.rs         # Cost tracking
├── plan_usage.rs    # Plan usage limit parsing
├── hook/            # Hook handlers (called by Claude Code)
│   ├── mod.rs       # Spool I/O, fast handlers, utilities
│   ├── format.rs    # Text extraction, formatting, hostname
│   ├── process.rs   # Stop/notification/pre-tool-use processors
│   ├── session.rs   # Session mapping, pending replies, consolidation
│   └── terminal.rs  # Ghostty tab detection and title management
└── bot/             # Telegram bot daemon
    ├── mod.rs       # Daemon lifecycle, signal handling, spool processing
    ├── callbacks.rs # Callback query handler (approvals)
    ├── commands.rs  # Slash commands (/help, /sessions, /dir, etc.)
    ├── messages.rs  # Message routing, dedup, Ghostty injection
    ├── polling.rs   # JSONL response polling, plan notifications
    ├── status.rs    # Statusline data (costs, plan limits, tool savings)
    └── streaming.rs # Claude CLI streaming output
```

Hook handlers use a fast spool-to-disk path (no config load, no network) so Claude Code is never blocked. The bot daemon processes spool files asynchronously. Only sessions running inside Ghostty (`TERM_PROGRAM=ghostty`) are processed — non-Ghostty sessions (e.g. `aid` agents, other terminals) are silently skipped at the hook entry point.

## Screen Lock Handling

Ghostty injection requires macOS Accessibility API, which is unavailable when the screen is locked. The bot detects this via `ioreg` (`CGSSessionScreenIsLocked`) and degrades gracefully:

- **Message injection**: Shows a locked icon with "Screen locked — unlock and resend" instead of a raw AppleScript error.
- **Plan approval**: Keeps inline keyboard buttons so the user can tap again after unlocking. Sends a notification that the screen is locked.

Note: `caffeinate` (spawned on each interaction) prevents system sleep but cannot prevent screen lock.

## Release Process

After `cargo build --release`, **always** copy and re-sign the binary:

```bash
cargo build --release && cp ./target/release/ghost-code ~/.claude/hooks/ghost-code && codesign --force --sign - ~/.claude/hooks/ghost-code
```

The binary at `~/.claude/hooks/ghost-code` is the one Claude Code actually invokes for hooks. Building alone does NOT update the running version. The `codesign` step is mandatory — macOS kills unsigned/invalidated binaries with SIGKILL (exit 137).
