# 查询建议契约

这是 Agent 的参考输出格式，也是未来 `AiQueryPlan v1` 的建议输入，当前应用尚未实现导入。由客户端补齐并验证真实 library/task/request generations，不接受模型创造这些身份。

```json
{
  "schema_version": 1,
  "intent": "科幻爆炸",
  "assumptions": ["先查成品，必要时分层设计"],
  "queries": [
    {"text": "scifi explosion", "purpose": "成品", "source_ids": ["scifi-explosion"]},
    {"text": "energy burst", "purpose": "能量主体", "source_ids": ["scifi-explosion"]}
  ],
  "layers": [
    {"role": "低频", "queries": ["sub boom", "bass drop"]},
    {"role": "尾音", "queries": ["rumble", "reverb tail"]}
  ],
  "ucs_hints": [{"cat_id": "EXPLDsgn", "reason": "设计爆炸的分类候选"}],
  "exclusions": [],
  "broader_queries": ["explosion", "blast"],
  "narrower_queries": ["plasma explosion distant"],
  "executed": false
}
```

`source_ids` 必须来自实际检索结果，不得复制示例 ID 代替核对。每个查询独立执行。建议 1–5 条主查询、0–5 层、每层 1–2 条；最多 12 条可执行候选进入 GUI，更多候选留在参考数据中。查询中不得放原始 SQL、FTS5 运算符、shell 命令或供应商密钥。

`exclusions` 使用 `{ "text": "枪声", "target": "unresolved", "reason": "用户明确要求" }`。只有经用户确认且实际标签含义匹配时，执行层才转成 `exclude_tags`。当前没有任意全字段排除语法，也没有 UCS 专用过滤器。

核心验证：版本、未知字段策略、UTF-8/字符长度、总候选数、非空、禁用字符、CatID allowlist、来源 ID、去重。无需给语言模型的自报置信度赋统计含义。Jev 概率保留在独立决策记录中，不与检索命中率混用。
