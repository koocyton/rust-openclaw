use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId};
use tracing::{error, info, warn};

use crate::config::AppConfig;
use crate::executor::{CommandResult, Executor, TaskCommand};
use crate::llm_client::{LlmClient, LlmIntent};

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);

fn format_results(commands: &[TaskCommand], results: &[CommandResult]) -> String {
    let mut msg = String::from("📋 任务执行报告\n\n");
    for (i, result) in results.iter().enumerate() {
        let desc = commands
            .get(i)
            .map(|c| c.description.as_str())
            .unwrap_or("未知");
        let status = if result.success { "✅" } else { "❌" };
        msg.push_str(&format!("{status} {desc}\n"));
        msg.push_str(&format!("  命令: {}\n", result.command));
        if !result.stdout.is_empty() {
            let stdout = truncate(&result.stdout, 500);
            msg.push_str(&format!("  输出:\n{stdout}\n"));
        }
        if !result.stderr.is_empty() {
            let stderr = truncate(&result.stderr, 300);
            msg.push_str(&format!("  错误:\n{stderr}\n"));
        }
        msg.push('\n');
    }
    msg
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...(截断)", &s[..max])
    }
}

const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"];

fn find_image_paths(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|word| {
            let lower = word.to_lowercase();
            IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
                && (word.starts_with('/') || word.starts_with("./"))
        })
        .map(|s| s.to_string())
        .collect()
}

fn find_images_in_results(results: &[CommandResult]) -> Vec<String> {
    let mut images = Vec::new();
    for r in results {
        images.extend(find_image_paths(&r.stdout));
        images.extend(find_image_paths(&r.stderr));
        images.extend(find_image_paths(&r.command));
    }
    images.sort();
    images.dedup();
    images
}

async fn send_images(bot: &Bot, chat_id: ChatId, paths: &[String], tid: u64) {
    for path in paths {
        let file_path = std::path::Path::new(path);
        if !file_path.exists() {
            tlog!(&format!("图片 #{tid}"), "文件不存在，跳过: {}", path);
            continue;
        }
        tlog!(&format!("图片 #{tid}"), "发送: {}", path);
        match bot
            .send_photo(chat_id, InputFile::file(file_path))
            .await
        {
            Ok(_) => tlog!(&format!("图片 #{tid}"), "发送成功: {}", path),
            Err(e) => {
                tlog!(&format!("图片 #{tid}"), "发送失败: {} - {}", path, e);
                error!(err = %e, path = %path, "图片发送失败");
                bot.send_message(chat_id, format!("⚠️ 图片发送失败 {path}: {e}"))
                    .await
                    .ok();
            }
        }
    }
}

async fn edit_or_send(bot: &Bot, chat_id: ChatId, status_msg_id: Option<MessageId>, text: &str) -> Option<MessageId> {
    if let Some(msg_id) = status_msg_id {
        match bot.edit_message_text(chat_id, msg_id, text).await {
            Ok(_) => return Some(msg_id),
            Err(e) => {
                tlog!("TG", "编辑消息失败，改为发送新消息: {}", e);
            }
        }
    }
    match bot.send_message(chat_id, text).await {
        Ok(msg) => Some(msg.id),
        Err(_) => None,
    }
}

async fn process_message(
    bot: Bot,
    chat_id: ChatId,
    text: String,
    llm: Arc<LlmClient>,
    executor: Arc<Executor>,
    echo_result: bool,
    tid: u64,
) {
    let tag = format!("#{tid}");
    let total_start = Instant::now();
    tlog!(&tag, "开始处理: {}", text);

    tlog!(&tag, "发送「正在分析」提示...");
    let status_msg_id = bot.send_message(chat_id, "🔄 正在分析...")
        .await
        .ok()
        .map(|m| m.id);
    tlog!(&tag, "状态消息 ID: {:?}", status_msg_id);

    tlog!(&tag, "调用 LLM...");
    let llm_start = Instant::now();
    let intent = match llm.classify(&text).await {
        Ok(intent) => intent,
        Err(e) => {
            tlog!(&tag, "LLM 失败 (耗时 {:.2}s): {}", llm_start.elapsed().as_secs_f64(), e);
            error!(err = %e, "LLM 调用失败");
            edit_or_send(&bot, chat_id, status_msg_id, &format!("❌ LLM 调用失败: {e}")).await;
            return;
        }
    };
    tlog!(&tag, "LLM 完成 (耗时 {:.2}s)", llm_start.elapsed().as_secs_f64());

    match intent {
        LlmIntent::Question { content } => {
            tlog!(&tag, "问答回复: {}", truncate(&content, 200));
            edit_or_send(&bot, chat_id, status_msg_id, &content).await;
            tlog!(&tag, "回答已发送（覆盖状态消息）");
        }
        LlmIntent::Command { commands } => {
            let commands: Vec<TaskCommand> = commands
                .into_iter()
                .map(|c| TaskCommand {
                    command: c.command,
                    description: c.description,
                })
                .collect();

            if commands.is_empty() {
                tlog!(&tag, "无需执行命令");
                edit_or_send(&bot, chat_id, status_msg_id, "ℹ️ 该消息不需要执行任何命令").await;
                return;
            }

            let plan: String = commands
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{}. {} → `{}`", i + 1, c.description, c.command))
                .collect::<Vec<_>>()
                .join("\n");
            tlog!(&tag, "执行计划:\n{}", plan);
            let plan_text = format!("📝 执行计划:\n{plan}\n\n⏳ 执行中...");
            edit_or_send(&bot, chat_id, status_msg_id, &plan_text).await;

            tlog!(&tag, "开始执行命令...");
            let exec_start = Instant::now();
            let results = executor.run_commands(&commands).await;
            tlog!(&tag, "命令执行完毕 ({} 条, 耗时 {:.2}s)", results.len(), exec_start.elapsed().as_secs_f64());

            if echo_result {
                let report = format_results(&commands, &results);
                edit_or_send(&bot, chat_id, status_msg_id, &report).await;
                tlog!(&tag, "报告已发送（覆盖状态消息）");
            }

            let images = find_images_in_results(&results);
            if !images.is_empty() {
                tlog!(&tag, "发现 {} 个图片", images.len());
                send_images(&bot, chat_id, &images, tid).await;
            }
        }
    }

    tlog!(&tag, "处理完毕 (总耗时 {:.2}s)", total_start.elapsed().as_secs_f64());
}

async fn handle_message(
    bot: Bot,
    msg: Message,
    me: teloxide::types::Me,
    llm: Arc<LlmClient>,
    executor: Arc<Executor>,
    allowed_chats: Vec<i64>,
    echo_result: bool,
) -> ResponseResult<()> {
    if let Some(from_user) = &msg.from {
        if from_user.id == me.id {
            return Ok(());
        }
    }
    if msg.via_bot.as_ref().map(|b| b.id) == Some(me.id) {
        return Ok(());
    }
    if msg.author_signature().is_some() && msg.from.is_none() {
        // skip bot's own channel posts (no `from` field, has author_signature)
    }

    let chat_id = msg.chat.id;
    let from = msg
        .from
        .as_ref()
        .map(|u| u.first_name.clone())
        .unwrap_or_else(|| {
            msg.author_signature()
                .unwrap_or("unknown")
                .to_string()
        });
    let tid = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tag = format!("收到 #{tid}");

    tlog!(&tag, "========================================");
    tlog!(&tag, "chat_id: {}, 发送者: {}", chat_id.0, from);
    tlog!(&tag, "内容: {:?}", msg.text().unwrap_or("<非文本消息>"));
    tlog!(&tag, "========================================");

    if !allowed_chats.is_empty() && !allowed_chats.contains(&chat_id.0) {
        tlog!(&format!("权限 #{tid}"), "chat_id {} 不在允许列表中，已忽略", chat_id.0);
        return Ok(());
    }

    let text = match msg.text() {
        Some(t) => t.to_string(),
        None => return Ok(()),
    };

    info!(chat_id = chat_id.0, text = %text, tid = tid, "收到消息");

    tokio::spawn(async move {
        process_message(bot, chat_id, text, llm, executor, echo_result, tid).await;
    });

    tlog!(&format!("调度 #{tid}"), "已提交后台处理，立即返回接收下一条消息");
    Ok(())
}

pub async fn run(config: AppConfig) -> Result<()> {
    let bot = Bot::new(&config.telegram.bot_token);
    let allowed_chats = config.telegram.allowed_chat_ids.clone();
    let echo_result = config.executor.echo_result;

    let llm = Arc::new(LlmClient::new(config.llm.clone()));
    let executor = Arc::new(Executor::new(config.executor.clone()));

    tlog!("启动", "开始监听 Telegram 消息...");
    tlog!("启动", "Bot Token: {}...", &config.telegram.bot_token[..config.telegram.bot_token.len().min(10)]);
    tlog!("启动", "允许的聊天 ID: {:?}", &config.telegram.allowed_chat_ids);
    tlog!("启动", "模型: {}", &config.llm.model);

    let handler = dptree::entry()
        .branch(
            Update::filter_message().endpoint(
                |bot: Bot,
                 msg: Message,
                 me: teloxide::types::Me,
                 llm: Arc<LlmClient>,
                 executor: Arc<Executor>,
                 allowed_chats: Vec<i64>,
                 echo_result: bool| {
                    handle_message(bot, msg, me, llm, executor, allowed_chats, echo_result)
                },
            ),
        )
        .branch(
            Update::filter_channel_post().endpoint(
                |bot: Bot,
                 msg: Message,
                 me: teloxide::types::Me,
                 llm: Arc<LlmClient>,
                 executor: Arc<Executor>,
                 allowed_chats: Vec<i64>,
                 echo_result: bool| {
                    handle_message(bot, msg, me, llm, executor, allowed_chats, echo_result)
                },
            ),
        );

    tlog!("启动", "清理 webhook...");
    let delete_url = format!(
        "https://api.telegram.org/bot{}/deleteWebhook?drop_pending_updates=true",
        &config.telegram.bot_token
    );
    match reqwest::get(&delete_url).await {
        Ok(resp) => tlog!("启动", "deleteWebhook: {}", resp.status()),
        Err(e) => tlog!("启动", "deleteWebhook 失败: {}", e),
    }

    tlog!("启动", "开始 polling 循环...");

    let llm_clone = llm.clone();
    let executor_clone = executor.clone();

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![
            llm_clone,
            executor_clone,
            allowed_chats,
            echo_result
        ])
        .default_handler(|upd| async move {
            tlog!("默认", "未匹配的更新: {:?}", upd.kind);
            warn!("未处理的更新: {:?}", upd.kind);
        })
        .error_handler(LoggingErrorHandler::with_custom_text("消息处理出错"))
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
