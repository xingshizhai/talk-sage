//! 应用升级：在线检查（框架）+ 离线安装升级包。
//!
//! ## 在线升级（框架，内容待实现）
//! 基于标准 Tauri v2 方案 `tauri-plugin-updater`：
//!   - 更新源端点与签名公钥配置在 `tauri.conf.json` 的 `plugins.updater`；
//!   - 运行 `talksage.ps1 package` 会自动生成签名密钥并把公钥写入该配置
//!     （公钥为空时本模块返回「在线升级尚未启用」而不是报错）；
//!   - [`check_for_updates`] 目前只做「检查并返回结果」，不自动下载/安装。
//!
//! TODO(在线升级内容): 接入真实更新服务器后，检查到新版本时可直接调用
//! `update.download_and_install()` 并在成功后 `app.restart()`；当前只把
//! `available/latest_version` 返回给前端展示。
//!
//! ## 离线升级
//! 用户在设置页选择安装包（`talksage.ps1 package` 产出的 NSIS `.exe` 或 MSI），
//! 校验版本高于当前、架构匹配后静默启动安装程序，应用随后退出让安装程序替换文件。

use std::path::PathBuf;

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// 当前应用版本（与 workspace 版本一致）。
pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 在线检查更新（框架）：有可更新版本时返回信息，不自动安装。
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<serde_json::Value, String> {
    let current = current_version();
    // 公钥为空 = 未配置在线升级（占位端点），直接提示，避免无谓请求
    let pubkey = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if pubkey.is_empty() {
        log::warn!("在线升级未配置: plugins.updater.pubkey 为空");
        return Ok(serde_json::json!({
            "available": false,
            "configured": false,
            "current_version": current,
            "message": "在线升级尚未启用（缺少更新公钥；运行 talksage.ps1 package 会自动生成签名密钥并写入配置）",
        }));
    }
    // 未配置公钥/端点时返回可读提示，而不是让前端看到原始错误
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("在线升级未配置: {e}");
            return Ok(serde_json::json!({
                "available": false,
                "configured": false,
                "current_version": current,
                "message": "在线升级尚未启用（缺少更新公钥；运行 talksage.ps1 package 会自动生成签名密钥并写入配置）",
            }));
        }
    };
    match updater.check().await {
        Ok(Some(update)) => Ok(serde_json::json!({
            "available": true,
            "configured": true,
            "current_version": current,
            "latest_version": update.version.to_string(),
            "message": format!("发现新版本 {}", update.version),
        })),
        Ok(None) => Ok(serde_json::json!({
            "available": false,
            "configured": true,
            "current_version": current,
            "message": "已是最新版本",
        })),
        Err(e) => {
            log::warn!("在线检查更新失败: {e}");
            Ok(serde_json::json!({
                "available": false,
                "configured": true,
                "current_version": current,
                "message": format!("检查更新失败: {e}"),
            }))
        }
    }
}

/// 打开系统文件对话框，选择升级安装包（NSIS .exe / MSI）。取消时返回 null。
#[tauri::command]
pub fn pick_upgrade_package() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("升级安装包", &["exe", "msi"])
        .add_filter("全部文件", &["*"])
        .set_title("选择升级安装包（talksage.ps1 package 产出的安装程序）")
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}

/// 离线升级：校验安装包（存在 / 扩展名 / 版本高于当前 / 架构匹配），
/// 静默启动安装程序，随后退出应用让安装程序替换文件。
#[tauri::command]
pub async fn install_offline_upgrade(path: String, app: AppHandle) -> Result<serde_json::Value, String> {
    let pkg = PathBuf::from(&path);
    if !pkg.is_file() {
        return Err(format!("升级包不存在: {}", pkg.display()));
    }
    let ext = pkg
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "exe" && ext != "msi" {
        return Err("升级包必须是 NSIS 安装程序（.exe）或 MSI（.msi），请选择 talksage.ps1 package 产出的安装包".into());
    }
    // 架构检查：文件名带 arm64 时与当前 x64 构建不匹配
    let fname = pkg
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if fname.to_ascii_lowercase().contains("arm64") {
        return Err("该升级包是 arm64 架构，当前应用为 x64 构建，无法安装".into());
    }
    let current = current_version();
    let Some(version) = version_in_filename(&fname) else {
        return Err(format!(
            "无法从文件名解析版本号: {fname}（预期如 拓思者_1.2.0_x64-setup.exe）"
        ));
    };
    if !version_newer(&version, &current) {
        return Err(format!("升级包版本（{version}）不高于当前版本（{current}），无需升级"));
    }
    log::info!("离线升级: 校验通过 path={path} version={version} ext={ext}");
    // 启动安装程序（NSIS /S 静默；MSI 最小界面 + 不重启），随后退出应用。
    let spawned = if ext == "msi" {
        std::process::Command::new("msiexec")
            .args(["/i", &path, "/qb", "/norestart"])
            .spawn()
    } else {
        std::process::Command::new(&path).arg("/S").spawn()
    };
    spawned.map_err(|e| format!("启动安装程序失败: {e}"))?;
    // 延迟退出：先让 invoke 响应送达前端，再退出应用（安装程序替换文件需要应用先关闭）
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        app.exit(0);
    });
    Ok(serde_json::json!({
        "ok": true,
        "version": version,
        "message": format!("升级程序已启动（v{version}），应用即将退出；安装完成后请重新启动应用"),
    }))
}

/// 从安装包文件名解析版本号（形如 x.y.z 或 x.y.z.w，如 `拓思者_1.2.0_x64-setup.exe`）。
fn version_in_filename(name: &str) -> Option<String> {
    // 按「非数字且非点」切分，找由数字段组成的版本子串
    for part in name.split(|c: char| !c.is_ascii_digit() && c != '.') {
        if part.is_empty() {
            continue;
        }
        let segs: Vec<&str> = part.split('.').collect();
        if (3..=4).contains(&segs.len()) && segs.iter().all(|s| !s.is_empty()) {
            return Some(part.to_string());
        }
    }
    None
}

/// 版本号 a 是否严格高于 b（各段按数值比较，缺省段按 0）。
fn version_newer(a: &str, b: &str) -> bool {
    let av = parse_version(a);
    let bv = parse_version(b);
    let n = av.len().max(bv.len());
    for i in 0..n {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false // 相等
}

fn parse_version(s: &str) -> Vec<u64> {
    s.split('.').filter_map(|p| p.parse::<u64>().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_versions() {
        assert_eq!(version_in_filename("拓思者_1.2.0_x64-setup.exe"), Some("1.2.0".into()));
        assert_eq!(version_in_filename("拓思者_0.1.3_x64-setup.exe"), Some("0.1.3".into()));
        assert_eq!(version_in_filename("拓思者_1.2.3.4_x64.msi"), Some("1.2.3.4".into()));
        assert_eq!(version_in_filename("setup.exe"), None);
    }

    #[test]
    fn compare_versions() {
        assert!(version_newer("1.2.0", "1.1.9"));
        assert!(!version_newer("1.2.0", "1.2.0"));
        assert!(!version_newer("1.1.9", "1.2.0"));
        assert!(version_newer("1.10.0", "1.9.0"));
        assert!(version_newer("2.0.0.1", "2.0.0"));
    }
}
