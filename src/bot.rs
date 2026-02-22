use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId};
use teloxide::update_listeners::webhooks;
use tracing::{error, info, warn};

use crate::config::AppConfig;
use crate::executor::{CommandResult, Executor, TaskCommand};
use crate::llm_client::{LlmClient, LlmIntent};
use crate::skills;

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

/// 按字节截断到 max，保证在 UTF-8 字符边界处切断，避免 panic。
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...(截断)", &s[..end])
}

const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"];
const VIDEO_EXTENSIONS: &[&str] = &[".mp4", ".webm", ".mov", ".mkv", ".avi"];

fn find_file_paths_by_ext(text: &str, exts: &[&str]) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|word| {
            let path = word.trim_matches(|c| c == '"' || c == '\'');
            let lower = path.to_lowercase();
            if exts.iter().any(|ext| lower.ends_with(ext))
                && (path.starts_with('/') || path.starts_with("./"))
            {
                Some(path.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn find_image_paths(text: &str) -> Vec<String> {
    find_file_paths_by_ext(text, IMAGE_EXTENSIONS)
}

fn find_video_paths(text: &str) -> Vec<String> {
    find_file_paths_by_ext(text, VIDEO_EXTENSIONS)
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

fn find_videos_in_results(results: &[CommandResult]) -> Vec<String> {
    let mut videos = Vec::new();
    for r in results {
        videos.extend(find_video_paths(&r.stdout));
        videos.extend(find_video_paths(&r.stderr));
        videos.extend(find_video_paths(&r.command));
    }
    videos.sort();
    videos.dedup();
    videos
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

async fn send_document(bot: &Bot, chat_id: ChatId, path: &str, tid: u64) {
    let file_path = std::path::Path::new(path);
    if !file_path.exists() {
        tlog!(&format!("文档 #{tid}"), "文件不存在: {}", path);
        return;
    }
    tlog!(&format!("文档 #{tid}"), "发送: {}", path);
    match bot.send_document(chat_id, InputFile::file(file_path)).await {
        Ok(_) => tlog!(&format!("文档 #{tid}"), "发送成功: {}", path),
        Err(e) => {
            tlog!(&format!("文档 #{tid}"), "发送失败: {} - {}", path, e);
            error!(err = %e, path = %path, "文档发送失败");
            bot.send_message(chat_id, format!("⚠️ 文档发送失败 {path}: {e}"))
                .await
                .ok();
        }
    }
}

async fn send_videos(bot: &Bot, chat_id: ChatId, paths: &[String], tid: u64) {
    for path in paths {
        let file_path = std::path::Path::new(path);
        if !file_path.exists() {
            tlog!(&format!("视频 #{tid}"), "文件不存在，跳过: {}", path);
            continue;
        }
        tlog!(&format!("视频 #{tid}"), "发送: {}", path);
        match bot
            .send_video(chat_id, InputFile::file(file_path))
            .await
        {
            Ok(_) => tlog!(&format!("视频 #{tid}"), "发送成功: {}", path),
            Err(e) => {
                tlog!(&format!("视频 #{tid}"), "发送失败: {} - {}", path, e);
                error!(err = %e, path = %path, "视频发送失败");
                bot.send_message(chat_id, format!("⚠️ 视频发送失败 {path}: {e}"))
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

fn is_asking_skills_list(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t.contains("有哪些技能") || t.contains("列出技能") || t.contains("有什么技能")
        || t.contains("list skill") || t.contains("已安装的 skill")
}

/// 是否为「列出 avfoundation 设备」命令
fn is_list_avfoundation_devices(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    c.contains("avfoundation") && c.contains("list_devices") && c.contains("-i")
}

/// 是否为 avfoundation 录屏命令（macOS）
fn is_avfoundation_record(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    c.contains("avfoundation") && c.contains("-i") && (c.contains("-t") || c.contains(".mp4") || c.contains("screen_record"))
}

/// 从 ffmpeg -list_devices 的 stdout 中解析第一个「Capture screen」对应的设备索引。
/// 格式示例: [AVFoundation indev @ 0x...] [1] Capture screen 0
fn parse_avfoundation_screen_index(stdout: &str) -> Option<u32> {
    for line in stdout.lines() {
        if !line.contains("Capture screen") {
            continue;
        }
        let before_cap = match line.find("Capture screen") {
            Some(p) => &line[..p],
            None => continue,
        };
        let mut idx = before_cap.len();
        while idx > 0 {
            let Some(close) = before_cap[..idx].rfind(']') else { break };
            let Some(open) = before_cap[..close].rfind('[') else { break };
            let between = before_cap[open + 1..close].trim();
            if !between.is_empty()
                && between.chars().all(|c| c.is_ascii_digit())
                && between.parse::<u32>().is_ok()
            {
                return between.parse().ok();
            }
            idx = close;
        }
    }
    None
}

/// 将 avfoundation 录屏命令中的 -i "X:0" 设备号替换为指定索引
fn replace_avfoundation_device_index(cmd: &str, index: u32) -> String {
    let mut result = cmd.to_string();
    let new_index_str = index.to_string();
    let Some(pos) = result.find("-i ") else { return result };
    let mut i = pos + 3;
    let bytes = result.as_bytes();
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
        i += 1;
    }
    let num_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let num_end = i;
    if num_end > num_start && num_end < bytes.len() && bytes[num_end] == b':' && bytes.get(num_end + 1) == Some(&b'0') {
        result.replace_range(num_start..num_end, &new_index_str);
    }
    result
}

/// 从 LLM 的「解决建议」文本中提取一条可执行的 shell 命令（优先代码块或反引号内的内容）。
fn extract_command_from_suggestion(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(a) = s.find("```") {
        let b = s[a + 3..].find("```");
        let block = if let Some(b) = b {
            s[a + 3..a + 3 + b].trim()
        } else {
            s[a + 3..].trim()
        };
        let first_line = block.lines().next().unwrap_or("").trim();
        if !first_line.is_empty() && !first_line.starts_with('#') {
            return Some(first_line.to_string());
        }
        if block.lines().count() <= 2 {
            let one = block.lines().filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#')).next();
            if let Some(line) = one {
                return Some(line.trim().to_string());
            }
        }
    }
    if let Some(start) = s.find('`') {
        let after = &s[start + 1..];
        if let Some(end) = after.find('`') {
            let inner = after[..end].trim();
            if !inner.is_empty() && (inner.contains(' ') || inner.starts_with('/') || inner.starts_with("echo")) {
                return Some(inner.to_string());
            }
        }
    }
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("ffmpeg ")
            || line.starts_with("python ")
            || line.starts_with("python3 ")
            || line.starts_with("/usr/bin/python")
            || line.starts_with("pip ")
            || line.starts_with("source ")
            || (line.starts_with('/') && line.contains("python"))
        {
            return Some(line.to_string());
        }
    }
    None
}

/// 解析 ppt-generator "标题" "内容" 形式的命令，返回 (标题, 讲稿内容)。
fn parse_ppt_generator_args(cmd: &str) -> Option<(String, String)> {
    let cmd = cmd.trim();
    if !cmd.starts_with("ppt-generator ") {
        return None;
    }
    let rest = cmd["ppt-generator ".len()..].trim_start();
    let mut in_quote = false;
    let mut escape = false;
    let mut segments: Vec<(usize, usize)> = vec![];
    let mut segment_start = 0usize;
    for (i, c) in rest.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && in_quote {
            escape = true;
            continue;
        }
        if c == '"' {
            if !in_quote {
                in_quote = true;
                segment_start = i + 1;
            } else {
                in_quote = false;
                segments.push((segment_start, i));
            }
        }
    }
    if segments.len() < 2 {
        return None;
    }
    let title = rest[segments[0].0..segments[0].1].to_string();
    let content = rest[segments[1].0..segments[1].1].to_string();
    Some((title, content))
}

fn extract_install_query(text: &str) -> Option<String> {
    let t = text.trim();
    let lower = t.to_lowercase();
    for prefix in ["怎么安装", "如何安装", "安装 ", "怎么用 "] {
        if lower.contains(prefix) {
            let start = lower.find(prefix).unwrap() + prefix.len();
            let rest = t[start..].trim();
            let end = rest.find(|c: char| c == '？' || c == '?' || c == '。').unwrap_or(rest.len());
            let query = rest[..end].trim();
            if !query.is_empty() {
                return Some(query.to_string());
            }
        }
    }
    None
}

/// 逐条执行命令；某条失败时若 max_fix_retries > 0 则向 LLM 询问修正并重试，直到成功或达到上限。
async fn run_commands_with_fix_retry(
    executor: &Executor,
    llm: &LlmClient,
    skills: &[skills::Skill],
    commands: &[TaskCommand],
    max_fix_retries: u32,
    tag: &str,
) -> Vec<CommandResult> {

    let mut results = Vec::new();
    for (i, task) in commands.iter().enumerate() {
        tlog!(tag, "[{}/{}] {} → {}", i + 1, commands.len(), task.description, truncate(&task.command, 80));
        let mut result = match executor.run_command(&task.command).await {
            Ok(r) => r,
            Err(e) => {
                tlog!(tag, "命令异常: {}", e);
                results.push(CommandResult {
                    command: task.command.clone(),
                    success: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: e.to_string(),
                });
                break;
            }
        };
        let mut retry_count = 0u32;
        while !result.success && retry_count < max_fix_retries {
            let fix_context = skills::build_relevant_context_for_fix(skills, &result.command);
            tlog!(tag, "命令失败，第 {} 次请求 LLM 修正 (最多 {})", retry_count + 1, max_fix_retries);
            let suggestion = match llm
                .ask_fix_for_failure(&result.command, result.exit_code, &result.stderr, Some(&fix_context))
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    tlog!(tag, "获取修正建议失败: {}", e);
                    break;
                }
            };
            let fix_cmd = match extract_command_from_suggestion(suggestion.trim()) {
                Some(c) => c,
                None => {
                    tlog!(tag, "未能从建议中解析出命令，停止重试");
                    break;
                }
            };
            tlog!(tag, "执行修正命令: {}", truncate(&fix_cmd, 120));
            match executor.run_command(&fix_cmd).await {
                Ok(r) => result = r,
                Err(e) => {
                    result = CommandResult {
                        command: fix_cmd,
                        success: false,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: e.to_string(),
                    };
                }
            }
            retry_count += 1;
        }
        let success = result.success;
        results.push(result);
        if !success {
            tlog!(tag, "命令失败，停止后续执行");
            break;
        }
    }
    results
}

async fn process_message(
    bot: Bot,
    chat_id: ChatId,
    text: String,
    llm: Arc<LlmClient>,
    executor: Arc<Executor>,
    skills: Arc<Vec<skills::Skill>>,
    max_fix_retries: u32,
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

    let prompt_suffix = skills::build_prompt_section(skills.as_slice());
    let prompt_suffix_opt = if prompt_suffix.is_empty() {
        tlog!(&tag, "未使用 skills（无技能或未加载）");
        None
    } else {
        tlog!(&tag, "使用 {} 个 skills 注入提示 ({} 字符)", skills.len(), prompt_suffix.len());
        Some(prompt_suffix.as_str())
    };

    tlog!(&tag, "调用 LLM...");
    let llm_start = Instant::now();
    let intent = match llm.classify(&text, prompt_suffix_opt).await {
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
            let reply = if is_asking_skills_list(&text) {
                skills::list_skills_summary(skills.as_slice())
            } else if let Some(query) = extract_install_query(&text) {
                skills::get_install_instructions(skills.as_slice(), &query)
                    .unwrap_or_else(|| content.clone())
            } else {
                content
            };
            tlog!(&tag, "问答回复: {}", truncate(&reply, 200));
            edit_or_send(&bot, chat_id, status_msg_id, &reply).await;
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
                .map(|(i, c)| format!("{}. {} → `{}`", i + 1, c.description, truncate(&c.command, 100)))
                .collect::<Vec<_>>()
                .join("\n");
            tlog!(&tag, "执行计划:\n{}", plan);
            let plan_text = format!("📝 执行计划:\n{plan}\n\n⏳ 执行中...");
            edit_or_send(&bot, chat_id, status_msg_id, &plan_text).await;

            let exec_start = Instant::now();
            let (results, extra_doc_paths) = if !commands.is_empty()
                && commands[0].command.trim_start().starts_with("ppt-generator ")
                && parse_ppt_generator_args(&commands[0].command).is_some()
            {
                let (title, content) = parse_ppt_generator_args(&commands[0].command).unwrap();
                tlog!(&tag, "使用 LLM 直接生成 PPT HTML（不依赖 Python 模块）");
                match llm.generate_ppt_html(&content).await {
                    Ok(html) => {
                        let path = "/tmp/slides.html";
                        if let Err(e) = std::fs::write(path, &html) {
                            tlog!(&tag, "写入 HTML 失败: {}", e);
                            (
                                vec![CommandResult {
                                    command: commands[0].command.clone(),
                                    success: false,
                                    exit_code: None,
                                    stdout: String::new(),
                                    stderr: format!("写入文件失败: {e}"),
                                }],
                                vec![],
                            )
                        } else {
                            tlog!(&tag, "已保存到 {}", path);
                            (
                                vec![CommandResult {
                                    command: format!("LLM 生成乔布斯风 HTML 演示稿（{}）", title),
                                    success: true,
                                    exit_code: Some(0),
                                    stdout: format!("已生成并保存到 {path}"),
                                    stderr: String::new(),
                                }],
                                vec![path.to_string()],
                            )
                        }
                    }
                    Err(e) => {
                        tlog!(&tag, "LLM 生成 PPT 失败: {}", e);
                        (
                            vec![CommandResult {
                                command: commands[0].command.clone(),
                                success: false,
                                exit_code: None,
                                stdout: String::new(),
                                stderr: e.to_string(),
                            }],
                            vec![],
                        )
                    }
                }
            } else if commands.len() >= 2
                && is_list_avfoundation_devices(&commands[0].command)
                && is_avfoundation_record(&commands[1].command)
            {
                tlog!(&tag, "录屏前先列出 avfoundation 设备...");
                match executor.run_command(&commands[0].command).await {
                    Ok(r0) => {
                        let screen_index = parse_avfoundation_screen_index(&r0.stdout);
                        let mut rest = commands[1..].to_vec();
                        if let Some(idx) = screen_index {
                            tlog!(&tag, "解析到屏幕设备索引: {}", idx);
                            rest[0].command = replace_avfoundation_device_index(&rest[0].command, idx);
                            tlog!(&tag, "已替换录屏命令设备号: {}", rest[0].command);
                        } else {
                            tlog!(&tag, "未解析到 Capture screen 索引，使用原录屏命令");
                        }
                        let rest_results =
                            run_commands_with_fix_retry(&executor, &llm, skills.as_slice(), &rest, max_fix_retries, &tag).await;
                        let mut all = vec![r0];
                        all.extend(rest_results);
                        (all, vec![])
                    }
                    Err(e) => {
                        tlog!(&tag, "列出设备失败，按原计划执行: {}", e);
                        (
                            run_commands_with_fix_retry(&executor, &llm, skills.as_slice(), &commands, max_fix_retries, &tag).await,
                            vec![],
                        )
                    }
                }
            } else {
                tlog!(&tag, "开始执行命令... (失败时最多修正重试 {} 次)", max_fix_retries);
                (
                    run_commands_with_fix_retry(&executor, &llm, skills.as_slice(), &commands, max_fix_retries, &tag).await,
                    vec![],
                )
            };
            tlog!(&tag, "命令执行完毕 ({} 条, 耗时 {:.2}s)", results.len(), exec_start.elapsed().as_secs_f64());

            let mut report = format_results(&commands, &results);
            if let Some(failed) = results.last().filter(|r| !r.success) {
                tlog!(&tag, "最终仍失败，附加一次解决建议到报告");
                let fix_context = skills::build_relevant_context_for_fix(skills.as_slice(), &failed.command);
                match llm.ask_fix_for_failure(&failed.command, failed.exit_code, &failed.stderr, Some(&fix_context)).await {
                    Ok(suggestion) => {
                        let suggestion_trim = truncate(suggestion.trim(), 1500);
                        report.push_str(&format!("\n💡 解决建议：\n{suggestion_trim}"));
                    }
                    Err(e) => {
                        report.push_str(&format!("\n⚠️ 获取解决建议失败: {e}"));
                    }
                }
            }

            if echo_result {
                edit_or_send(&bot, chat_id, status_msg_id, &report).await;
                tlog!(&tag, "报告已发送（覆盖状态消息）");
            }

            let images = find_images_in_results(&results);
            if !images.is_empty() {
                tlog!(&tag, "发现 {} 个图片", images.len());
                send_images(&bot, chat_id, &images, tid).await;
            }
            let videos = find_videos_in_results(&results);
            if !videos.is_empty() {
                tlog!(&tag, "发现 {} 个视频", videos.len());
                send_videos(&bot, chat_id, &videos, tid).await;
            }
            for path in &extra_doc_paths {
                send_document(&bot, chat_id, path, tid).await;
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
    skills: Arc<Vec<skills::Skill>>,
    max_fix_retries: u32,
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
        process_message(bot, chat_id, text, llm, executor, skills, max_fix_retries, echo_result, tid).await;
    });

    tlog!(&format!("调度 #{tid}"), "已提交后台处理，立即返回接收下一条消息");
    Ok(())
}

pub async fn run(config: AppConfig) -> Result<()> {
    let bot = Bot::new(&config.telegram.bot_token);
    let allowed_chats = config.telegram.allowed_chat_ids.clone();
    let echo_result = config.executor.echo_result;
    let max_fix_retries = config.executor.max_fix_retries;

    let llm = Arc::new(LlmClient::new(config.llm.clone()));
    let executor = Arc::new(Executor::new(config.executor.clone()));
    let skills = Arc::new(skills::load_skills(config.skills_dir.as_deref()));

    tlog!("启动", "开始监听 Telegram 消息...");
    tlog!("启动", "Bot Token: {}...", truncate(&config.telegram.bot_token, 10));
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
                 skills: Arc<Vec<skills::Skill>>,
                 max_fix_retries: u32,
                 allowed_chats: Vec<i64>,
                 echo_result: bool| {
                    handle_message(bot, msg, me, llm, executor, skills, max_fix_retries, allowed_chats, echo_result)
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
                 skills: Arc<Vec<skills::Skill>>,
                 max_fix_retries: u32,
                 allowed_chats: Vec<i64>,
                 echo_result: bool| {
                    handle_message(bot, msg, me, llm, executor, skills, max_fix_retries, allowed_chats, echo_result)
                },
            ),
        );

    let llm_clone = llm.clone();
    let executor_clone = executor.clone();

    let mut dp = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![
            llm_clone,
            executor_clone,
            skills,
            max_fix_retries,
            allowed_chats,
            echo_result
        ])
        .default_handler(|upd| async move {
            tlog!("默认", "未匹配的更新: {:?}", upd.kind);
            warn!("未处理的更新: {:?}", upd.kind);
        })
        .error_handler(LoggingErrorHandler::with_custom_text("消息处理出错"))
        .enable_ctrlc_handler()
        .build();

    match (&config.telegram.webhook_url, &config.telegram.webhook_listen) {
        (Some(url_str), Some(listen_str)) => {
            let webhook_url = url_str
                .parse::<url::Url>()
                .map_err(|e| anyhow!("webhook_url 解析失败: {}", e))?;
            if webhook_url.scheme() != "https" {
                return Err(anyhow!("Telegram Webhook 要求 HTTPS，当前: {}", webhook_url.scheme()));
            }
            let addr: SocketAddr = listen_str
                .parse()
                .map_err(|e| anyhow!("webhook_listen 解析失败 (例: 0.0.0.0:8443): {}", e))?;
            tlog!("启动", "使用 Webhook 模式: {} <- {}", webhook_url, addr);
            let options = webhooks::Options::new(addr, webhook_url);
            let listener = webhooks::axum(bot, options)
                .await
                .map_err(|e| anyhow!("Webhook 设置失败: {:?}", e))?;
            let err_handler = Arc::new(teloxide::error_handlers::IgnoringErrorHandlerSafe);
            dp.dispatch_with_listener(listener, err_handler).await;
        }
        _ => {
            tlog!("启动", "清理 webhook...");
            let delete_url = format!(
                "https://api.telegram.org/bot{}/deleteWebhook?drop_pending_updates=true",
                &config.telegram.bot_token
            );
            match reqwest::get(&delete_url).await {
                Ok(resp) => tlog!("启动", "deleteWebhook: {}", resp.status()),
                Err(e) => tlog!("启动", "deleteWebhook 失败: {}", e),
            }
            tlog!("启动", "开始 Long Polling...");
            dp.dispatch().await;
        }
    }

    Ok(())
}
