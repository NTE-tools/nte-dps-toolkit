# AGENTS.md

适用于整个仓库。优先级：用户当次明确要求 → 目标目录更深层的 `AGENTS.md` → 本文件。文档与代码不一致时以当前代码和可运行的测试为准。

本项目的主要技术债不是“校验不够”，而是**校验太多、边界外溢、函数太长、测试结构失衡**。本文件的目的是让每次改动都更短、更直接、边界更清晰。

## 1. 工作方式

- 交付满足当前需求的最小正确改动。不预建扩展点、兼容层、开关或“以后可能用到”的参数。
- 新规则约束**新写和被修改的代码**。修改某个函数时，顺手清理该函数内违反本文件的代码（重复校验、死分支、测试钩子）；不在无关任务里顺手做整仓清理，整片存量清理作为单独任务推进。
- 删除优于保留，内联优于抽象，直接调用优于间接层。只有一个调用方的 wrapper、trait、工厂、配置参数，没有存在理由就不要写。
- 发现问题先读代码确认，不靠猜；能用编译器、类型和现有测试证明的事，不另写检查。

## 2. 架构边界（硬约束）

- 桌面 UI 唯一实现：**Tauri 2 + Vite + React + TypeScript + Tailwind CSS + shadcn/ui**。
  - Rust 核心（`src/`）是领域状态、协议解析、数据计算、插件通信、历史与持久化、更新状态机的唯一事实源；
  - `src-tauri/` 只做薄 adapter（窗口管理、Tauri command 参数转发、Channel 订阅桥接、DTO 投影），不承载协议命令 ID、重试/确认状态机、DPS 业务运算或持久化规则；
  - React 只渲染 read model 和维护 UI-only state，不直接解析原始 `@res/data/` 游戏表或复制 reducer、计算、history、持久化规则。
- `nte-core --no-default-features --features cli` 不依赖 Tauri、WebView、egui、eframe、wgpu 或 Win32 窗口/DWM/热键库。不恢复旧 `gui` Feature 或平行 UI 状态机。
- 原生扩展只通过外部编译的 `UETools-NTE` Toolkit 插件及通用 JSON Forms 面板接入（`get_plugin_form` / `invoke_plugin_form_action`）。旧 Mod 脚本工作区（`mod_scripts`）、`mod_sdk`、`mod_studio` 运行时与 v7 `mods_plugin` 管道已下线，不做兼容层，也不恢复已关闭的研究功能。

## 3. 信任边界：只在这里校验

**外部输入**（必须校验，且只在入口校验一次）：

| 边界 | 入口 |
| --- | --- |
| 抓包帧、pcapng、回放/历史/队伍导入文件 | `src/engine/capture.rs` 导入入口、`src/storage/` 读取入口 |
| 原生 Toolkit / Loader IPC、插件表单与返回数据 | `src/platform/toolkit*.rs`、`src/platform/capture_pipe.rs`、`src/core/toolkit*.rs` |
| CLI stdin JSON-RPC | `src/api/request.rs` |
| 网络（updater、Mod 市场、深渊远端数据） | `src/platform/update_http.rs`、`src/core/mod_market.rs`、`src/storage/abyss_remote.rs` |
| 本地磁盘与更新事务（config、i18n、updater transaction） | 对应 `storage` load 函数、`src/platform/update_install.rs`（校验路径锚定与完整性） |
| WebView / Tauri 安全边界 | `src-tauri/tauri.conf.json` CSP、`src-tauri/capabilities/`、Tauri command 窗口权限校验 |

在这些入口：外部数据不得导致 panic；文件先用 metadata 限制大小，再解析成**带类型的值**（parse, don't validate）；错误使用已有的 typed error 和稳定 code。

**不是边界**（不得再校验）：

- Rust 模块之间的内部调用；Rust 刚产出或刚持久化的数据。
- Rust → Tauri → React 的 DTO。前端与 Rust 打包在同一个二进制里，不存在版本漂移。DTO 与后端共享枚举的 TS 类型一律由 `ts-rs` 生成到 `frontend/src/lib/tauri/generated/`，前端直接 import，**禁止手写镜像类型或 runtime validator**。
- React → Tauri command 参数。用 serde 类型（含 `#[serde(rename_all)]` 枚举）直接反序列化，不手写 `parse_*`。
- 已被上游类型或入口保证的值，例如 enum、非空、范围、已排序、“恰好一个 live row”。

由此推出的禁止项：

- 同一份数据在第二层再次检查 size、count、range、budget，或在推送前额外 `serde_json::to_vec` 做二次序列化检长。同一概念只保留一个 `MAX_*` 常量，其他地方引用它。
- 对内部计数器、索引、长度使用 `saturating_*` / `checked_*` / `.max(0)` 来“防御”不可能的溢出或下溢。不变量用普通运算；需要表达时用 `debug_assert!`。只有在值确实可能越界时才饱和。
- 用 `unwrap_or_default()`、`.ok()`、`?? 0`、`?? []`、`if let Some(..) else {}` 静默吞掉“不应发生”或“需要显式失败”的状态。
- 区分 panic 与错误（Release 启用 `panic = "abort"`）：**仅编译期/静态不变量**允许 `expect("原因")` 或 `unreachable!`；凡受外部数据、协议变化或运行期可变状态影响的路径一律返回 typed error，不得用 `expect()` 直接拉崩进程。
- 永远不会被构造的错误变体；只做改名或吞掉原始 code 的错误映射函数。一个领域只写一次 `From<DomainError> for CommandError`，保留原始 code。
- 为不存在的威胁增加检查，或为内部 DTO 增加版本号常量/运行时版本比对。版本号只用于真正跨发布边界的格式：CLI JSON-RPC、持久化文件、updater 协议、Toolkit 插件 ABI。

## 4. 死代码

- 不写 `#[allow(dead_code)]`、`cfg_attr(..., allow(dead_code))`。用不到就删；只有测试用就移进 `mod tests`。
- 生产代码中不放 `#[cfg(test)]` 的函数、字段、计数器、访问器、常量或“test-only 复制版”。测试必须驱动真实生产路径；需要测试的行为若无法从公开入口观察，应改进设计而不是开后门。
- 前端不为测试保留 `create*Client(transport)` 工厂、可选 DI 依赖参数或 test-only export；测试直接 mock `invoke` 或对应模块。
- 删除功能时一并删除它的命令注册、client 方法、hook、样式、i18n key（`zh-CN.json` 与 `ja.json` 同步）、测试和文档。不写断言“旧功能已不存在”的测试。
- 兼容/迁移代码（`legacy_*`、`migrate_*`、`compatibility_*`）必须在注释中写明移除条件或版本；没有明确条件的兼容层不新增。
- 已 export 但只在本文件使用的符号去掉 `export` / `pub`。

## 5. 可读性

- 一个函数只做一个阶段的事。热路径（如 `process_capture_frame`）只允许变短：新增逻辑放进独立的阶段函数，不往大函数里继续加分支。
- 修改一个已超过约 150 行或嵌套超过 4 层的函数时，先把要改的那一段提取成命名函数再改。这是默认做法，不是硬阈值。
- 协议、wire 格式与原生插件命令 ID 中的魔法数字用命名常量；同一数值的不同含义使用不同常量，同一含义只定义一次。
- 同一词汇表在 Rust 端用一个 serde enum 定义并导出到 `frontend/src/lib/tauri/generated/`，前端直接 import；仅纯前端 UI 私有词汇表才在 TS 端单独 export 一份字面量联合类型。
- 重复 3 次以上的样板（锁获取、Channel subscribe/unsubscribe、流订阅 hook、窗口 blur 关闭、错误构造、窗口几何保存）提取成一个 helper；2 次以内允许重复。
- Rust 文件中测试超过约 1,000 行时，放到同目录 `tests.rs`（`#[cfg(test)] mod tests;`），不和生产代码混在一个文件里。
- React 组件超过约 300 行，或 `useState` 超过约 6 个时，修改它就按视觉区块拆子组件，或把独立状态提取成 hook。effect 依赖不得包含每帧变化的 snapshot；用 ref、派生状态或 `useEffectEvent`。
- 异步请求防竞态只用一种机制：effect 内的 `active` 标志，或一个 generation ref。不要叠加 `mounted` + `generation` + `active` 三重守卫。

## 6. 测试

测试的目的是锁定**用户可观察的行为**和真实缺陷，不是刷覆盖率或证明代码写过。

**要写的测试：**

- 修复缺陷时：写一个能复现该缺陷的回归测试（不因代码行数少于测试行数而省略夹具）。
- 新增行为时：每个行为一个正常路径测试，外加真实可能出现的失败路径；外部输入入口要有畸形输入测试。
- 同一行为的多个输入变体：写成一个表驱动测试，不写 N 个近似复制的测试。
- 前端测试：除纯模型函数外，涉及交互、状态流转、订阅 hook、快捷键与竞态修复时，应覆盖真实交互/状态行为，而非仅依赖 `renderToStaticMarkup` 静态字符串断言。

**不写、并且在修改相关代码时删除的测试：**

- 断言 serde 字段名或 camelCase 的测试（如 `value["someKey"]`）。每个 DTO 族最多保留一个往返测试。
- 逐个镜像 `invoke` 命令名和参数的 client 测试。
- 前端 contract validator 针对每个字段的接受/拒绝测试。
- 以 `?raw` 读取源码后断言文本的测试，以及断言 CSS class 字符串的测试。
- 断言私有实现形状的测试：访问次数、cache 命中计数、`Arc::ptr_eq`、内部容量、调用顺序。
- 依赖 unwind 的互斥锁 poison 恢复测试（Release 为 `panic = "abort"`）。
- 只测试 std、serde、React 或第三方库本身行为的测试。
- “已删除功能不存在”的负向断言；仅为迁移临时写的守卫测试。
- 近似重复测试。保留覆盖最完整的一个，或合并为表驱动。

**附加约束：**

- 纯重构不新增测试，现有测试保持通过即可。
- 不为测试新增生产抽象、参数（如 `_with_limits`、`*_with` 注入点）或仅测试使用的导出。

## 7. 前端 UI 与交互规范

样式入口、token 与布局细节以 [frontend/README.md](frontend/README.md) 的「UI 样式入口与约定」为准；本节为前端工程与界面的唯一规范（已吸收原 `frontend-guidelines.md` 的有效条款，面向 Windows 桌面应用，不包含移动端适配或无止境的装饰性打磨循环）。

**按窗口形态分层约束（避免不同场景强套一套外观，同时保持底层体系一致）：**

| 窗口/界面类别 | 定位与允许的差异 | 必须统一守住的底线 |
| --- | --- | --- |
| **控制台与常规桌面窗**（Console、Combat Details、Abyss Values） | shadcn/ui（Nova 风格）+ Radix Colors 克制扁平风格；密度只调间距与控件高度，**不改根 `html` font-size** | 复用 `PageHeader`/`PageScrollArea`/`FormRow`/`ActionNotice`/`ui/dialog`；统一字号、阴影、控件高度与间距 token |
| **透明游戏叠加层**（Main DPS、HUD、Notification Island） | 允许透明背景、强可读性文字描边、置顶/穿透与高信息密度布局 | 仍消费同一套语义色板、减少动效偏好与键盘/无障碍规则，不跌破 12px 最小字号 |
| **游戏图鉴与配装面板**（Character Data、Empty Curtain） | 允许角色立绘、属性视觉表达与 Canvas 网格 | 样式必须放在 `@layer components`（禁止无层级全局 CSS）；Canvas 共用主题语义色与含 `"Microsoft YaHei UI"` 的字体回退链；弹窗/交互与控制台统一 |

**可检查工程条款：**

- **组件与控件**：统一使用 `components/ui` 与 `components/nte`。文本输入用 `Input`，下拉用 `NativeSelect`，表单项用 `FormRow`（标签可关联聚焦/切换控件），浮层提示统一用 `ActionNotice`（不各页手写 portal/fixed `<Alert>`，也不把浮层 `ActionNotice` 塞进文档流），弹窗用 `ui/dialog` / `ui/alert-dialog`。禁止原生 `<select>`、原生 `<input type="text|checkbox">`（滑块/隐藏文件输入/Canvas 等业务控件除外）或原生 `<dialog>`。控制台长页用 `PageHeader` + `PageScrollArea`，不手写固定 `p-3`/`p-4` 滚动容器。
- **色彩与对比度**：统一基于 Radix Colors 3.0.0 映射到 `styles/theme.css` 语义 token（`text-success`/`warning`/`info`/`destructive`/`muted-foreground`、`bg-primary`/`card`/`muted` 等）。UI 外框不使用 `emerald-*`/`zinc-*` 等原始调色板类，也不在 `theme.css` 与游戏领域文件（如 `lib/character-color.ts`）外写 hex/rgb/oklch 字面量或多余的 `bg-[var(--x)]`。文字与交互态（含暗色模式悬停/关闭按钮及所有主题预设）需保证可读对比度。
- **字号与层级**：页面使用 3–4 档字号（12/14/20px，关键统计数字可用 24px），最小字号 12px，不写任意字号；优先通过字重、颜色、间距与留白建立层级。
- **边框、圆角与装饰**：使用 `--ui-border-width`（1x/100% DPI 屏幕默认 `1px` 防亚像素发虚，HiDPI `>= 1.5dppx` 使用 `0.5px`，高对比度模式 `1px`），不手写 `border-2` 外框；嵌套容器保持同心圆角；**禁止卡片左侧彩色竖条**（包括绝对定位色条、`border-l-2` 色条、inset 阴影条纹、Canvas 左侧竖条）；同一层级浮层（Dialog / Popover / Menu / Toast）使用统一阴影档位。
- **文案与国际化**：简洁自然、去冗余；不用中点“·”作分隔符（包括 Rust 生成给 UI 的文案）；不加眉题/uppercase overline；页面描述不复述标题。所有用户可见文案走 i18n，英文为 key，**同步维护 `res/languages/zh-CN.json` 与 `res/languages/ja.json`**（禁止在 TS 代码中硬编码覆盖词典），删除文案时同步删除两个词典中的 key。
- **交互与状态完整**：关键操作必须覆盖加载、成功、失败、空状态、禁用与键盘操作；破坏性操作（清空文件、批量卸装、重置数据）须有明确二次确认且主确认按钮使用 `variant="destructive"`；不得劫持全局 `Tab` 键或在文本输入框内全局屏蔽右键菜单。
- **动效**：克制、有明确状态或空间反馈才加动效；时长与缓动统一走 `lib/motion.ts` 与 CSS token（默认 `ease-out`，`120/180/250ms` 刻度），严格尊重减少动效偏好，不新增纯装饰性动效或每帧重播动效。
- **库选型**：UI 用 shadcn/ui（Base UI），图标用 Lucide，拖拽排序用 dnd-kit，动画用 Motion（`WindowMotionBoundary` 统一接入 `MotionConfig`），跨组件共享客户端状态用 Zustand，局部状态用 React 原生能力；可调分栏按需引入 `react-resizable-panels`。不新增手写指针拖拽排序、FLIP、数值插值或 `useSyncExternalStore` store。
- **样式层**：所有共享及页面自定义 CSS 必须归属 `@layer components`，不从祖先选择器覆盖子组件内部（`[data-slot]`、`> aside`/`> section`/`> div:first-child`、`textarea` 等）；`!important` 仅限透明窗口与减少动效两个平台例外。
- **无障碍**：纯图标按钮必须带 Tooltip 与 `aria-label`；所有表单控件必须有 `<label htmlFor>` 或 `aria-label`，校验失败透传 `aria-invalid`；自定义可点击元素必须有 `type="button"` 与 `focus-visible:` 焦点样式；Dialog/AlertDialog 必须有可访问标题。

## 8. 真实存在的运行时约束

只有在改动涉及对应能力时才需要考虑以下约束；不涉及时不为它们加代码或测试。

- **抓包线程**：不做磁盘 I/O（含 pcapng 写盘/复制）、大型序列化或深拷贝完整 `CombatState`/History。慢操作移到锁外或独立后台 worker 执行。
- **热锁**（`event_gate`、权威状态、capture/session 锁）：持锁期间不做 I/O、dialog、sleep、join、阻塞 send、IPC 或全量状态克隆；多锁保持一致加锁顺序。
- **Panic 与锁策略（Release 启用 `panic = "abort"`）**：生产环境不依赖 `catch_unwind` 或 `Mutex` poison 恢复来兜底状态一致性；**不再新增或维护 `lock_or_repair` / `poison_for_test` 等不可达的 poison 恢复状态机**。锁本身只保护纯内存操作并保持短临界区；需要容错的外部调用和状态转换在持锁外通过 `Result` 显式处理。
- **Tokio 异步线程**：异步命令与 `spawn_polling_stream` 等 Tokio worker 线程不得执行阻塞磁盘 I/O、OS 进程扫描或长时间自旋 sleep；阻塞操作一律放入 `spawn_blocking` 或独立托管线程。
- **revision**：只在用户可观察的状态变化时 bump；no-op 不 bump；packet、combat、presentation 三类 revision 不混用。
- **history**：保持 `bounded history rows + exactly one live row` 由 Rust 保证；live、import、replay 复用同一套领域逻辑。
- **Channel / worker**：可靠事件通道断开时立即停止 worker，不逐条 `let _ = send(..)` 静默丢弃；长期运行的 worker（含窗口防抖线程）必须有 Rust 侧 owner 和明确的 cancel/shutdown 路径；流通道推送不应包含隐藏写副作用。
- **`unsafe`**：保持最小范围，每个 `unsafe` 块/函数必须写明具体的 `SAFETY:` / `# Safety` 依据；禁止暴露接受未校验偏移量或指针的安全包装函数。

## 9. 需要先确认

用户已经明确要求且范围清楚的事项，不必重复确认。

- 新增或升级依赖：说明用途、许可证、体积和替代方案。前端用 pnpm（`frontend/pnpm-lock.yaml`）。
- 破坏性的跨边界变更：CLI JSON-RPC 协议、持久化文件格式、updater 协议、Toolkit 插件 ABI，以及删除用户数据。
- 修改 updater 或签名机制、`vendor/`、`[patch]`、发布 profile、Tauri capabilities 安全配置或 WebView CSP（特别是不扩大 `'unsafe-eval'` 范围）。
- 删除仍可能被用户使用的功能或迁移路径，例如 legacy JSON 回放、旧配置迁移。

## 10. 文件与 Git

- 保留无关的未提交改动。不创建 `.bak`、copy、snapshot 等备份文件，Git 就是回滚手段。本次产生的临时文件在结束前清理。
- 未经要求不 commit、push 或创建 PR；获准提交时只 stage 本次任务涉及的文件。
- 不提交以下内容：`target/`、`out/`、`logs/`、`data/`、`NTE_Assets/`、`UETools-NTE/`、`NTE-Drive-Calculator/`、`nte-resource-exporter/`、`Dumper-7/`、`tools/`、`site/assets/`、`node_modules/`、C# `bin/obj`、`.env`、抓包样本、解包数据。不把 payload、token、key 或本机完整路径（如 `.display()`）写入日志、错误信息、测试 snapshot 或 commit message。

## 11. 验证

本地运行与改动直接相关的检查；在 [.github/workflows/build.yml](.github/workflows/build.yml) 接入自动 PR/push 触发前，不要假设远程 CI 会自动替每次改动做全量兜底，重要改动请在本地跑齐受影响目标。

| 改动 | 本地验证 |
| --- | --- |
| 文档 | `git diff --check -- <files>` |
| 前端 | `pnpm --dir frontend format:check`、`lint`、`typecheck`，以及相关的 `vitest` 文件 |
| i18n / 文案 | `pwsh -NoProfile -File scripts/test_i18n_source_coverage.ps1`（确认 `zh-CN.json` 与 `ja.json` 对齐且无缺键） |
| Rust 核心 | `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`，以及 `cargo test <filter>` |
| Tauri adapter | 上一行命令加上 `--manifest-path src-tauri/Cargo.toml` |
| 带 `#[derive(TS)]` 的 DTO | 删除 `frontend/src/lib/tauri/generated/` 后运行 `cargo test --manifest-path src-tauri/Cargo.toml export_bindings` 和 `cargo test --features tauri_frontend --lib export_bindings`，提交生成结果 |
| 依赖或 feature | `pwsh scripts/verify_architecture.ps1`、`cargo check --no-default-features --features cli` |

- 依靠编译器、类型、clippy、oxlint 和行为测试来验证。不新增基于正则扫描源码的门禁脚本，也不为现有门禁脚本扩写规则；与本文件冲突的过时门禁规则应随对应清理一并删除。
- 本次引入的失败要定位根因并修复。与本次无关的失败只需记录命令和首个错误，不为了全绿去改无关文件。
- 窗口、HUD、游戏内行为需要用户在真实环境验收。未实际验证的内容在交付时明确标注为“未验证”。

## 12. 交付

说明改了什么、涉及哪些文件、运行了哪些验证命令及结果、哪些内容尚未验证，以及删除了多少行、新增了多少行。达到验收标准即停止，不顺手优化无关代码。

## 导航

| 主题 | 入口 |
| --- | --- |
| 包解析、战斗模型 | `src/engine/`、`tests/` |
| 状态归并、capture/history/replay | `src/core/reducer.rs`、`src/core/live_capture.rs`、`src/storage/history.rs` |
| CLI 协议 | `docs/CLI_PROTOCOL_ZH.md`、`src/api/`、`src/cli/` |
| Tauri 命令与 Channel | `src-tauri/src/state.rs`、`src-tauri/src/commands/`、`src-tauri/src/channels/`、`frontend/src/lib/tauri/`（`generated/` 由 ts-rs 生成） |
| 前端与 i18n | `frontend/README.md`、`frontend/src/`、`res/languages/zh-CN.json`、`res/languages/ja.json` |
| Windows、HUD、窗口 | `src/platform/`、`src-tauri/src/windows/` |
| Toolkit 插件与表单 IPC | `src/platform/toolkit.rs`、`src/platform/capture_pipe.rs`、`src/core/toolkit*.rs`、`src-tauri/src/commands/plugin_form.rs`；Loader/插件源码在私有 `UETools-NTE` 工作区 |
| 发布与更新 | `src/core/update.rs`、`src/platform/update_install.rs`、`docs/releases/` |
