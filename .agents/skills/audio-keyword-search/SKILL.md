---
name: audio-keyword-search
description: 将声音设计自然语言需求转成 AudioLib Explorer 可执行的英文关键词组合，按需检索个人经验词表、UCS 分类和分层素材配方。用于找音效、扩展或收窄查询；不用于生成音频、自动修改 Metadata 或管理 AI 账号。
---

# Audio keyword search

输出少量能直接检索的词，而不是把需求逐字翻译。默认面向 AudioLib Explorer 的 Main 搜索范围。知识版本与来源见 [manifest](data/manifest.json) 和 [来源说明](references/sources.md)。

## 快速执行

1. 从需求提取：事件/声源、动作、材质、音色、空间/距离、时间结构、用途与明确排除项。缺少细节时先按合理默认给候选；只有会改变查询方向的歧义才提一个问题或并列分支。
2. 先查配方，再查少量关键词。把下面路径替换为本 skill 所在目录；脚本只读本地参考数据，不联网、不访问声音库。

   ```powershell
   pwsh -NoProfile -File <skill-dir>/scripts/lookup.ps1 -Query '我要设计科幻爆炸声' -Kind Recipe -Limit 3
   pwsh -NoProfile -File <skill-dir>/scripts/lookup.ps1 -Query '爆炸 能量 碎片' -Kind Keyword -Limit 12
   ```

   需要 CatID 时才用 `-Kind Ucs`；已知 CatID 可直接查询。无 PowerShell 时用 `rg -n -i` 搜 [recipes.jsonl](data/recipes.jsonl)、[keywords.jsonl](data/keywords.jsonl)、[ucs.jsonl](data/ucs.jsonl)。不要默认加载完整 UCS 或整张词表。
3. 先给 3–5 组直接查询；设计型需求再给 3–5 个素材层，每层 1–2 个候选。把同义/近义替代分开查，不把整套配方拼成一个 AND 条件。
4. 先守住核心事件；零结果时按“去掉风格 → 换同义/历史拼写 → 拆素材层 → 用户选择扩大范围”退让。结果过多则每次加一个材质、动作或距离条件。
5. 用户只要关键词时直接返回关键词；请求机器格式时按 [输出契约](references/output-contract.md) 返回 JSON。没有执行库查询时，明确“建议查询，尚未验证命中”，不编造文件、数量或可用性。

## 当前项目必须遵守的查询语义

- 搜索框是普通文本 AND，`OR` / `NOT` / 引号 / `*` 不是给用户使用的布尔/短语/通配语法。输出 `plasma explosion` 和 `energy burst` 两条独立查询，不输出 `(plasma OR energy) AND explosion`。
- 每条最多 500 个 Unicode 字符，不能含分号或 NUL；默认 1–3 个核心词。英文两个及以上字符有字母片段匹配，所以 `wind` 也可能召回 `window`。需要试听和加限定词，不能承诺整词匹配。
- Main 搜文件名及 Metadata；Filename 只搜文件名；Path 只搜目录；All 可包括路径。保留用户选定的目录、库与范围；不要静默扩大范围。
- `include_tags` / `exclude_tags` 只筛库标签，不是全字段 NOT。模型提出的“不要枪声”先作为待确认排除建议；仅在用户确定标签含义时转成标签过滤。
- UCS CatID 是分类/命名提示，可在包含它的文件名/Metadata 中查找；当前没有独立 UCS 分类过滤器。普通词查询和 CatID 查询并列，不把 CatID 变成必填条件。
- 跨层素材是设计替代，不等于词义相同。煎炸可模拟雨、玻璃可参与冰魔法，但不修改原始声源或现有 Metadata。

## 按需参考

- 需要查词与纠错：检索 [关键词数据](data/keywords.jsonl)。`P` 条目保留原表行号；`E` 是编辑补充。`alternatives` 是可另试的检索词，不保证同义。
- 需要技巧、歧义与拓展：读 [搜索技巧](references/search-techniques.md)。
- 需要字段语义与示例：读 [输出契约](references/output-contract.md)。
- 需要更新或溯源：读 [来源说明](references/sources.md)，官方 CatID 必须从当前数据验证，不凭记忆创造。

## 默认回答示例

需求：科幻爆炸。先试 `scifi explosion`、`energy burst`、`plasma explosion`。

| 素材层 | 独立查询候选 |
| --- | --- |
| 低频主体 | `sub boom`、`bass drop` |
| 冲击音头 | `metal impact`、`crack` |
| 能量纹理 | `electric arc`、`plasma` |
| 飞散碎片 | `metal debris`、`glass shatter` |
| 尾音 | `rumble`、`reverb tail` |

支持 UCS 命名的库可另试 `EXPLDsgn`、`DSGNBoom`。零结果先退到 `explosion` 或 `blast`。以上为建议查询，尚未验证命中。
