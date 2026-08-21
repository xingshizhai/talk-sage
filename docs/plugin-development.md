# TalkSage 插件开发指南

本文面向需要为 TalkSage 增加转写过滤、实时分析或会后处理能力的开发者。内容以当前代码为准，既说明如何开发插件，也明确现有插件机制的边界和后续优化方向。

## 1. 机制定位

TalkSage 当前采用的是 **编译期注册、运行时配置** 的 Rust 内部插件机制：

- 插件与主程序一起编译，不支持从目录动态加载 `.dll`、`.dylib`、`.so` 或 Wasm；
- 插件通过稳定的 trait 接入转写生命周期，不直接侵入音频采集、VAD 或 ASR 主链路；
- 用户可在设置页或 `[plugins.<id>]` 配置中启停插件、覆盖参数；
- `/plugins` 元数据接口供前端动态生成设置控件；
- 插件执行顺序由内置插件清单顺序确定。

因此，它目前更准确的名称是“内部扩展点”，不是可独立安装的第三方插件平台。新增插件仍需要修改源码、重新编译和发布应用。

核心代码位于：

| 文件 | 作用 |
|---|---|
| `crates/talksage-plugins/src/registry.rs` | 插件契约、三类 hook、注册表和配置载体 |
| `crates/talksage-plugins/src/builtin.rs` | 内置插件清单、执行顺序、配置合并和 UI 元数据 |
| `crates/talksage-plugins/src/lib.rs` | 插件模块导出和 `PluginContext` 宿主能力 |
| `crates/talksage-pipeline/src/plugin_executor.rs` | 慢插件的有界队列、worker、panic 与结果隔离 |
| `crates/talksage-pipeline/src/service.rs` | 用户配置、场景策略、宿主能力的最终装配 |

## 2. 执行模型

一次 final 转写段的主要路径如下：

```text
ASR final segment
  → EventFilter（同步、按注册顺序）
      → None：丢弃，后续 observer 不再触发
      → Some(event)：事件进入 sink，并继续
  → SegmentObserver.should_trigger（同步）
  → SegmentObserver.skeleton（同步，本地即时结果）
  → 有界 PluginExecutor 队列
  → SegmentObserver.run（后台 worker，可访问 LLM/知识库）

session stop → flush → persist
  → SessionFinalizer（同步、按注册顺序、逐个隔离错误）
```

插件产生的事件不会再次进入 filter 链，避免递归。当前 filter 实际只处理 committed/final 段；partial 段默认不会触发 observer，只有显式返回 `accepts_speculative() == true` 才会处理。

### 2.1 EventFilter：实时快路径

适合：短段抑制、重复段消除、纯内存文本改写。

```rust
pub trait EventFilter: Send + Sync {
    fn filter(&self, event: DomainEvent) -> Option<DomainEvent>;
}
```

约束：

- 必须快速、确定、非阻塞；
- 不得进行文件、数据库、网络、LLM 等 I/O；
- 不得 panic；
- 对不认识的事件必须原样返回；
- 返回 `None` 会同时阻止事件展示、持久化及后续 observer，使用前要确认语义。

### 2.2 SegmentObserver：实时慢路径

适合：术语解释、翻译、知识库检索、对话指标和智能提示。

```rust
pub trait SegmentObserver: Send + Sync {
    fn name(&self) -> &'static str;
    fn should_trigger(&self, segment: &TranscriptSegment) -> bool;
    fn accepts_speculative(&self) -> bool { false }
    fn skeleton(&self, segment: &TranscriptSegment) -> Vec<DomainEvent>;
    fn run(
        &self,
        segment: &TranscriptSegment,
        context: &PluginContext,
    ) -> anyhow::Result<Option<DomainEvent>>;
}
```

执行规则：

- `should_trigger` 和 `skeleton` 仍在实时调度线程中，必须只做便宜的内存计算；
- `skeleton` 用于先显示“处理中”的即时结果，不需要时返回空数组；
- `run` 进入固定 worker 池，可调用 LLM 或知识库；
- 队列有界，积压时新任务会被丢弃，以保证实时转写优先；
- worker 会捕获 panic；会话停止、执行超时或取消后，结果会被丢弃；
- `Ok(None)` 表示正常但没有结果；`Err` 表示执行失败，执行器会记录插件名、耗时和错误。不要用 `.ok()?` 吞掉外部调用错误。
- 内置 LLM provider 设置连接、读写和总超时，总时限短于执行器的 15 秒 deadline；自定义外部客户端也必须遵守这一约束。

插件实例通过 `Arc` 被多个 worker 共享。内部可变状态必须使用 `Mutex`、原子变量等同步机制，并避免长时间持锁。不要用单个共享 `pending_id` 表示多个并发任务；应按 segment/result id 保存关联关系，否则并行完成时可能串结果。

### 2.3 SessionFinalizer：会后处理

适合：质量评估、会后 webhook、会话索引或导出。

```rust
pub trait SessionFinalizer: Send + Sync {
    fn name(&self) -> &'static str;
    fn finalize(&self, context: &FinalizeContext) -> anyhow::Result<()>;
}
```

`FinalizeContext` 当前只提供 `session_id`。所需宿主能力应在 `Plugin::register` 时从 `PluginContext` 获取，并捕获到 finalizer 实例中。finalizer 按注册顺序逐项运行，每项在独立命名线程中执行，默认 deadline 为 10 秒；普通错误、panic 和 timeout 会分类记录，并继续执行其他 finalizer。

Rust 无法安全强杀线程。超过 deadline 后宿主会停止等待，超时 finalizer 可能仍在后台自行收尾，其迟到状态不会被计为成功。因此 finalizer 自己调用的数据库或网络客户端仍须设置更短的超时，不应把宿主 deadline 当作取消机制。正常完成时顺序严格成立；某项超时后，后续项可能与它的后台尾部重叠，有强依赖的插件应共享一个 finalizer 或确保上游操作具有可靠的内部超时。

## 3. 插件定义与配置

每个插件实现统一入口：

```rust
pub trait Plugin: Send + Sync {
    fn descriptor(&self) -> &'static PluginDescriptor;
    fn default_config(&self) -> PluginConfig;
    fn register(
        &self,
        config: &PluginConfig,
        context: &PluginContext,
        hooks: &mut HookRegistry,
    );
}
```

约定：

- descriptor 的 `id` 使用稳定、唯一的 `snake_case` 英文标识；它会成为配置键和 API 标识；
- descriptor 集中声明 `label`、`description`、`category`、`phase`、所需 `capabilities`、`host_managed` 字段和 `after` 顺序依赖；
- 每个默认配置必须包含布尔字段 `enabled`；
- 默认值就是当前 UI 的类型来源：布尔值生成开关、数字生成数字框、字符串生成文本框；
- 当前配置只支持对象顶层的浅合并；嵌套对象会被整体替换；
- 读取参数要提供与默认配置一致的 fallback；
- 保存入口会拒绝已知插件中的未知字段、错误 JSON 类型和非对象配置；运行装配时会再次校验，防御手工修改的非法配置；
- `/plugins` 同时返回兼容设置页的 `schema` 默认值和机器可读的 `config_schema`；当前结构化 schema 覆盖类型、默认值和未知字段策略；
- 数值范围、枚举和跨字段约束需要插件覆写 `validate_config` 补充，尚未全部声明到元数据中。

配置的优先级为：

```text
插件 default_config
  < 用户 [plugins.<id>] 配置
  < 宿主能力和当前场景的最终裁决
```

示例：

```toml
[plugins.keyword_alert]
enabled = true
keyword = "银行卡密码"
```

`category = Analysis` 的插件还受场景 `plugin_allowlist` 控制。allowlist 只能关闭插件，不能覆盖用户的 `enabled = false`。由宿主控制的配置键写入 descriptor 的 `host_managed`，设置页会将它们置灰；宿主仍需在 `plugin_overrides_for` 中提供真正的最终覆盖。

## 4. 完整开发示例

下面实现一个只做本地计算的关键词提醒 observer。

### 4.1 创建插件模块

新建 `crates/talksage-plugins/src/keyword_alert.rs`：

```rust
use std::sync::Arc;

use serde_json::json;
use talksage_core::{DomainEvent, TranscriptSegment};

use crate::{
    HookRegistry, Plugin, PluginCategory, PluginConfig, PluginContext,
    PluginDescriptor, PluginPhase, SegmentObserver,
};

const DEFAULT_KEYWORD: &str = "银行卡密码";

struct KeywordAlertObserver {
    keyword: String,
}

impl SegmentObserver for KeywordAlertObserver {
    fn name(&self) -> &'static str {
        "keyword_alert"
    }

    fn should_trigger(&self, segment: &TranscriptSegment) -> bool {
        !segment.is_partial && segment.text.contains(&self.keyword)
    }

    fn skeleton(&self, _segment: &TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }

    fn run(
        &self,
        _segment: &TranscriptSegment,
        _context: &PluginContext,
    ) -> anyhow::Result<Option<DomainEvent>> {
        // 换成项目已有、语义匹配的 DomainEvent 变体。
        // 不要为绕过类型系统拼装 JSON 字符串。
        Ok(None)
    }
}

pub struct KeywordAlertPlugin;

impl Plugin for KeywordAlertPlugin {
    fn descriptor(&self) -> &'static PluginDescriptor {
        static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
            id: "keyword_alert",
            label: "关键词提醒",
            description: "在转写中发现指定关键词时生成提醒",
            category: PluginCategory::Analysis,
            phase: PluginPhase::Observer,
            capabilities: &[],
            host_managed: &[],
            after: &[],
        };
        &DESCRIPTOR
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({
            "enabled": false,
            "keyword": DEFAULT_KEYWORD,
        }))
    }

    fn register(
        &self,
        config: &PluginConfig,
        _context: &PluginContext,
        hooks: &mut HookRegistry,
    ) {
        hooks.add_observer(Arc::new(KeywordAlertObserver {
            keyword: config.get_str("keyword", DEFAULT_KEYWORD),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_safe_and_disabled() {
        let config = KeywordAlertPlugin.default_config();
        assert!(!config.enabled());
        assert_eq!(config.get_str("keyword", ""), DEFAULT_KEYWORD);
    }

    #[test]
    fn registers_one_observer() {
        let mut hooks = HookRegistry::default();
        let mut config = KeywordAlertPlugin.default_config();
        config.merge(&json!({"enabled": true}));
        KeywordAlertPlugin.register(&config, &PluginContext::new(), &mut hooks);
        assert_eq!(hooks.observers().len(), 1);
    }
}
```

真实插件应返回已有的 `DomainEvent`，或先在 `talksage-core` 中设计一个可被桌面、WebSocket、CLI 和持久化层共同理解的新事件。新增事件时必须检查所有 transport 的序列化和前端消费逻辑。

### 4.2 导出并注册

在 `crates/talksage-plugins/src/lib.rs` 增加：

```rust
pub mod keyword_alert;
```

在 `crates/talksage-plugins/src/builtin.rs` 引入插件，并在 `builtin_plugins()` 的正确位置增加：

```rust
Box::new(KeywordAlertPlugin),
```

列表顺序就是执行顺序。插件分类、能力和顺序约束来自 descriptor，不再维护 `ANALYSIS_PLUGIN_IDS` 或 `HOST_MANAGED_KEYS` 平行清单。如果它属于分析能力，仍需按产品意图更新配置 crate 中各场景的 `plugin_allowlist`；pipeline 契约测试会检查场景全集与 descriptor 是否一致。如果字段由宿主决定，还必须在 `plugin_overrides_for` 中真正覆盖它。

### 4.3 注入宿主能力

已有 observer 可使用：

- `context.llm`：LLM provider；
- `context.kb`：知识库；
- `context.translation`：当前会话的显式翻译策略。

会后插件已有 `quality`、`webhook` 两个最小权限 trait。新增数据库、网络或系统能力时，不要把整个 `TalkSageService`、配置管理器或数据库连接暴露给插件。应在 `talksage-plugins` 定义最小能力 trait，由 pipeline 实现并在 composition root 注入。

同时把能力写入 descriptor 的 `capabilities`。`build_registry_with_report` 会检查 `PluginContext`：缺少任一能力时不注册 hook，并返回 `unavailable` 和具体的 `missing_capabilities`。插件在 `run` 中仍应防御能力意外缺失，避免未来调用方式变化后 panic。

装配报告有四种状态：

| 状态 | 含义 |
|---|---|
| `active` | 配置有效、已启用、能力齐备，hook 已注册 |
| `disabled` | 用户配置或场景策略关闭 |
| `unavailable` | 已启用，但缺少 LLM、知识库、翻译策略或会后宿主能力 |
| `invalid_config` | 配置字段、类型或插件业务校验失败 |

真实会话使用带报告的入口并记录异常状态。设置页会显示当前场景和宿主能力下的状态；REST 客户端可调用 `GET /api/plugins/status`，Tauri 客户端可调用 `list_plugin_status`。状态预检不会启动音频设备或 ASR 模型，但会检查 LLM、会话存储，并索引当前知识库目录，因此它反映的是“现在启动会话时能否注册”，不是仅依据配置文件作出的静态猜测。单元测试和不关心诊断的调用可继续使用 `build_registry` 便捷包装。

依赖缺失时当前有两种选择：

- 能安全降级的插件在 `register` 中不注册对应 hook，并记录清晰日志；
- 必须依赖该能力的功能应默认关闭，避免设置页显示“已启用”但实际无效。

### 4.4 更新前端和 API

普通的 bool、number、string 顶层配置无需手写前端控件，`GET /plugins` 会从默认配置生成元数据。仍需验证：

- 设置页能显示正确 label 和字段；
- 保存后 TOML 中生成 `[plugins.<id>]`；
- 重启后配置仍生效；
- host-managed 字段被正确置灰；
- 新 `DomainEvent` 在 Tauri、WebSocket、CLI 和历史会话中都有合理行为。

当前结构化元数据还不支持说明文字、数值范围、枚举、密码输入或嵌套表单。需要这些能力时，应扩展 `config_schema`，不能依赖前端根据字段名猜测。

## 5. 测试要求

每个插件至少应覆盖：

1. id、label 和默认配置；
2. `enabled = false` 时不会被注册；
3. 用户配置覆盖确实传入 hook；
4. 触发和不触发条件；
5. 缺少 LLM、知识库或宿主能力时的行为；
6. 外部调用失败、空响应和重复响应；
7. 多线程并发时内部状态不会串扰；
8. 若有顺序依赖，增加显式顺序不变量测试；
9. 若属于分析类，验证各场景 allowlist；
10. 若新增事件，验证服务 API 和前端消费。

推荐命令：

```bash
cargo test -p talksage-plugins
cargo test -p talksage-pipeline
cargo test -p talksage-config
cargo test -p talksage-server
cd web && npm test
```

提交前运行完整检查：

```bash
./scripts/talksage.sh test
```

## 6. 性能与安全清单

### 实时性能

- filter、`should_trigger`、`skeleton` 不做 I/O，不做大文本解析；
- 为 LLM/知识库插件设置冷却、去重或采样策略；
- 不假设 observer 任务一定执行：队列满、停止或超时都可能丢结果；
- skeleton 与 final 结果应使用稳定 result id 关联；
- 不无限积累 `seen`、缓存或每会话状态；
- 外部客户端本身必须设置连接和读取超时。

### 数据与安全

- 向外部 LLM/webhook 发送转写前，应让用户明确启用；
- API key 不应放进普通插件元数据或日志；当前 schema 不支持 secret 字段，应继续使用宿主的安全配置；
- 日志避免记录完整敏感对话，优先记录 id、耗时和截断摘要；
- 插件只获取完成任务所需的最小宿主能力；
- 不在插件中自行启动无法关闭的永久线程。

## 7. 现有机制评估

### 已经比较完善的部分

| 方面 | 评价 |
|---|---|
| 生命周期分层 | filter、observer、finalizer 边界清晰，适合实时语音产品 |
| 实时链路保护 | 慢任务使用固定 worker 和有界队列，不会无限创建线程或增长内存 |
| 故障隔离 | observer 与 finalizer panic 被捕获；超时/取消后的结果被丢弃；单项失败不阻断后续项 |
| 依赖方向 | 通过 `PluginContext` 和最小能力 trait 注入，插件 crate 不反向依赖 pipeline/session |
| 配置归属 | 默认值属于插件；用户覆盖、场景裁决层次明确 |
| UI 扩展 | 元数据接口和动态表单减少了新增普通插件时的前端硬编码 |
| 可测试性 | 已有 id 唯一、默认配置、注册开关、顺序依赖、宿主裁决等契约测试 |

### 仍不完善的部分

| 优先级 | 问题 | 风险与建议 |
|---|---|---|
| 已完成（第二阶段） | 内置外部调用真实超时 | LLM 与 webhook 已设置 connect/read/write/overall timeout，LLM 总时限短于 observer deadline；第三方 provider 仍必须遵守契约，中期可改为可取消 async task |
| 已完成（第一阶段） | 基础配置校验 | 已增加 `validate_config`、保存入口错误反馈、启动二次校验及 `config_schema`；下一步补充范围、枚举、字段说明和 UI 展示 |
| 已完成（第六阶段） | 注册状态不可诊断 | `build_registry_with_report` 已区分 active、disabled、unavailable、invalid_config，报告缺失能力和配置问题；真实会话记录异常状态，REST/Tauri 暴露当前状态，设置页直接展示，CLI 展示声明的能力要求 |
| P1 | 元数据表达力不足 | 增加 description、plugin version、API version、字段 label/help、range、enum、secret、required、host-managed reason |
| 部分完成（第三阶段） | finalizer 隔离 | 已增加独立线程、10 秒默认 deadline、panic 捕获和 completed/failed/timed_out/panicked 分类报告；下一步增加显式依赖声明，避免超时上游与下游发生语义重叠 |
| 部分完成 | 插件可观测性 | 已统计 submitted、dropped、success、no-result、failed、timeout、panic、canceled 和平均耗时并在会话结束记录；下一步按插件拆分并暴露到 doctor/诊断页 |
| P1 | 插件并发契约不够显式 | 当前同一实例可被多个 worker 并发调用。文档化并增加并发测试；结果关联改为以 job/result id 为键 |
| 已完成（第四阶段） | 插件静态描述分散 | `PluginDescriptor` 已集中描述身份、说明、分类、阶段、能力、host-managed 字段和顺序依赖；场景模板作为产品策略仍独立存在并由 pipeline 契约测试校验 |
| P2 | `PluginContext` 随能力线性增加字段 | 短期可接受；能力增多后改为类型化 capability registry，但要保留最小权限和可发现性 |
| P2 | 仅支持编译期内置插件 | 如果产品确实要开放第三方安装，优先考虑进程外插件或 Wasm ABI；不要直接加载不受信任的 Rust 动态库 |

## 8. 建议演进路线

### 第一阶段：把现有内部插件平台做稳

1. 在现有 descriptor 与结构化 schema/validation 上补充范围、枚举和字段说明；
2. 在已接入设置页和 API 的注册状态基础上，补充 `doctor` 的完整宿主能力预检；
3. 为所有外部 provider 增加强制 timeout/cancellation；
4. 增加插件级指标和诊断输出；
5. 在已有 finalizer panic/timeout 隔离上增加依赖声明与状态诊断界面。

### 第二阶段：降低内部插件接入成本

1. 将 schema 的字段级 UI 信息也收敛到 descriptor 体系；
2. 用契约测试自动验证 id、默认值、schema、host-managed 覆盖和场景声明；
3. 提供 `examples/plugin-template` 或脚手架命令；
4. 将稳定的插件 API 与具体业务事件分层，减少核心事件变更带来的连锁修改。

### 第三阶段：仅在有明确生态需求时开放第三方插件

第三方插件涉及 API 兼容、签名校验、权限、资源限额、崩溃隔离和升级策略。推荐采用：

- **进程外协议**：隔离最好，适合 Python/其他语言和需要 GPU 的扩展；
- **Wasm/WASI**：适合纯计算和受限 I/O，可控制权限和资源；
- 不推荐直接将第三方原生动态库载入桌面主进程，ABI、崩溃和安全风险都较高。

在进入这一阶段前，应先定义 `plugin_api_version`、插件 manifest、能力权限、进程通信协议、超时/内存限制、安装来源与签名验证。

## 9. Pull Request 检查表

- [ ] 插件 id 唯一、稳定，默认配置包含 `enabled`
- [ ] 选择了正确 hook，没有在实时同步路径做 I/O
- [ ] 配置 fallback 与默认值一致，默认行为安全
- [ ] 依赖通过最小能力注入，缺失依赖时行为可解释
- [ ] 分析类和 host-managed 声明已同步
- [ ] 执行顺序及顺序依赖有测试保护
- [ ] 并发状态、冷却、去重和缓存有边界
- [ ] 外部请求具有 timeout，不泄露密钥和完整敏感文本
- [ ] 设置保存、重启加载和 `/plugins` 元数据已验证
- [ ] 新事件在所有 transport、UI 和持久化路径中已处理
- [ ] 单元测试、pipeline 测试和完整脚本通过
