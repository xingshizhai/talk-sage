# TalkSage CLI

`talksage` 服务三类场景：**无界面转写**、**脚本自动化**、**本机运维**。它不是桌面端的终端克隆。

构建后二进制在 `target/debug/talksage`（或 `target/release/talksage`）。开发期也可用 `cargo run -p talksage-cli -- <命令>`。

## 全局参数

| 参数 | 作用 |
|---|---|
| `--json` | 成功 JSON 写 stdout；失败 JSON 写 stderr 并 exit 1。未另设日志级别时控制台日志降为 `error`，避免污染 stdout |
| `--verbose` | 等价 `TALKSAGE_LOG=trace` |
| `--log-level LEVEL` | `trace` / `debug` / `info` / `warn` / `error` |

控制台日志一律写 **stderr**。脚本里请用 `--json` 解析 stdout。

## 会话

```bash
talksage session list [--limit 20]
talksage session show <id> [--dup-only]
talksage session search <关键词> [--limit 50]
talksage session rename <id> <新标题>
talksage session delete <id> --yes
talksage session export [id] [--format md|txt|audio] [-o 路径]
talksage session notes <id> [--template standard_meeting]
talksage session trio <id> [--name 会议名] [--desc 说明]
talksage session replay <id> [--engine qwen3-asr]
```

- `talksage session <id>` 仍等同 `session show <id>`；`talksage sessions` 等同 `session list`。
- `session export` 未指定 `-o` 时写入 `<data_dir>/sessions/<id>/exports/`。顶层 `talksage export` 是 Markdown 别名，默认写当前目录。
- 删除会话必须加 `--yes`。
- **`replay`**：用该会话 master 录音（没有则退回分轨 / 会话录音目录）再转写，**另存为新会话**。未指定 `--engine` 时用原会话快照里的 `user_engine`，再缺省 `qwen3-asr`。

## 转写

```bash
talksage transcribe audio.wav [--engine qwen3-asr] [--save] [--speaker 导入]
talksage import audio.wav [--engine qwen3-asr]     # transcribe --save 的别名
talksage listen --input mic|loopback|<wav>
```

`transcribe` 默认只打印结果；加 `--save` 才落库。`listen` 是实时/回放验证，不是会话 `replay`。

## 模型

```bash
talksage models list
talksage models download qwen3-asr
talksage models remove punct --yes
talksage models gpu
```

与桌面「模型管理」共用 `talksage-asr::models`。安装状态、目录解析与校验见 [model-management.md](model-management.md)。删除必须 `--yes`。

## 配置

```bash
talksage config path
talksage config get [点路径]          # 省略路径则打印全部
talksage config set <点路径> <值>
```

点路径示例：`asr.engine_zh`、`llm.default`、`llm.providers.deepseek.api_key`、`audio.input_gain_db`。

- 读写走 `talksage-config` 的 `apply_updates`（与设置页同一套合并规则）。
- 值能解析成 JSON（`true` / `12` / `{"a":1}` / `[...]`）则按 JSON，否则当字符串。
- **密钥一律打码**（含 `--json`）。把打码值原样 `set` 回去视为未修改。
- 未知路径会拒绝，不会误写 `talksage.toml`。
- `config path` 打印配置文件、数据目录、日志目录（受 `TALKSAGE_CONFIG_DIR` / `TALKSAGE_DATA_DIR` / `TALKSAGE_LOG_DIR` 影响）。

## 日志

```bash
talksage logs [--lines 200]
```

对齐桌面调试窗口：在日志目录找最新 `talksage*.log`，打印尾部 N 行。`--json` 额外给出文件路径。构建脚本 `./scripts/talksage.sh logs` / `.\scripts\talksage.ps1 logs` 仍可用，固定 `tail` 最近 50 行。

级别与文件格式见 [LOGGING.md](LOGGING.md)。

## 其他运维命令

| 命令 | 作用 |
|---|---|
| `talksage doctor` | 环境 / 目录 / 平台诊断 |
| `talksage record --seconds 60 [--input mic\|loopback]` | 只录音、不转写 |
| `talksage trim rec.wav [-o out.wav] [--preset standard]` | VAD 去静音 |
| `talksage bench [--dir 语料] [--engine …]` | 固定语料 CER/WER / RTF |
| `talksage diarize audio.wav [--speakers N]` | 离线说话人时间轴 |
| `talksage serve [--host 127.0.0.1] [--port 8080]` | headless HTTP/WS |
| `talksage version` | 版本号 |

录音目录与会话主录音见 [RECORDING.md](RECORDING.md)。
