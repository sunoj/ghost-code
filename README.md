# ghost-code

Ghostty + [Claude Code](https://docs.anthropic.com/en/docs/claude-code) Telegram bridge. Get notified on your phone when Claude finishes a task or needs input — reply to inject text back into the terminal, or chat with Claude directly via streaming responses.

## Features

- **Stop hook** — sends task completion summaries (silent, with expandable blockquote for long text)
- **Notification hook** — forwards Claude's attention requests
- **PreToolUse hook** — tool approval via inline Allow/Deny buttons (expandable details)
- **StatusLine hook** — real-time status bar showing model, cost, plan usage, context window, and tool stats
- **Streaming chat** — send messages to Claude from Telegram, responses stream live via `sendMessageDraft`
- **Session reply** — reply to any notification (including confirm messages) to inject text into the originating Ghostty tab, with target project confirmation to prevent misrouting
- **Multi-machine support** — all messages, commands, and sessions include hostname (e.g. `project@MacBook-Air`) to distinguish notifications across devices
- **Noise filtering** — suppresses noisy system notifications (quota recovery, session resumed, waiting for input)
- **Sleep prevention** — auto-runs `caffeinate` for 1 hour on every Telegram interaction to keep macOS awake
- **Single instance** — flock-based PID locking ensures only one daemon runs at a time
- **Graceful shutdown** — signal handling (SIGINT/SIGTERM) with PID file cleanup

Single binary, no runtime dependencies.

## Security notice

ghost-code has **high system privileges** by design. Before installing, understand what it can do:

- **Terminal injection** — Telegram messages can be injected as keystrokes into your Ghostty terminal via macOS Accessibility API. Anyone with access to your Telegram bot token can send arbitrary commands to your terminal.
- **macOS Accessibility** — The app requires Accessibility permissions, which grant broad control over UI elements and keyboard input.
- **File system access** — Reads Claude Code session files (`~/.claude/`) including conversation transcripts.
- **Keychain access** — Reads OAuth tokens from macOS Keychain for plan usage display.
- **Process control** — Spawns `claude -p` processes, `caffeinate`, and `osascript` subprocesses.

**Recommendations:**

1. **Keep your bot token secret.** Anyone with the token can send messages to your bot — and those messages can be injected into your terminal. Never share it or commit it to git.
2. **Use `APPROVAL_TOOLS`** to require explicit Telegram approval before Claude executes dangerous tools (e.g., `Bash,Write,Edit`).
3. **One bot per machine.** Do not reuse bot tokens across devices.
4. **Review the source code** before installing. This tool runs with your user permissions and has direct access to your terminal.

## Prerequisites

| Requirement | Version | Check |
|-------------|---------|-------|
| [Rust](https://rustup.rs) | 1.70+ | `cargo --version` |
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code) | any | `claude --version` |
| [Ghostty](https://ghostty.org) | any | only needed for session reply |

## Installation

### Step 1: Create a Telegram bot

1. Open Telegram and search for [@BotFather](https://t.me/BotFather)
2. Send `/newbot` and follow the prompts to pick a name and username
3. BotFather replies with a **bot token** — save it (looks like `123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11`)
4. Open a chat with your new bot and send any message (this initializes the chat)
5. Get your **chat ID** by messaging [@userinfobot](https://t.me/userinfobot) — it replies with your numeric ID

### Step 2: Install

**Recommended** — install from crates.io:

```bash
cargo install ghost-code
```

Then run the setup script to configure Claude Code hooks:

```bash
ghost-code setup
```

**Alternative** — install from source:

```bash
git clone https://github.com/sunoj/ghost-code.git
cd ghost-code
bash install.sh
```

The install script builds the binary, copies it to `~/.claude/hooks/ghost-code`, merges hook entries into `~/.claude/settings.json`, and creates `~/.claude/hooks/ghost-code.env` from the template.

### Step 3: Configure

Edit `~/.claude/hooks/ghost-code.env` with the token and chat ID from Step 1:

```bash
TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11
TELEGRAM_CHAT_ID=987654321
```

Values can optionally be quoted (`"value"` or `'value'`).

### Step 4: Verify

```bash
ghost-code test
```

You should receive two Telegram messages:
- A **silent** stop-hook summary
- A **notification** message

If nothing arrives, see [Troubleshooting](#troubleshooting) below.

## Bot daemon

The bot daemon handles callback queries, session replies, and streaming chat. It is auto-started by hook commands, or you can run it manually:

```bash
~/.claude/hooks/ghost-code bot
```

Send any text message to chat with Claude — responses stream live using `sendMessageDraft`. Reply to any hook notification to inject text into the originating Ghostty terminal tab.

### Bot commands

| Command        | Description                              |
|----------------|------------------------------------------|
| `/help`        | Show help (includes hostname)            |
| `/sessions`    | List active sessions on this machine     |
| `/dir [path]`  | Get/set working directory                |
| `/status`      | Show bot status with hostname and PID    |
| `/stop`        | Stop the bot                             |

### Configuration

All settings can be set in `ghost-code.env` or as environment variables (env vars take precedence):

| Variable            | Default                            | Description                                  |
|---------------------|------------------------------------|----------------------------------------------|
| `TELEGRAM_BOT_TOKEN`| (required)                         | Telegram bot token from BotFather            |
| `TELEGRAM_CHAT_ID`  | (required)                         | Your Telegram chat ID                        |
| `WORKING_DIR`       | `~/.claude/ghost-code/workspace`   | Working directory for `claude -p`            |
| `CLAUDE_TIMEOUT`    | `300`                              | Execution timeout in seconds                 |
| `APPROVAL_TOOLS`    | (empty)                            | Comma-separated tool names requiring approval|
| `APPROVAL_TIMEOUT`  | `120`                              | Seconds to wait for approval response        |
| `STATUSLINE`        | `true`                             | Enable statusline hook (see below)           |
| `DEBUG`             | `false`                            | Log raw hook JSON to debug file              |

### Tool approval

To require Telegram approval before Claude executes certain tools, set `APPROVAL_TOOLS`:

```
APPROVAL_TOOLS=Bash,Write,Edit
```

When Claude tries to use a listed tool, you'll receive a Telegram message with Allow/Deny buttons. The hook blocks until you respond or the timeout expires (defaults to deny).

## Statusline

The statusline hook adds a real-time status bar to Claude Code showing:

```
🤖 Opus 4.6 | 💰 $5 / $463 today | 📊 82% block · 38% weekly | 🧠 25% | 🌐 AIS 80% 51.0K $0.15 | 📡 TG
```

| Component | Source | Description |
|-----------|--------|-------------|
| 🤖 Model | Claude Code session | Current model name |
| 💰 Costs | JSONL scan | Session cost / today's total |
| 📊 Plan | Anthropic API | Block (5h) and weekly usage with reset times |
| 🧠 Context | Claude Code session | Context window usage percentage |
| 🌐 AIS | `ai-summary stats` | Token savings and cost saved from [ai-summary](https://github.com/sunoj/ai-summary) (auto-detected) |
| 📡 TG | Bot daemon | Telegram bot status |

**Auto-detection**: ai-summary stats are shown only if the tool is installed. If the `ai-summary` command is not found, the section is silently omitted.

### Enabling / disabling

The statusline hook is installed by default. To disable it:

1. Set `STATUSLINE=false` in `~/.claude/hooks/ghost-code.env`
2. Re-run `bash install.sh`

This removes the StatusLine hook from `~/.claude/settings.json`. Set back to `true` and re-run to re-enable.

## Multi-device setup

Telegram Bot API only allows **one polling consumer per bot token**. If you run ghost-code on multiple machines with the same token, both will get HTTP 409 errors and neither can receive messages.

**Each device needs its own bot.** Go to [@BotFather](https://t.me/BotFather), create a separate bot for each machine, and configure each `ghost-code.env` with its own token. The `TELEGRAM_CHAT_ID` can be the same across all devices — only the token must differ.

If you see persistent `409 Conflict` errors in the log, it almost always means another instance is polling with the same token (another machine, a stale process, or a deployed service).

## Telegram API features

This project uses modern Telegram Bot API features:

- **`link_preview_options`** — replaces deprecated `disable_web_page_preview`
- **`sendMessageDraft`** — streams partial responses as Claude generates them (Bot API 9.3+)
- **`disable_notification`** — stop hook sends silently (no ring)
- **Expandable blockquotes** (`<blockquote expandable>`) — long summaries and tool details are collapsible

## Hook data format

The binary reads JSON from stdin as provided by Claude Code hooks:

| Hook         | Key field                | Description                          |
|--------------|--------------------------|--------------------------------------|
| Stop         | `last_assistant_message` | Claude's final response text         |
| Notification | `message`, `title`       | What Claude needs from you           |
| PreToolUse   | `tool_name`, `tool_input`| Tool about to be executed            |
| StatusLine   | `model`, `cost`, `context_window` | Session data (piped to stdin) |

## Logging

The bot and hook handlers output structured log lines to stderr with timestamps and contextual tags:

```
12:34:56.789 [bot] started (PID 12345)
12:34:56.790 [bot] chat_id=123456789
12:34:56.790 [bot] working_dir=~/.claude/ghost-code/workspace
12:34:57.001 [poll] received 1 update(s)
12:34:57.002 [msg] from=Peter msg_id=42 len=15: hello claude
12:34:57.003 [claude] starting: dir=~/.claude/ghost-code/workspace timeout=300s prompt=hello claude
12:34:57.050 [claude] spawned pid=67890
12:34:58.100 [claude] streaming... 256chars, 1 drafts sent
12:35:02.100 [claude] done in 5.1s (512 chars, 6 drafts) status=Ok(ExitStatus(0))
[telegram] sent msg_id=Some(43) len=512 mode=Some("HTML")
```

Hook handlers log with `[hook:stop]`, `[hook:notification]`, and `[hook:pre-tool-use]` tags.

### Debug mode

Set `DEBUG=true` in the `.env` file (or as an env var). Raw hook JSON will be logged to `~/.claude/hooks/ghost-code.debug.log`. This is in addition to the standard stderr logging described above.

## Updating

```bash
cargo install ghost-code
```

If installed from source: `cd ghost-code && git pull && bash install.sh`

## Uninstalling

```bash
cargo uninstall ghost-code
rm -f ~/.claude/hooks/ghost-code ~/.claude/hooks/ghost-code.env ~/.claude/hooks/ghost-code.pid
```

Then remove the ghost-code hook entries from `~/.claude/settings.json` (under `hooks.Stop`, `hooks.Notification`, `hooks.PreToolUse`, `hooks.StatusLine`).

## Manual configuration

If you prefer not to use the install script, add this to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/ghost-code stop"
          }
        ]
      }
    ],
    "Notification": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/ghost-code notification"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/ghost-code pre-tool-use"
          }
        ]
      }
    ],
    "StatusLine": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/ghost-code statusline"
          }
        ]
      }
    ]
  }
}
```

The StatusLine hook is optional — omit it if you don't want the status bar.

## License

MIT
