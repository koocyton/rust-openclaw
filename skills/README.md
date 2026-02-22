# Skills 目录

本目录用于存放 **skills**（技能）：每个 skill 描述一类扩展能力，通过提示词注入让 LLM 在合适时生成对应命令，从而用「技能」解决更多类型的问题。

## 如何启用 Skills

1. 在项目根目录下创建或使用本 `skills` 目录。
2. 在 `config.toml` 中（可选）指定 skills 目录路径：
   ```toml
   skills_dir = "skills"
   ```
   不配置或留空时，默认使用项目根下的 `skills` 目录。
3. 在 `skills/` 下为每个技能建一个**子目录**，目录内包含 `skill.toml` 清单文件。
4. 重启 rust-bot 后，会加载所有合法 skill，并在意图分类时把技能说明注入系统提示。

## 如何安装单个 Skill

每个 skill 的安装方式由该 skill 的 `skill.toml` 中的 `install` 字段定义（依赖、命令、权限等）。

- 在 Telegram 中向 bot 发送：**「有哪些技能」**，可列出已安装的 skills。
- 发送：**「怎么安装 截图」**（或具体技能名），可查看该技能的安装说明。

## 如何添加新 Skill（安装方式）

1. 在 `skills/` 下新建子目录，建议用英文小写+下划线，例如 `my_skill`。
2. 在该目录下创建 `skill.toml`，格式如下：

```toml
# 唯一标识，建议与目录名一致
id = "my_skill"
# 显示名称（会出现在「有哪些技能」列表里）
name = "我的技能"
# 简短描述
description = "做某件事的扩展能力"
# 注入到 LLM 系统提示的说明：何时使用、推荐命令或示例（影响 LLM 何时生成什么命令）
prompt_hint = "当用户要求做 XXX 时，使用命令：yyy ...，结果保存到 /tmp/。"
# 安装说明（用户问「怎么安装 我的技能」时展示）
install = """
1. 安装依赖：sudo apt install xxx
2. 确保有权限执行 yyy
3. 可选：配置环境变量 ZZZ
"""
```

3. 保存后重启 rust-bot，新 skill 会被自动加载。

## 目录结构示例

```
skills/
├── README.md           # 本说明
├── screenshot/         # 截图技能
│   └── skill.toml
└── screen_record/      # 录屏技能
    └── skill.toml
```

## 通过 Skills 解决「任意问题」

- **内置能力**：LLM 默认会做意图分类、问答、生成 shell 命令并执行。
- **Skills 扩展**：每个 skill 的 `prompt_hint` 会追加到系统提示中，告诉模型「在什么场景下、用什么命令」。
- 通过不断增加 skills（如：截图、录屏、部署、监控、数据库查询等），并在 `prompt_hint` 里写清触发场景和命令形式，就可以让 bot 覆盖更多类型的问题，而不必改主程序代码。

## 注意事项

- 只有包含有效 `skill.toml` 的子目录才会被加载；无 `skill.toml` 或解析失败时该目录会被跳过并打日志。
- `prompt_hint` 不宜过长，建议一段话说明「何时用 + 用什么命令」即可，避免挤占过多 token。
- `install` 支持多行，可写依赖、步骤、权限等，便于用户自助安装该技能所需环境。
