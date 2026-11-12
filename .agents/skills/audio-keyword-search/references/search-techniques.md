# 声音设计关键词技巧

以下是资料归纳与本项目适配，不代表实库召回测试。社区经验用于提供创意方向；UCS 用于分类统一；编辑配方不属于 UCS 官方列表。

| 技巧 | 操作与例子 | 依据 |
| --- | --- | --- |
| 先事件，再限定 | `explosion` → `plasma explosion`；每次只增加一个限制，定位哪个词让结果消失 | [PSE 检索指南](https://blog.prosoundeffects.com/how-to-find-the-right-sound-effects) |
| 按可听维度描述 | 对象、动作、材质、节奏、距离、空间分别提取，`footstep gravel` 后再试 `boots gravel` | [PSE 特定音效](https://blog.prosoundeffects.com/how-to-find-a-specific-sound-effect) |
| 成品与素材双路径 | 找不到完整科幻爆炸，拆为音头、低频、纹理、尾音，各自检索 | [社区分层讨论](https://www.reddit.com/r/sounddesign/comments/qfklal/) |
| 同义词轮换 | explosion / blast / detonation 分别查；已有匹配后再加修饰 | [社区分层讨论](https://www.reddit.com/r/sounddesign/comments/qfklal/) |
| 用途词补充声源词 | `sweetener`、`bowed`、`pneumatic` 可打开设计方向；不是每个库都这样标注 | [社区灵感词](https://www.reddit.com/r/sounddesign/comments/bqjbcr/) |
| 保留原声源 | 煎炸声可为雨层提供质感，但原声源仍是 frying；创意映射不回写原 Metadata | [AudioPost 讨论](https://www.reddit.com/r/AudioPost/comments/1hcqeah/) |
| UCS 和普通词双查 | CatID `EXPLDsgn` 是有 UCS 命名的素材的入口；普通库仍查 explosion | [UCS](https://universalcategorysystem.com/) |
| 描述搜索优先 | 同时利用文件名、描述与已有 Metadata；避免仅凭简短文件名判断素材 | [Metadata 风格指南](https://www.asoundeffect.com/metadata-style-guide/) |
| 短词与缩写降权 | imp / fs / amb / ele 属于不同程度的简写；先用 impact / footsteps / ambience / electronic | 个人表与编辑适配 |
| 拼写变体并列 | scifi / sci fi / sci-fi，flyby / fly by / passby 单独尝试；不把变体全 AND 起来 | 个人表与当前解析器 |
| 先消歧再收窄 | drone=无人机或持续音；close=近距离或关闭；ring=鸣响、戒指或铃响 | 个人表与编辑适配 |
| 空间不能靠标签保证 | dry / wet 既可能描述混响也可能描述材质；close / distant 是提示，仍需试听 | 个人表与编辑适配 |
| 情绪落到声音特征 | “有重量”先尝试 low thump / sub boom / metal stress，“明亮魔法”试 shimmer / sparkle | 编辑设计建议 |
| 数值约束用真实过滤器 | “两秒内”可转 duration_max=2；“没有人声”不能自动伪装成已实现的全字段 NOT | 当前 SearchFilters 契约 |
| 区分声音结构 | charge → launch → impact → tail 是制作时间线，不是需要同时出现的元数据词组 | 编辑设计建议 |
| 片段匹配会引入噪声 | wind 可能匹配 window；用 wind gust 限定，或切换到适当范围，不声称引号可修复 | 当前 search_text.rs |

## 典型歧义

- `drone`：未给语境时并列 `quadcopter / drone propeller` 与 `tonal drone / dark drone`，不要默认排除其中一个。
- `cast`：施法、铸造、演员阵容均可能出现；魔法用 `spell cast`。
- `gear`：装备、齿轮、传动、枪械机构；与 gun / mechanical / movement 配对。
- `organic`：可以是有机材质听感、血肉或自然来源，不当作生物声检测标签。
- `clean`、`raw`、`dry`、`loop`：文件文字标签不能证明降噪、未经处理、无混响或无缝循环。

## 迭代上限

一次返回 3–5 条主查询。若用户授权实际搜索，先执行一条；根据真实命中再选一条放宽或收窄。自动迭代建议最多两轮；随后提供可见选择，不无界批量搜索。未命中并不证明库内不存在可用音频，只能说明当前文本检索没有找到。
