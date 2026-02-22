//! Skills 模块：从 skills 目录加载扩展能力，供 LLM 在分类时参考。
//!
//! 每个 skill 是一个子目录，支持两种清单格式：
//! - `skill.toml`：TOML 格式，含 id / name / description / prompt_hint / install
//! - `SKILL.md`：Markdown + YAML frontmatter（--- 内 name、description 等），无 prompt_hint 时用 description

use serde::Deserialize;
use std::path::Path;
use tracing::{debug, info, warn};

const DEFAULT_SKILLS_DIR: &str = "skills";
const SKILL_MANIFEST: &str = "skill.toml";
const SKILL_MD: &str = "SKILL.md";

#[derive(Debug, Deserialize, Clone)]
pub struct SkillManifest {
    /// 唯一标识，建议小写+下划线
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 简短描述
    #[serde(default)]
    pub description: String,
    /// 注入到 LLM 系统提示的说明：何时使用、推荐命令或示例等
    #[serde(default)]
    pub prompt_hint: String,
    /// 安装方式说明（依赖、命令、权限等），用于回复「怎么安装 xx」
    #[serde(default)]
    pub install: String,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub prompt_hint: String,
    pub install: String,
}

/// 解析 SKILL.md：提取 frontmatter（--- 之间的 name/description/prompt_hint/install），
/// 以及正文中 "## 安装" 段落作为 install（若 frontmatter 未提供）。
fn parse_skill_md(content: &str, dir_name: &std::ffi::OsStr) -> Result<Skill, String> {
    let dir_id = dir_name.to_string_lossy();
    let (front, body) = split_frontmatter(content);
    let mut name = String::new();
    let mut description = String::new();
    let mut prompt_hint = String::new();
    let mut install = String::new();

    for line in front.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim().to_lowercase();
        let v = v.trim().to_string();
        match k.as_str() {
            "name" => name = v,
            "description" => description = v.clone(),
            "prompt_hint" => prompt_hint = v,
            "install" => install = v,
            _ => {}
        }
    }

    if name.is_empty() {
        name = dir_id.to_string();
    }
    if prompt_hint.is_empty() {
        prompt_hint = description.clone();
    }
    if install.is_empty() {
        install = extract_md_section(body, "安装");
    }

    let id = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>();
    let id = if id.is_empty() { dir_id.to_string() } else { id };

    Ok(Skill {
        id,
        name,
        description,
        prompt_hint,
        install,
    })
}

fn split_frontmatter(content: &str) -> (&str, &str) {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return ("", content);
    }
    let after_first = content.get(3..).unwrap_or("").trim_start();
    let second = after_first.find("\n---").or(after_first.find("\r\n---"));
    match second {
        Some(i) => {
            let front = after_first[..i].trim();
            let body = after_first.get(i + 4..).unwrap_or("").trim_start();
            (front, body)
        }
        None => (after_first.trim(), ""),
    }
}

fn extract_md_section(body: &str, title: &str) -> String {
    let needle = format!("## {}", title);
    let start = body
        .lines()
        .position(|l| l.trim().starts_with(&needle));
    let Some(start_i) = start else { return String::new() };
    let rest = body.lines().skip(start_i + 1);
    let mut lines = Vec::new();
    for line in rest {
        let t = line.trim();
        if t.starts_with("## ") && !t.starts_with(&format!("## {} ", title)) {
            break;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// 从目录加载所有 skills，目录不存在或为空时返回空列表。
pub fn load_skills(dir: Option<&str>) -> Vec<Skill> {
    let dir = dir.unwrap_or(DEFAULT_SKILLS_DIR);
    let path = Path::new(dir);
    if !path.is_dir() {
        debug!(dir = %dir, "skills 目录不存在，跳过加载");
        return Vec::new();
    }

    let mut skills = Vec::new();
    let read_dir = match std::fs::read_dir(path) {
        Ok(d) => d,
        Err(e) => {
            warn!(dir = %dir, err = %e, "读取 skills 目录失败");
            return skills;
        }
    };

    for entry in read_dir.filter_map(Result::ok) {
        let dir_name = entry.file_name();
        let sub = path.join(&dir_name);
        if !sub.is_dir() {
            continue;
        }
        let manifest_path = sub.join(SKILL_MANIFEST);
        let skill_md_path = sub.join(SKILL_MD);

        if manifest_path.is_file() {
            let content = match std::fs::read_to_string(&manifest_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(path = %manifest_path.display(), err = %e, "读取 skill 配置失败");
                    continue;
                }
            };
            let manifest: SkillManifest = match toml::from_str(&content) {
                Ok(m) => m,
                Err(e) => {
                    warn!(path = %manifest_path.display(), err = %e, "解析 skill.toml 失败");
                    continue;
                }
            };
            skills.push(Skill {
                id: manifest.id,
                name: manifest.name,
                description: manifest.description,
                prompt_hint: manifest.prompt_hint,
                install: manifest.install,
            });
        } else if skill_md_path.is_file() {
            let content = match std::fs::read_to_string(&skill_md_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(path = %skill_md_path.display(), err = %e, "读取 SKILL.md 失败");
                    continue;
                }
            };
            match parse_skill_md(&content, &dir_name) {
                Ok(skill) => skills.push(skill),
                Err(e) => {
                    warn!(path = %skill_md_path.display(), err = %e, "解析 SKILL.md 失败");
                }
            }
        } else {
            debug!(?dir_name, "无 skill.toml 且无 SKILL.md，跳过");
        }
    }

    if !skills.is_empty() {
        info!(dir = %dir, count = skills.len(), "已加载 skills: {:?}", skills.iter().map(|s| s.id.as_str()).collect::<Vec<_>>());
    }
    skills
}

/// 生成要追加到分类系统提示的段落。无 skills 时返回空字符串。
pub fn build_prompt_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n你还可以参考以下已安装的技能，在适当时生成对应命令：\n");
    for sk in skills {
        if sk.prompt_hint.is_empty() {
            continue;
        }
        s.push_str(&format!("- [{}] {}\n", sk.name, sk.prompt_hint));
    }
    s
}

/// 列出所有 skill 的摘要（id, name, description），用于回复「有哪些 skill」。
pub fn list_skills_summary(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return "当前未安装任何 skill。".to_string();
    }
    let mut s = format!("已安装 {} 个 skill：\n\n", skills.len());
    for sk in skills {
        s.push_str(&format!("• **{}** ({}) — {}\n", sk.name, sk.id, sk.description));
    }
    s.push_str("\n回复「怎么安装 <技能名>」可查看安装方式。");
    s
}

/// 根据失败命令内容匹配相关 skill，返回其 prompt_hint 拼接成的上下文，供「询问解决方式」时注入 LLM。
pub fn build_relevant_context_for_fix(skills: &[Skill], failed_command: &str) -> String {
    let cmd_lower = failed_command.to_lowercase();
    let mut hints = Vec::new();
    for sk in skills {
        if sk.prompt_hint.is_empty() {
            continue;
        }
        let relevant = match sk.id.as_str() {
            "screen_record" => cmd_lower.contains("ffmpeg") || cmd_lower.contains("avfoundation"),
            "screenshot" => cmd_lower.contains("screencapture") || cmd_lower.contains("scrot") || cmd_lower.contains("import"),
            _ => false,
        };
        if relevant {
            hints.push(format!("[{}] {}", sk.name, sk.prompt_hint));
        }
    }
    hints.join("\n\n")
}

/// 根据 id 或 name 查找 skill 并返回其安装说明。
pub fn get_install_instructions(skills: &[Skill], query: &str) -> Option<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    for sk in skills {
        if sk.id.to_lowercase() == q || sk.name.to_lowercase().contains(&q) {
            if sk.install.is_empty() {
                return Some(format!("「{}」当前无安装说明。", sk.name));
            }
            return Some(format!("**{}** 安装方式：\n\n{}", sk.name, sk.install));
        }
    }
    None
}
