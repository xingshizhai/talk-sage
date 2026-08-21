# 会议录音与音频处理（测试闭环）

本项目"边开发、边使用、边完善"：监听转写的同时会把原始音频保存下来，
再用静音裁剪工具加工成紧凑的测试素材，回放验证 ASR/插件/事件链路，
形成闭环优化路径。

```
真实会议监听 ──► recordings/*.wav（原始双流录音）
                    │
                    ▼ talksage trim（silero VAD 去静音）
              *.trimmed.wav（紧凑测试素材）
                    │
                    ▼ talksage listen --input（真实模型回放验证）
              转写结果 / 事件 → 回归断言
```

## 1. 监听时自动录音（默认开启）

- 配置：`[recording] enabled = true`（设置页「会议录音」可开关/改目录）
- 保存位置：`<data_dir>/recordings/`（`TALKSAGE_DATA_DIR` 或 `~/.talksage`）
- 命名：`2026-08-19_15-30-22_我.wav`、`2026-08-19_15-30-22_客户.wav`
  （用户流/客户流各一条，16kHz mono PCM16，记录的是**原始**音频——预处理前，
  便于之后对比降噪/高通效果）
- 停止监听（或文件输入结束）时自动收尾写头；文件数/占用可用 `talksage doctor` 查看

## 2. 静音裁剪（去掉没有声音的部分）

```powershell
# 单文件（输出 <输入>.trimmed.wav）
.\target\release\talksage.exe trim .\recordings\2026-08-19_15-30-22_我.wav

# 指定输出 / 灵敏度（sensitive 保留弱语音，strict 抗噪）
talksage trim in.wav -o out.wav --preset sensitive

# 脚本包装
.\scripts\talksage.ps1 trim -wav <录音.wav> [-out <输出.wav>] [-preset standard]
```

- 与实时转写共用同一套 silero VAD 参数（threshold/min_speech/min_silence/窗口）
- 每段语音前后保留 300ms 静音（不切音头音尾）
- 输出统计：输入/输出时长、去掉的静音、段数、压缩率

## 3. 纯录音（不转写）

```powershell
talksage record --seconds 60                    # 麦克风录 60s → <data_dir>/recordings/
talksage record --seconds 30 --input loopback   # 系统回环（会议软件里客户语音）
.\scripts\talksage.ps1 record -seconds 30 -input loopback
```

## 4. 一键闭环：录制 → 裁剪 → 回放验证

```powershell
.\scripts\recording_loop.ps1                # 处理 <data_dir>/recordings 全部录音
.\scripts\recording_loop.ps1 -Latest 5      # 只处理最近 5 个
.\scripts\recording_loop.ps1 -NoAsr         # 只裁剪，不回放
.\scripts\talksage.ps1 loop                 # 等价包装
```

每个文件输出：裁剪统计 + 回放转写文本，最后汇总表（原始/裁剪大小、压缩率）。

## 5. 用录音做回归测试

1. 把有价值的录音（如真实会议片段）放入 `models/sherpa-onnx-streaming-paraformer-zh/`
   或单独测试素材目录，参考 `crates/talksage-pipeline/tests/pipeline_live.rs`
   的 `recording_saves_wav_files_for_each_stream` / `file_input_produces_status_and_segments`
   用例模式接入自动测试；
2. `scripts\run_tests.ps1` 全量跑（Rust 单元/集成 + Vitest）；
3. 新素材只需替换 wav 文件即可回归验证 ASR 与事件链路。
## 6. 录音电平

麦克风采集会在录音和 ASR 前执行两步处理：

1. 多声道设备自动选择当前回调中 RMS 最高的通道，避免无线麦只有单侧有声时被平均衰减。
2. 应用 `audio.input_gain_db` 输入增益（默认 `12dB`，范围 `0..24dB`），并把峰值限制在 `±0.98`。

可在“设置 → ASR 转写 → 麦克风输入电平”调整。环境噪声被明显放大时降低到 `6dB`；无线麦输出仍很小时可提高到 `18dB`。该设置只影响之后的新录音，不会改写历史 WAV。
