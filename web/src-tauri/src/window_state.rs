//! 窗口状态持久化：位置 + 尺寸，保存到 `<data_dir>/window.json`。
//!
//! 单位约定：**物理像素**（tauri `outer_position` / `outer_size` 的返回值）。
//! 恢复时必须以物理单位应用（`PhysicalPosition` / `PhysicalSize`），
//! 否则在 >100% DPI 缩放下会被再次放大（逻辑→物理转换）导致窗口巨大。
//!
//! 保存/恢复前都经 `clamp_to_work_area` 钳制，防止异常值/DPI 变化导致窗口超出屏幕。

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
        self.width >= 320 && self.height >= 240 && self.width <= 20000 && self.height <= 20000
    }
}

/// 把窗口状态钳制到显示器工作区内（`monitor_size`/`monitor_pos` 为物理像素）：
/// - 尺寸不超过显示器 95%，且不小于最小尺寸；
/// - 左上角落在显示器范围内（负坐标/超出右下的窗口被拉回）。
pub fn clamp_to_work_area(ws: &mut WindowState, monitor_size: (u32, u32), monitor_pos: (i32, i32)) {
    let (mw, mh) = monitor_size;
    let (mx, my) = monitor_pos;
    if mw == 0 || mh == 0 {
        return;
    }
    // 尺寸
    let max_w = (mw as f64 * 0.95) as u32;
    let max_h = (mh as f64 * 0.95) as u32;
    ws.width = ws.width.min(max_w).max(320).min(mw);
    ws.height = ws.height.min(max_h).max(240).min(mh);
    // 位置：确保窗口主体可见
    ws.x = ws.x.max(mx);
    ws.y = ws.y.max(my);
    if ws.x + ws.width as i32 > mx + mw as i32 {
        ws.x = mx + mw as i32 - ws.width as i32;
    }
    if ws.y + ws.height as i32 > my + mh as i32 {
        ws.y = my + mh as i32 - ws.height as i32;
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
