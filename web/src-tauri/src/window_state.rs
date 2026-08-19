//! 窗口状态持久化：位置 + 尺寸，保存到 `<data_dir>/window.json`。
//! 桌面应用的典型偏好：下次启动恢复上次的窗口位置与大小。

use std::path::Path;

/// 窗口状态（物理像素，由 tauri outer_position/outer_size 提供）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowState {
    /// 简单校验：尺寸需在合理范围内，避免恢复异常值。
    pub fn is_valid(&self) -> bool {
        self.width >= 320 && self.height >= 240 && self.width <= 10000 && self.height <= 10000
    }
}

/// 读取保存的窗口状态；缺失或损坏时返回 None。
pub fn load(path: &Path) -> Option<WindowState> {
    let raw = std::fs::read_to_string(path).ok()?;
    // 容错：外部工具（如 PowerShell）可能写入带 BOM 的 UTF-8。
    let raw = raw.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let ws = WindowState {
        x: v.get("x")?.as_i64()? as i32,
        y: v.get("y")?.as_i64()? as i32,
        width: v.get("width")?.as_u64()? as u32,
        height: v.get("height")?.as_u64()? as u32,
    };
    ws.is_valid().then_some(ws)
}

/// 保存窗口状态（覆盖写，JSON 一行）。
pub fn save(path: &Path, ws: &WindowState) -> std::io::Result<()> {
    let json = serde_json::json!({
        "x": ws.x,
        "y": ws.y,
        "width": ws.width,
        "height": ws.height,
    })
    .to_string();
    std::fs::write(path, json)
}
