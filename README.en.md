# rust-bot

A long-running Telegram Bot process written in Rust. It listens to Telegram channels, groups, and private chats, uses an LLM (Large Language Model) to infer user intent: answers questions directly, and turns operation instructions into shell commands for execution. It supports returning images.

## Architecture

```
Telegram channel/group messages
       │
       ▼
  ┌──────────┐
  │ rust-bot │  (long-running process, Long Polling)
  └──────────┘
       │
       ▼
  ┌──────────────────┐
  │ LLM API intent   │  (OpenAI-compatible API)
  │ classification   │
  └──────────────────┘
       │
       ├── Question → Answer directly, edit message to return result
       │
       └── Action   → Parse command list
                    │
                    ▼
              ┌──────────┐
              │ Shell    │
              │ execution│
              └──────────┘
                    │
                    ▼
              Send result to Telegram (text + images)
```

## Features

- **Smart intent recognition** — LLM automatically classifies messages as questions or operation instructions
- **Q&A mode** — Question-type messages are answered directly by the LLM
- **Command execution** — Action-type messages are turned into shell commands and executed
- **Image support** — Automatically sends images when command output contains image paths (e.g. screenshots)
- **Message editing** — "Analyzing..." status messages are overwritten by the result, avoiding message spam
- **Concurrent handling** — Multiple messages processed in parallel, no blocking queue
- **Channel support** — Works with private chats, groups, and channels
- **Detailed logging** — Timestamps and duration for each step, easier debugging

## Workflow

1. **Startup** — Load `config.toml`; if `webhook_url` and `webhook_listen` are set, use **Webhook mode**, otherwise clear webhook and start **Long Polling**
2. **Listen** — In Webhook mode Telegram pushes updates to your HTTPS URL; in Polling mode the bot long-polls for messages (both support channels, groups, private)
3. **Analyze** — Send "🔄 Analyzing...", call LLM to classify intent
4. **Handle**
   - Question → LLM answers, edit status message with result
   - Action → Parse command list → Show execution plan → Execute one by one (with optional auto-fix retries on failure) → Send result back
5. **Images/videos** — If command output contains image or video file paths, they are sent to the chat automatically

## Quick Start

### Prerequisites

- Rust 1.70+
- A Telegram Bot Token (from [@BotFather](https://t.me/BotFather))
- An OpenAI-compatible LLM API (e.g. [OpenRouter](https://openrouter.ai))

### Build

```bash
cargo build --release
```

### OpenWrt ARM64 cross-compilation (MTK FILOGIC 820 / MT7981B, etc.)

Target is **aarch64-unknown-linux-musl**, compatible with typical OpenWrt musl environments.

**Option 1: Using [cross](https://github.com/cross-rs/cross) (recommended, no local ARM toolchain needed)**

Install and start Docker first. Then:

```bash
cargo install cross
rustup target add aarch64-unknown-linux-musl
./scripts/build-openwrt.sh
# or directly:
cross build --target aarch64-unknown-linux-musl --release
```

Output is at `target/aarch64-unknown-linux-musl/release/rust-bot`; the script copies it to `./openwrt-out/rust-bot`. If you use `cargo build --target ...` without cross, you need a local aarch64-linux-musl C toolchain (e.g. musl-cross or OpenWrt SDK), or C dependencies like `ring` may fail to build.

**Option 2: Using OpenWrt SDK toolchain**

In `.cargo/config.toml`, set the `linker` for `[target.aarch64-unknown-linux-musl]` to the path of your SDK’s `aarch64-openwrt-linux-musl-gcc`. Then:

```bash
rustup target add aarch64-unknown-linux-musl
cargo build --target aarch64-unknown-linux-musl --release
```

**Upload and run**

Copy `rust-bot` and `config.toml` to the device (e.g. `/opt/rust-bot/`). On OpenWrt, libc is required (musl is statically linked for musl targets, so usually no extra libraries). Run:

```bash
./rust-bot /path/to/config.toml
```

### Configuration

```bash
cp config.example.toml config.toml
# Edit config.toml with your Bot Token and LLM API settings
```

### Run

```bash
# Use default config file config.toml
./target/release/rust-bot

# Specify config path
./target/release/rust-bot /path/to/config.toml

# Enable debug logging
RUST_LOG=debug ./target/release/rust-bot
```

### Telegram Bot setup

1. Create a Bot in [@BotFather](https://t.me/BotFather) and get the Token
2. For **channels**: Add the Bot as a channel administrator
3. For **groups**: Add the Bot to the group
4. Start the program, send a message, and read `chat_id` from the logs
5. Put `chat_id` into `config.toml` under `allowed_chat_ids`

### Webhook mode and exposing from internal network (recommended for channels)

With **Webhook**, Telegram pushes updates to your HTTPS URL, which is often more stable and lower latency than Long Polling, especially for channels and groups.

**Requirements**: `webhook_url` must be **HTTPS** and reachable from the public internet. If the bot runs on an internal network, you need to expose the local port.

**Free and relatively stable ways to expose an internal webhook:**

1. **Cloudflare Tunnel (recommended)**  
   - Free, stable, no public IP or router port forwarding.  
   - You need a domain added to Cloudflare.  

   **Expose port 8443 from scratch with Cloudflare Tunnel (step-by-step):**

   1. **Install cloudflared** (macOS example; see [official install guide](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/installation/) for others):
      ```bash
      brew install cloudflared
      ```
   2. **Log in and create a tunnel**:
      ```bash
      cloudflared tunnel login
      cloudflared tunnel create rust-bot
      ```
      This prints a tunnel ID (e.g. `abcd1234-...`) and creates `~/.cloudflared/<TUNNEL_ID>.json`.
   3. **Route a subdomain to the tunnel** (replace `yourdomain.com` with your domain; `rust-bot` is the subdomain):
      ```bash
      cloudflared tunnel route dns rust-bot rust-bot.yourdomain.com
      ```
   4. **Create a tunnel config file** (e.g. `~/.cloudflared/config.yml`) with the following (**copy-paste ready**; adjust tunnel name and port if needed):
      ```yaml
      tunnel: rust-bot
      credentials-file: /Users/YOUR_USERNAME/.cloudflared/<TUNNEL_ID>.json

      ingress:
        - hostname: rust-bot.yourdomain.com
          service: http://localhost:8443
        - service: http_status:404
      ```
      Replace `<TUNNEL_ID>` with the ID from step 2, and `yourdomain.com` with your domain. On Linux use `/home/yourusername/...` for `credentials-file`.
   5. **Run the tunnel** (ensure rust-bot is already listening on 8443):
      ```bash
      cloudflared tunnel run rust-bot
      ```
      To use a custom config path: `cloudflared tunnel --config /path/to/config.yml run rust-bot`.
   6. **Set Webhook in config.toml**:
      ```toml
      webhook_url = "https://rust-bot.yourdomain.com"
      webhook_listen = "0.0.0.0:8443"
      ```
   7. Start rust-bot; Telegram will push updates to `https://rust-bot.yourdomain.com` → your machine:8443.

   - Docs: [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/)

2. **ngrok**  
   - Free tier gives a quick HTTPS URL, good for local testing.  
   - After install: `ngrok http 8443`, then set `webhook_url` to the generated `https://xxx.ngrok.io`.  
   - Free URLs can change; update config and restart the bot if the URL changes.

**Summary**: Set both `webhook_url` (public HTTPS) and `webhook_listen` (e.g. `0.0.0.0:8443`) in `config.toml` to use Webhook; leave them unset to keep using Long Polling.

## Configuration reference

| Option | Description | Default |
|--------|-------------|---------|
| `telegram.bot_token` | Telegram Bot API Token | Required |
| `telegram.allowed_chat_ids` | Allowed chat ID whitelist; empty array = no restriction | `[]` |
| `telegram.webhook_url` | Webhook mode: public HTTPS URL where Telegram sends updates | None; omit to use Long Polling |
| `telegram.webhook_listen` | Webhook mode: local listen address, e.g. `0.0.0.0:8443` | None |
| `llm.base_url` | OpenAI-compatible API base URL | Required |
| `llm.api_key` | LLM API Key | Required |
| `llm.model` | Model name | Required |
| `llm.max_tokens` | Max generated tokens | `2048` |
| `llm.system_prompt` | Custom system prompt (overrides built-in default) | Built-in |
| `executor.working_dir` | Working directory for command execution | Current dir |
| `executor.timeout_secs` | Per-command timeout (seconds) | `120` |
| `executor.echo_result` | Whether to send execution result to Telegram | `true` |
| `executor.activate_venv` | Python venv path to activate before execution (e.g. `.venv`) | None |
| `executor.max_fix_retries` | Max retries after failure (LLM suggests fix, then auto-retry); 0 = no retry, only show suggestion | `10` |
| `skills_dir` | Path to Skills extension directory; leave empty to use project `skills` | None |

## Skills (extensions)

**Skills** extend what the bot can do without changing the main code. Each skill is a subdirectory with a `skill.toml` that defines name, description, LLM prompt snippets, and install instructions.

- **Enable**: Create a `skills` directory at project root, add skills as in `skills/README.md`; optionally set `skills_dir = "skills"` in `config.toml`.
- **List skills**: Send "what skills are available" (or equivalent) in Telegram.
- **Install help**: Send "how to install screenshot" (or skill name) to see that skill’s install instructions.

See [skills/README.md](skills/README.md) for details.

## Error handling and auto-retry

When a command fails, the program asks the LLM for a fix and can parse suggested commands from the reply to retry automatically:

- **Config** `executor.max_fix_retries`: After a command fails, how many times to ask the LLM for a fix and retry (default **10**). Set to **0** to disable auto-retry and only append "suggested fix" in the report.
- **Flow**: Command fails → Send command, exit code, stderr to LLM (with relevant skill context) → Parse fix command from reply (e.g. code blocks or backtick content) → Execute it; repeat until success or `max_fix_retries`. If still failing after all retries, the report includes the "suggested fix" again at the end.
- This can auto-succeed when the environment or parameters are fixable by the LLM, without the user resending a corrected command.

## LLM interface

The bot talks to the LLM via the OpenAI-compatible Chat Completions API (`/v1/chat/completions`), not MCP.

Supported LLM services:
- [OpenRouter](https://openrouter.ai) — Multiple models
- [OpenAI](https://platform.openai.com)
- Any service with OpenAI-style API (Ollama, vLLM, LocalAI, etc.)

The LLM classifies user messages into:
- **Question** — Return answer text
- **Command** — Return a list of shell commands to run

## Project structure

```
src/
├── main.rs        # Entry, config loading
├── bot.rs         # Telegram Bot message handling, concurrency
├── llm_client.rs  # LLM API calls, intent classification
├── executor.rs    # Shell command execution
├── config.rs      # Config parsing
├── skills.rs      # Skills loading and prompt injection
└── log.rs         # Timestamped logging macros
skills/            # Extension skills (see skills/README.md)
├── README.md      # Skills usage and install
├── screenshot/
│   └── skill.toml
└── screen_record/
    └── skill.toml
```

## Security notes

- **Always set `allowed_chat_ids`** so only authorized chats can trigger command execution
- This program runs arbitrary shell commands on the host; run it in a safe environment
- Prefer running as a restricted user, not root
- `config.toml` contains secrets (Token, API Key); do not commit it to public repos
