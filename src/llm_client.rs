use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tracing::{debug, info};

use crate::config::LlmConfig;

const CLASSIFY_PROMPT: &str = r#"你是一个消息意图分类器。用户通过 Telegram 频道发来消息，你需要判断用户的意图属于以下哪种类型：

1. "question" — 用户在提问、闲聊、咨询，不需要在服务器上执行任何操作
2. "command" — 用户想要在服务器上执行某些操作（如查看文件、检查系统状态、部署、安装软件、截图等）

请返回一个 JSON 对象，格式如下：

如果是问题：
{"type": "question", "content": "直接回答用户问题的完整内容"}

如果是操作命令：
{"type": "command", "commands": [{"command": "shell命令", "description": "说明"}]}

注意：
- 如果用户要求截图或查看屏幕，使用 screencapture 命令（macOS）或 scrot/import 命令（Linux），将图片保存到 /tmp/ 目录
- 只返回 JSON，不要包含其他文字或 markdown 代码块标记
- 对于问题类型，content 字段中直接给出详细有用的回答"#;

const LLM_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum LlmIntent {
    #[serde(rename = "question")]
    Question { content: String },
    #[serde(rename = "command")]
    Command {
        commands: Vec<CommandItem>,
    },
}

#[derive(Debug, Deserialize)]
pub struct CommandItem {
    pub command: String,
    #[serde(default)]
    pub description: String,
}

pub struct LlmClient {
    client: reqwest::Client,
    config: LlmConfig,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(LLM_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client, config }
    }

    pub async fn classify(&self, user_message: &str) -> Result<LlmIntent> {
        let system_prompt = self
            .config
            .system_prompt
            .as_deref()
            .unwrap_or(CLASSIFY_PROMPT);

        tlog!("LLM", ">>> 用户消息: {}", user_message);
        let raw = self.call_api(system_prompt, user_message).await?;
        tlog!("LLM", "<<< 原始响应 ({} 字符): {}", raw.len(), raw);

        let json_text = extract_json_object(&raw);
        tlog!("LLM", "解析 JSON: {}", json_text);

        let intent = serde_json::from_str::<LlmIntent>(&json_text)
            .with_context(|| format!("无法解析 LLM 意图响应: {raw}"))?;

        match &intent {
            LlmIntent::Question { content } => {
                tlog!("LLM", "意图: 问答 → {}", truncate_str(content, 200));
            }
            LlmIntent::Command { commands } => {
                tlog!("LLM", "意图: 命令 → {} 条", commands.len());
                for (i, c) in commands.iter().enumerate() {
                    tlog!("LLM", "  {}. [{}] {}", i + 1, c.description, c.command);
                }
            }
        }

        Ok(intent)
    }

    async fn call_api(&self, system_prompt: &str, user_message: &str) -> Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_message },
            ]
        });

        tlog!("LLM", "模型: {}", self.config.model);
        tlog!("LLM", "URL: {}", url);
        tlog!("LLM", "超时: {}s", LLM_TIMEOUT_SECS);
        tlog!("LLM", "请求体: {}", serde_json::to_string_pretty(&body).unwrap_or_default());
        info!(model = %self.config.model, "调用 LLM");
        debug!(url = %url, body = %body, "LLM 请求");

        let start = Instant::now();
        tlog!("LLM", "发送请求...");

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("LLM API 请求失败（可能超时或网络问题）")?;

        let network_elapsed = start.elapsed();
        let status = resp.status();
        let headers = format!("{:?}", resp.headers());
        tlog!("LLM", "HTTP {} (网络耗时 {:.2}s)", status, network_elapsed.as_secs_f64());
        tlog!("LLM", "响应头: {}", headers);

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            tlog!("LLM", "错误响应体: {}", text);
            anyhow::bail!("LLM API 错误 {status}: {text}");
        }

        tlog!("LLM", "读取响应体...");
        let raw_text = resp.text().await.context("读取 LLM 响应体失败")?;
        let body_elapsed = start.elapsed();
        tlog!("LLM", "响应体大小: {} 字节 (总耗时 {:.2}s)", raw_text.len(), body_elapsed.as_secs_f64());
        tlog!("LLM", "完整响应: {}", truncate_str(&raw_text, 2000));

        let result: Value = serde_json::from_str(&raw_text).context("LLM 响应 JSON 解析失败")?;

        let content = result
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(u) = result.pointer("/usage") {
            tlog!("LLM", "Token 用量: {}", u);
        }
        if let Some(model) = result.pointer("/model").and_then(|v| v.as_str()) {
            tlog!("LLM", "实际模型: {}", model);
        }

        let total = start.elapsed();
        tlog!("LLM", "总耗时: {:.2}s", total.as_secs_f64());

        Ok(content)
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn extract_json_object(text: &str) -> String {
    if let Some(start) = text.find("```") {
        let after_backticks = &text[start + 3..];
        let content_start = after_backticks.find('\n').map(|i| i + 1).unwrap_or(0);
        let content = &after_backticks[content_start..];
        if let Some(end) = content.find("```") {
            return content[..end].trim().to_string();
        }
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return text[start..=end].to_string();
        }
    }
    text.trim().to_string()
}
