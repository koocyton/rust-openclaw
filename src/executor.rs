use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tracing::{error, info};

use crate::config::ExecutorConfig;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskCommand {
    pub command: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct CommandResult {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub struct Executor {
    config: ExecutorConfig,
}

impl Executor {
    pub fn new(config: ExecutorConfig) -> Self {
        Self { config }
    }

    pub async fn run_command(&self, cmd: &str) -> Result<CommandResult> {
        let working_dir = self
            .config
            .working_dir
            .as_deref()
            .unwrap_or(".");

        tlog!("CMD", "执行: {}", cmd);
        tlog!("CMD", "工作目录: {}", working_dir);
        tlog!("CMD", "超时: {}s", self.config.timeout_secs);
        info!(cmd = %cmd, "执行命令");

        let start = Instant::now();

        let output = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(working_dir)
                .output(),
        )
        .await
        .with_context(|| format!("命令超时 ({} 秒): {cmd}", self.config.timeout_secs))?
        .with_context(|| format!("命令执行失败: {cmd}"))?;

        let elapsed = start.elapsed();

        let result = CommandResult {
            command: cmd.to_string(),
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        };

        if result.success {
            tlog!("CMD", "成功 (exit=0, 耗时 {:.2}s)", elapsed.as_secs_f64());
        } else {
            tlog!("CMD", "失败 (exit={:?}, 耗时 {:.2}s)", result.exit_code, elapsed.as_secs_f64());
            error!(cmd = %cmd, code = ?result.exit_code, stderr = %result.stderr, "命令执行失败");
        }

        if !result.stdout.is_empty() {
            tlog!("CMD", "stdout ({} 字节):\n{}", result.stdout.len(), truncate_str(&result.stdout, 1000));
        }
        if !result.stderr.is_empty() {
            tlog!("CMD", "stderr ({} 字节):\n{}", result.stderr.len(), truncate_str(&result.stderr, 500));
        }

        Ok(result)
    }

    pub async fn run_commands(&self, commands: &[TaskCommand]) -> Vec<CommandResult> {
        let total_start = Instant::now();
        tlog!("CMD", "批量执行 {} 条命令", commands.len());

        let mut results = Vec::new();
        for (i, task) in commands.iter().enumerate() {
            tlog!("CMD", "[{}/{}] {} → {}", i + 1, commands.len(), task.description, task.command);
            match self.run_command(&task.command).await {
                Ok(result) => {
                    let success = result.success;
                    results.push(result);
                    if !success {
                        tlog!("CMD", "命令失败，停止后续执行");
                        break;
                    }
                }
                Err(e) => {
                    tlog!("CMD", "命令异常: {}", e);
                    error!(err = %e, "命令执行异常");
                    results.push(CommandResult {
                        command: task.command.clone(),
                        success: false,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: e.to_string(),
                    });
                    break;
                }
            }
        }

        tlog!("CMD", "批量执行完毕 (总耗时 {:.2}s)", total_start.elapsed().as_secs_f64());
        results
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...(截断)", &s[..max])
    }
}
