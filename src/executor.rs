use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
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

        let run_cmd = if let Some(ref venv) = self.config.activate_venv {
            let activate = if venv.ends_with("activate") || venv.contains("/bin/activate") {
                venv.clone()
            } else {
                let sep = if venv.ends_with('/') { "" } else { "/" };
                format!("{venv}{sep}bin/activate")
            };
            format!("source {} 2>/dev/null && {}", activate, cmd)
        } else {
            cmd.to_string()
        };

        tlog!("CMD", "执行: {}", if run_cmd.len() > 200 { format!("{}...(略)", truncate_str(&run_cmd, 200)) } else { run_cmd.clone() });
        tlog!("CMD", "工作目录: {}", working_dir);
        tlog!("CMD", "超时: {}s", self.config.timeout_secs);
        info!(cmd = %cmd, "执行命令");

        let start = Instant::now();

        let output = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            Command::new("sh")
                .arg("-c")
                .arg(&run_cmd)
                .current_dir(working_dir)
                .stdin(Stdio::null())
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
}

/// 按字节截断到 max，保证在 UTF-8 字符边界处切断，避免 panic。
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...(截断)", &s[..end])
}
