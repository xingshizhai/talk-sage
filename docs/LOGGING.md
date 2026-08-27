# TalkSage v2 日志与调试指南

结构化日志（tracing）用于问题定位，**AI Agent 可通过日志文件分析并解决故障**。

## 1. 日志位置与格式

| 输出 | 位置/格式 |
|---|---|
| **文件**（主） | `TALKSAGE_LOG_DIR/talksage.YYYY-MM-DD.log`（默认 `<数据目录>/logs/`），**JSON lines**（每行一个事件，可直接解析） |
| 控制台 | 人类可读文本（级别着色），写到 **stderr**（避免污染 `--json` stdout） |

- 日志目录：`TALKSAGE_LOG_DIR`（未设时 `<数据目录>/logs`）；数据目录 `TALKSAGE_DATA_DIR`（默认 `~/.talksage`）
- 脚本运行（`talksage.ps1` / `talksage.sh`）时日志独立于数据目录，位于项目内 `logs/`
- 每日轮转（`talksage.YYYY-MM-DD.log`），多天日志并存

快速查看最近日志：

```bash
talksage logs                  # 最新 talksage*.log 尾部 200 行
talksage logs --lines 50
talksage --json logs --lines 20
./scripts/talksage.sh logs     # 构建脚本：固定 tail 最近 50 行
```

**JSON lines 示例：**

```json
{"timestamp":"2026-08-19T01:05:24.440741Z","level":"INFO",
 "fields":{"message":"流[我] 就绪: engine=paraformer-zh model=... 加载耗时=1.2442789s",
            "log.file":"crates\\talksage-pipeline\\src\\lib.rs","log.line":368},
 "target":"talksage_pipeline"}
```

## 2. 级别控制

优先级：`--log-level` > `--verbose` > `RUST_LOG`/`TALKSAGE_LOG` > 默认 `info`

```bash
# 环境变量
$env:TALKSAGE_LOG = "debug"        # 或 RUST_LOG
$env:TALKSAGE_LOG = "talksage=debug,info"   # 按 crate 过滤

# CLI 参数
talksage --verbose listen --input a.wav
talksage --log-level debug serve
```

级别：`error`（故障）→ `warn`（异常但可继续）→ `info`（生命周期/关键操作）→ `debug`（细粒度）→ `trace`（最详细，含事件级）

## 3. 已埋点内容

| 事件 | 级别 | 字段 |
|---|---|---|
| 应用/CLI 启动 | info | 版本 |
| 流就绪（ASR 加载） | info | 引擎、模型路径、**加载耗时** |
| 管道事件循环 | info | 流数 |
| 插件触发 | debug | 插件名、段摘要 |
| 插件完成 | info | 插件名、**耗时**、是否有结果 |
| 启动失败（流/管道） | error | 错误详情（含模型路径） |
| 音频设备信息 | info | 设备、采样率、声道 |
| 会话/纪要 | info | 落库、生成结果 |

## 4. AI Agent 分析指引

1. **定位入口**：`talksage logs`，或直接读 `<TALKSAGE_LOG_DIR>/talksage.<最近日期>.log`（默认 `<数据目录>/logs/`）
2. **过滤关键信号**：
   ```bash
   # 只看错误与警告
   Select-String -Pattern '"level":"(ERROR|WARN)"' talksage.*.log
   # 只看管道/插件问题
   Select-String -Pattern 'talksage_pipeline' talksage.*.log
   ```
3. **常见诊断模式**：
   - `启动失败: ...` → 配置/模型路径问题（日志含具体路径）
   - `流[...] 就绪` 缺失 → ASR 模型加载失败（查 error 行）
   - `插件[...] 完成: 有结果=false` → LLM 未配置或调用失败（对照配置 api_key）
   - `加载耗时` 异常长 → 首次加载/磁盘/GPU 问题
4. **提高日志级别复现**：`talksage --verbose <命令>` 或 `TALKSAGE_LOG=trace` 重跑，对比日志
5. **控制台无输出但怀疑程序运行**：查日志文件（文件输出独立于控制台）

## 5. 调试辅助

- `talksage logs [--lines 200]`：打印最新日志文件尾部（与桌面调试窗口同一套文件）
- `talksage doctor`：环境/模型/配置诊断（配合日志确认）
- `talksage config path`：打印配置文件、数据目录、日志目录
- `TALKSAGE_LOG_DIR`：覆盖日志目录（默认 `<数据目录>/logs`；脚本运行默认项目内 `logs/`）
- `TALKSAGE_LOG_JSON=0`：文件也用文本格式（人工阅读）
- `--json` 且未设日志级别时，CLI 默认 `TALKSAGE_LOG=error`，避免 INFO 行混进 stdout
- 日志文件与 `sessions.db` 同数据目录树，随数据目录一起备份/迁移

## 6. 隐私

- 默认 `info` 级别**不记录转写全文**（仅段摘要 ≤60 字符、插件名与耗时）
- `debug`/`trace` 可能含更多上下文；生产环境建议保持 `info`
