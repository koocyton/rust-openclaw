# rust-bot

一个 Rust 编写的 Telegram Bot 常驻进程，监听 Telegram 频道/群组/私聊消息，通过 LLM（大语言模型）智能判断用户意图：对于提问直接回答，对于操作指令自动转化为 shell 命令并执行，支持返回图片。

## 架构

```
Telegram 频道/群组消息
       │
       ▼
  ┌──────────┐
  │ rust-bot │  (常驻进程，Long Polling)
  └──────────┘
       │
       ▼
  ┌──────────────────┐
  │ LLM API 意图分类  │  (OpenAI 兼容接口)
  └──────────────────┘
       │
       ├── 问题 → 直接回答，编辑消息返回
       │
       └── 操作 → 解析命令列表
                    │
                    ▼
              ┌──────────┐
              │ Shell 执行 │
              └──────────┘
                    │
                    ▼
              结果回传 Telegram（文本 + 图片）
```

## 功能特性

- **智能意图识别** — LLM 自动判断消息是提问还是操作指令
- **问答模式** — 提问类消息直接由 LLM 回答
- **命令执行** — 操作类消息自动生成 shell 命令并执行
- **图片支持** — 命令输出中包含图片路径时自动发送图片（如截图）
- **消息编辑** — 「正在分析...」状态消息会被结果直接覆盖，不刷屏
- **并发处理** — 多条消息同时处理，不排队阻塞
- **频道支持** — 同时支持私聊、群组和频道消息
- **详细日志** — 每步操作带时间戳和耗时统计，方便排查问题

## 工作流程

1. **启动** — 读取 `config.toml`；若配置了 `webhook_url` + `webhook_listen` 则使用 **Webhook 模式**，否则清理 Webhook 并启动 **Long Polling**
2. **监听** — Webhook 下由 Telegram 主动推送更新到你的 HTTPS 地址；Polling 下通过长轮询拉取消息（均支持频道/群组/私聊）
3. **分析** — 发送「🔄 正在分析...」，调用 LLM 判断意图
4. **处理**
   - 提问 → LLM 直接回答，编辑覆盖状态消息
   - 操作 → 解析命令列表 → 显示执行计划 → 逐条执行（失败时可自动修正重试）→ 回传结果
5. **图片/视频** — 如果命令输出中包含图片或视频文件路径，自动发送到频道

## 快速开始

### 前置条件

- Rust 1.70+
- 一个 Telegram Bot Token（从 [@BotFather](https://t.me/BotFather) 获取）
- 一个 OpenAI 兼容的 LLM API（如 [OpenRouter](https://openrouter.ai)）

### 构建

```bash
cargo build --release
```

### OpenWrt ARM64 交叉编译（MTK FILOGIC 820 / MT7981B 等）

目标为 **aarch64-unknown-linux-musl**，与 OpenWrt 常见 musl 环境兼容。

**方式一：使用 [cross](https://github.com/cross-rs/cross)（推荐，无需本机装 ARM 工具链）**

需先安装并启动 Docker。然后：

```bash
cargo install cross
rustup target add aarch64-unknown-linux-musl
./scripts/build-openwrt.sh
# 或直接：
cross build --target aarch64-unknown-linux-musl --release
```

产物在 `target/aarch64-unknown-linux-musl/release/rust-bot`，脚本会复制到 `./openwrt-out/rust-bot`。若不用 cross 而直接用 `cargo build --target ...`，需本机具备 aarch64-linux-musl 的 C 工具链（如 musl-cross 或 OpenWrt SDK），否则 `ring` 等 C 依赖会编译失败。

**方式二：使用 OpenWrt SDK 工具链**

在 `.cargo/config.toml` 中为 `[target.aarch64-unknown-linux-musl]` 设置 `linker` 为 SDK 中的 `aarch64-openwrt-linux-musl-gcc` 的路径，然后：

```bash
rustup target add aarch64-unknown-linux-musl
cargo build --target aarch64-unknown-linux-musl --release
```

**上传与运行**

将 `rust-bot` 和 `config.toml` 拷到设备（如 `/opt/rust-bot/`），在 OpenWrt 上需有 libc（musl 已静态链接进 musl 目标的可执行文件，一般无需额外库）。直接运行：

```bash
./rust-bot /path/to/config.toml
```

### 配置

```bash
cp config.example.toml config.toml
# 编辑 config.toml，填入你的 Bot Token 和 LLM API 配置
```

### 运行

```bash
# 使用默认配置文件 config.toml
./target/release/rust-bot

# 指定配置文件路径
./target/release/rust-bot /path/to/config.toml

# 开启 debug 日志
RUST_LOG=debug ./target/release/rust-bot
```

### Telegram Bot 设置

1. 在 [@BotFather](https://t.me/BotFather) 创建 Bot，获取 Token
2. 如果用于**频道**：将 Bot 添加为频道管理员
3. 如果用于**群组**：将 Bot 加入群组
4. 启动程序，发送一条消息，从日志中获取 `chat_id`
5. 将 `chat_id` 填入 `config.toml` 的 `allowed_chat_ids`

### Webhook 模式与内网暴露（推荐频道使用）

使用 **Webhook** 时，Telegram 主动把更新推到你的 HTTPS 地址，通常比 Long Polling 更稳定、延迟更低，适合频道/群组。

**要求**：`webhook_url` 必须是 **HTTPS**、且能从公网访问。若 bot 跑在内网，需要把本机端口暴露到公网。

**免费、相对稳定的内网暴露方式：**

1. **Cloudflare Tunnel（推荐）**  
   - 免费、稳定，无需公网 IP 或路由器端口映射。  
   - 需要有一个域名在 Cloudflare 托管。  

   **从零用 Cloudflare Tunnel 暴露 8443（逐步操作）：**

   1. **安装 cloudflared**（以 macOS 为例，其他见 [官方安装说明](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/installation/)）：
      ```bash
      brew install cloudflared
      ```
   2. **登录并创建隧道**：
      ```bash
      cloudflared tunnel login
      cloudflared tunnel create rust-bot
      ```
      执行后会输出隧道 ID（如 `abcd1234-...`），并生成凭证文件 `~/.cloudflared/<TUNNEL_ID>.json`。
   3. **把子域名解析到该隧道**（把 `yourdomain.com` 换成你的域名，`rust-bot` 即子域名）：
      ```bash
      cloudflared tunnel route dns rust-bot rust-bot.yourdomain.com
      ```
   4. **新建隧道配置文件**（如 `~/.cloudflared/config.yml`），写入以下内容（**可直接复制**，只需把 `rust-bot` 和端口按需修改）：
      ```yaml
      tunnel: rust-bot
      credentials-file: /Users/你的用户名/.cloudflared/<TUNNEL_ID>.json

      ingress:
        - hostname: rust-bot.yourdomain.com
          service: http://localhost:8443
        - service: http_status:404
      ```
      注意：`credentials-file` 里的 `<TUNNEL_ID>` 换成第 2 步输出的 ID；`yourdomain.com` 换成你的域名。
   5. **启动隧道**（先确保本机已启动 rust-bot 并监听 8443）：
      ```bash
      cloudflared tunnel run rust-bot
      ```
      若未用默认路径，可指定配置：`cloudflared tunnel --config /path/to/config.yml run rust-bot`。
   6. **在 config.toml 里配置 Webhook**：
      ```toml
      webhook_url = "https://rust-bot.yourdomain.com"
      webhook_listen = "0.0.0.0:8443"
      ```
   7. 启动 rust-bot，Telegram 的更新会经 `https://rust-bot.yourdomain.com` 推到本机 8443。

   - 文档：[Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/)

2. **ngrok**  
   - 免费版可快速得到一条 HTTPS 域名，适合本地调试。  
   - 安装后：`ngrok http 8443`，把生成的 `https://xxx.ngrok.io` 填到 `webhook_url`。  
   - 免费域名会变，重启或换地址后需更新配置并重启 bot。

**简要步骤**：在 `config.toml` 中同时设置 `webhook_url`（公网 HTTPS）和 `webhook_listen`（如 `0.0.0.0:8443`），启动 bot 即可使用 Webhook；不配则自动退回到 Long Polling。

## 配置说明

| 配置项 | 说明 | 默认值 |
|--------|------|--------|
| `telegram.bot_token` | Telegram Bot API Token | 必填 |
| `telegram.allowed_chat_ids` | 允许的聊天 ID 白名单，空数组表示不限制 | `[]` |
| `telegram.webhook_url` | Webhook 模式：公网 HTTPS 地址（Telegram 推送更新的 URL） | 无，不配则用 Long Polling |
| `telegram.webhook_listen` | Webhook 模式：本机监听地址，如 `0.0.0.0:8443` | 无 |
| `llm.base_url` | OpenAI 兼容 API 的 Base URL | 必填 |
| `llm.api_key` | LLM API Key | 必填 |
| `llm.model` | 模型名称 | 必填 |
| `llm.max_tokens` | 最大生成 Token 数 | `2048` |
| `llm.system_prompt` | 自定义系统提示词（覆盖内置默认值） | 内置 |
| `executor.working_dir` | 命令执行的工作目录 | 当前目录 |
| `executor.timeout_secs` | 单条命令超时时间（秒） | `120` |
| `executor.echo_result` | 是否回传执行结果到 Telegram | `true` |
| `executor.activate_venv` | 执行前激活的 Python venv 路径（如 `.venv`） | 无 |
| `executor.max_fix_retries` | 命令失败时向 LLM 询问修正并自动重试的最大次数，0 表示不重试仅展示建议 | `10` |
| `skills_dir` | Skills 扩展技能目录路径，留空则默认使用项目下的 `skills` 目录 | 无 |

## Skills（扩展技能）

通过 **skills** 可以扩展 bot 能处理的问题类型，而不改主程序代码。每个 skill 是一个子目录，内含 `skill.toml`，定义名称、描述、给 LLM 的提示片段和安装说明。

- **启用**：在项目根下建 `skills` 目录，按 `skills/README.md` 添加 `skill.toml`，可选在 `config.toml` 中设置 `skills_dir = "skills"`。
- **列出技能**：在 Telegram 中发送「有哪些技能」。
- **安装方式**：发送「怎么安装 截图」等，可查看对应技能的安装说明。

详见 [skills/README.md](skills/README.md)。

## 错误修复与自动重试

当某条命令执行失败时，程序会向 LLM 询问修正方式，并可从回复中解析出建议的命令自动重试：

- **配置项** `executor.max_fix_retries`：单条命令失败后，最多请求 LLM 修正并重试的次数，默认 **10**。设为 **0** 时不做自动重试，仅把「解决建议」附在报告里给用户参考。
- **流程**：命令失败 → 将命令、退出码、stderr 发给 LLM（并注入相关 skill 上下文）→ 从回复中解析出修正命令（如代码块或反引号内的内容）→ 执行该命令；若仍失败则重复，直到成功或达到 `max_fix_retries`。全部重试后仍失败时，会在报告末尾再附一次「解决建议」。
- 这样在环境或参数可被 LLM 修正的情况下，有机会自动执行成功，而无需用户手动改命令再发一次。

## LLM 接口

本程序通过 OpenAI 兼容的 Chat Completions API（`/v1/chat/completions`）与 LLM 交互，不使用 MCP 协议。

支持的 LLM 服务：
- [OpenRouter](https://openrouter.ai) — 聚合多家模型
- [OpenAI](https://platform.openai.com)
- 任何兼容 OpenAI API 格式的服务（如 Ollama、vLLM、LocalAI 等）

LLM 负责将用户消息分类为两种意图：
- **问题** — 返回回答内容
- **命令** — 返回要执行的 shell 命令列表

## 项目结构

```
src/
├── main.rs        # 入口，配置加载
├── bot.rs         # Telegram Bot 消息处理、并发调度
├── llm_client.rs  # LLM API 调用、意图分类
├── executor.rs    # Shell 命令执行
├── config.rs      # 配置文件解析
├── skills.rs      # Skills 加载与提示注入
└── log.rs         # 带时间戳的日志宏
skills/            # 扩展技能目录（见 skills/README.md）
├── README.md      # Skills 使用与安装说明
├── screenshot/
│   └── skill.toml
└── screen_record/
    └── skill.toml
```

## 安全注意事项

- **务必配置 `allowed_chat_ids`**，限制只有授权的频道/用户才能触发命令执行
- 该程序会在服务器上执行任意 shell 命令，请确保运行环境安全
- 建议使用受限用户运行，避免使用 root
- `config.toml` 包含敏感信息（Token、API Key），请勿提交到公开仓库
