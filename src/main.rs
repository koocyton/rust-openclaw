#[macro_use]
mod log;
mod bot;
mod config;
mod executor;
mod llm_client;

use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string());

    info!(path = %config_path, "加载配置文件");
    let config = config::AppConfig::load(&config_path)?;

    if std::env::args().any(|a| a == "--test-polling") {
        return test_polling(&config.telegram.bot_token).await;
    }

    info!("rust-bot 启动");
    bot::run(config).await
}

async fn test_polling(token: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let base = format!("https://api.telegram.org/bot{}", token);

    println!("=== Telegram Polling 测试 ===");

    let me: serde_json::Value = client
        .get(format!("{}/getMe", base))
        .send().await?.json().await?;
    println!("[getMe] {}", serde_json::to_string_pretty(&me)?);

    let del: serde_json::Value = client
        .get(format!("{}/deleteWebhook?drop_pending_updates=true", base))
        .send().await?.json().await?;
    println!("[deleteWebhook] {}", serde_json::to_string_pretty(&del)?);

    println!("\n现在给 bot 发一条消息，等待 30 秒...\n");

    let updates: serde_json::Value = client
        .get(format!("{}/getUpdates?timeout=30", base))
        .timeout(std::time::Duration::from_secs(35))
        .send().await?.json().await?;
    println!("[getUpdates] {}", serde_json::to_string_pretty(&updates)?);

    Ok(())
}
