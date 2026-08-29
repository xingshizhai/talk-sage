//! 应用升级：在线检查（框架）+ 离线安装升级包。
//!
//! ## 在线升级（框架，内容待实现）
//! 基于标准 Tauri v2 方案 `tauri-plugin-updater`：
//!   - 更新源端点与签名公钥配置在 `tauri.conf.json` 的 `plugins.updater`；
//!   - 运行 `talksage.ps1 package` / `talksage.sh package` 会自动生成签名密钥并把公钥写入该配置
//!     （公钥为空时本模块返回「在线升级尚未启用」而不是报错）；
//!   - [`check_for_updates`] 目前只做「检查并返回结果」，不自动下载/安装。
//!
//! TODO(在线升级内容): 接入真实更新服务器后，检查到新版本时可直接调用
//! `update.download_and_install()` 并在成功后 `app.restart()`；当前只把
//! `available/latest_version` 返回给前端展示。
//!
//! ## 离线升级
//! - **Windows**：选择 `package` 产出的 NSIS `.exe` 或 MSI，校验版本/架构后静默启动安装程序，应用退出让安装程序替换文件。
//! - **macOS**：选择 `.dmg` 或 `.app`。应用先退出，后台脚本把 bundle 复制到 `/Applications/<productName>.app` 再 `open`。
//!   macOS 没有等价于 NSIS `/S` 的静默安装器；用 `ditto` 替换 bundle 是官方离线分发（拖到 Applications）的可编程对应。

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Stdio;

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
            "message": "在线升级尚未启用（缺少更新公钥；运行 talksage.ps1 / talksage.sh package 会自动生成签名密钥并写入配置）",
        }));
    }
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("在线升级未配置: {e}");
            return Ok(serde_json::json!({
                "available": false,
                "configured": false,
                "current_version": current,
                "message": "在线升级尚未启用（缺少更新公钥；运行 talksage.ps1 / talksage.sh package 会自动生成签名密钥并写入配置）",
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
            log::warn!("检查更新失败: {e}");
            Ok(serde_json::json!({
                "available": false,
                "configured": true,
                "current_version": current,
                "message": format!("检查更新失败: {e}"),
            }))
        }
    }
}

/// 打开系统文件对话框，选择本平台升级包。取消时返回 null。
#[tauri::command]
pub fn pick_upgrade_package() -> Option<String> {
    let mut dlg = rfd::FileDialog::new();
    #[cfg(windows)]
    {
        dlg = dlg
            .add_filter("升级安装包", &["exe", "msi"])
            .set_title("选择升级安装包（talksage.ps1 package 产出的 NSIS / MSI）");
    }
    #[cfg(target_os = "macos")]
    {
        dlg = dlg
            .add_filter("macOS 安装包", &["dmg", "app"])
            .set_title("选择升级包（talksage.sh package 产出的 .dmg 或 TalkSage.app）");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        dlg = dlg.set_title("选择升级包");
    }
    dlg.add_filter("全部文件", &["*"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}

/// 离线升级：校验安装包后启动本平台安装流程，并退出当前应用。
#[tauri::command]
pub async fn install_offline_upgrade(path: String, app: AppHandle) -> Result<serde_json::Value, String> {
    let pkg = PathBuf::from(&path);
    if !pkg.exists() {
        return Err(format!("升级包不存在: {}", pkg.display()));
    }
    let kind = PackageKind::detect(&pkg)?;
    let fname = pkg
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if let Some(msg) = arch_incompatible(&fname) {
        return Err(msg.to_string());
    }
    let current = current_version();
    let version = version_of_package(&pkg, &fname, kind)?;
    if !version_newer(&version, &current) {
        return Err(format!("升级包版本（{version}）不高于当前版本（{current}），无需升级"));
    }
    #[cfg(target_os = "macos")]
    if matches!(kind, PackageKind::MacApp) {
        macos_bundle_arch_ok(&pkg)?;
    }
    log::info!("离线升级: 校验通过 path={path} version={version} kind={kind:?}");

    match kind {
        PackageKind::WindowsNsis | PackageKind::WindowsMsi => spawn_windows_installer(&path, kind)?,
        PackageKind::MacDmg | PackageKind::MacApp => spawn_macos_replacer(&pkg, kind, &app)?,
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        handle.exit(0);
    });
    Ok(serde_json::json!({
        "ok": true,
        "version": version,
        "message": match kind {
            PackageKind::MacDmg | PackageKind::MacApp => format!(
                "应用即将退出并安装 v{version} 到 /Applications；完成后会自动重新打开"
            ),
            PackageKind::WindowsNsis | PackageKind::WindowsMsi => format!(
                "升级程序已启动（v{version}），应用即将退出；安装完成后请重新启动应用"
            ),
        },
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageKind {
    #[allow(dead_code)] // 仅 Windows 构造；macOS 仍要在 match 里穷尽
    WindowsNsis,
    #[allow(dead_code)]
    WindowsMsi,
    #[allow(dead_code)] // 仅 macOS 构造；Windows 仍要在 match 里穷尽
    MacDmg,
    #[allow(dead_code)]
    MacApp,
}

impl PackageKind {
    fn detect(pkg: &Path) -> Result<Self, String> {
        let ext = pkg
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        #[cfg(windows)]
        {
            if ext == "exe" && pkg.is_file() {
                return Ok(Self::WindowsNsis);
            }
            if ext == "msi" && pkg.is_file() {
                return Ok(Self::WindowsMsi);
            }
            return Err("升级包必须是 NSIS 安装程序（.exe）或 MSI（.msi），请选择 talksage.ps1 package 产出的安装包".into());
        }
        #[cfg(target_os = "macos")]
        {
            if ext == "dmg" && pkg.is_file() {
                return Ok(Self::MacDmg);
            }
            if ext == "app" && pkg.is_dir() {
                return Ok(Self::MacApp);
            }
            return Err("升级包必须是 .dmg 或 .app，请选择 talksage.sh package 产出的安装包".into());
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (pkg, ext);
            Err("当前平台尚未支持离线升级".into())
        }
    }
}

fn version_of_package(pkg: &Path, fname: &str, kind: PackageKind) -> Result<String, String> {
    if let Some(v) = version_in_filename(fname) {
        return Ok(v);
    }
    #[cfg(target_os = "macos")]
    if kind == PackageKind::MacApp {
        return macos_app_version(pkg)
            .ok_or_else(|| format!("无法读取 {} 的版本（Contents/Info.plist）", pkg.display()));
    }
    let _ = (pkg, kind);
    Err(format!(
        "无法从文件名解析版本号: {fname}（预期如 TalkSage_0.1.4_aarch64.dmg 或 TalkSage_0.1.4_x64-setup.exe）"
    ))
}

fn spawn_windows_installer(path: &str, kind: PackageKind) -> Result<(), String> {
    #[cfg(windows)]
    {
        let spawned = match kind {
            PackageKind::WindowsMsi => std::process::Command::new("msiexec")
                .args(["/i", path, "/qb", "/norestart"])
                .spawn(),
            PackageKind::WindowsNsis => std::process::Command::new(path).arg("/S").spawn(),
            PackageKind::MacDmg | PackageKind::MacApp => {
                return Err("内部错误：Windows 安装器收到了 macOS 包".into());
            }
        };
        spawned.map_err(|e| format!("启动安装程序失败: {e}"))?;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let _ = (path, kind);
        Err("当前不是 Windows，无法启动 NSIS/MSI 安装程序".into())
    }
}

fn spawn_macos_replacer(pkg: &Path, kind: PackageKind, app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let product = app
            .config()
            .product_name
            .clone()
            .unwrap_or_else(|| "TalkSage".into());
        let dest = PathBuf::from(format!("/Applications/{product}.app"));
        let src_kind = match kind {
            PackageKind::MacDmg => "dmg",
            PackageKind::MacApp => "app",
            PackageKind::WindowsNsis | PackageKind::WindowsMsi => {
                return Err("内部错误：macOS 替换器收到了 Windows 包".into());
            }
        };
        let pid = std::process::id();
        // 固定写 /tmp，避免 macOS 用户级临时目录（/var/folders/...）难找。
        let log_path = PathBuf::from("/tmp/talksage-offline-upgrade.log");
        let log = std::fs::File::create(&log_path)
            .map_err(|e| format!("无法写入升级日志 {}: {e}", log_path.display()))?;
        let mut cmd = std::process::Command::new("/usr/bin/nohup");
        cmd.arg("/bin/bash")
            .arg("-c")
            .arg(MACOS_UPGRADE_SCRIPT)
            .env("TALKSAGE_UPGRADE_PID", pid.to_string())
            .env("TALKSAGE_UPGRADE_KIND", src_kind)
            .env("TALKSAGE_UPGRADE_SRC", pkg)
            .env("TALKSAGE_UPGRADE_DEST", &dest)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log));
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.spawn()
            .map_err(|e| format!("启动 macOS 升级脚本失败: {e}"))?;
        log::info!(
            "macOS 离线升级脚本已启动 dest={} log={}",
            dest.display(),
            log_path.display()
        );
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pkg, kind, app);
        Err("当前不是 macOS，无法安装 .dmg / .app".into())
    }
}

/// 等待当前进程退出后，把 dmg/.app 安装到 /Applications 并打开。
#[cfg(target_os = "macos")]
const MACOS_UPGRADE_SCRIPT: &str = r#"
set -euo pipefail
PID="${TALKSAGE_UPGRADE_PID:?}"
KIND="${TALKSAGE_UPGRADE_KIND:?}"
SRC="${TALKSAGE_UPGRADE_SRC:?}"
DEST="${TALKSAGE_UPGRADE_DEST:?}"
case "$DEST" in
  /Applications/*.app) ;;
  *) echo "refusing dest outside /Applications: $DEST" >&2; exit 1 ;;
esac
echo "talksage offline upgrade: wait pid=$PID kind=$KIND src=$SRC dest=$DEST"
for _ in $(seq 1 120); do
  if ! kill -0 "$PID" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
if kill -0 "$PID" 2>/dev/null; then
  echo "timeout waiting for pid $PID to exit" >&2
  exit 1
fi
sleep 0.4
MOUNT=""
cleanup() {
  if [ -n "${MOUNT:-}" ] && [ -d "$MOUNT" ]; then
    hdiutil detach "$MOUNT" -quiet || true
    rmdir "$MOUNT" 2>/dev/null || true
  fi
}
trap cleanup EXIT
APP_SRC=""
if [ "$KIND" = "dmg" ]; then
  MOUNT="$(mktemp -d /tmp/talksage-upgrade.XXXXXX)"
  hdiutil attach -nobrowse -readonly -noverify -mountpoint "$MOUNT" "$SRC"
  APP_SRC="$(find "$MOUNT" -maxdepth 2 -name '*.app' -type d -print -quit)"
  if [ -z "$APP_SRC" ]; then
    echo "dmg 内未找到 .app" >&2
    exit 1
  fi
else
  APP_SRC="$SRC"
fi
SRC_REAL="$(cd "$APP_SRC" && pwd -P)"
DEST_REAL=""
if [ -e "$DEST" ]; then
  DEST_REAL="$(cd "$DEST" && pwd -P)"
fi
if [ -n "$DEST_REAL" ] && [ "$SRC_REAL" = "$DEST_REAL" ]; then
  echo "source is already the install dest; just reopen" >&2
  open "$DEST"
  exit 0
fi
echo "copy $APP_SRC -> $DEST"
NEW="${DEST}.new"
OLD="${DEST}.old"
rm -rf "$NEW" "$OLD"
ditto "$APP_SRC" "$NEW"
if [ -e "$DEST" ]; then
  mv "$DEST" "$OLD"
fi
mv "$NEW" "$DEST"
rm -rf "$OLD"
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true
open "$DEST"
echo "opened $DEST"
"#;

#[cfg(target_os = "macos")]
fn macos_app_version(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let out = std::process::Command::new("plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

#[cfg(target_os = "macos")]
fn macos_bundle_arch_ok(app: &Path) -> Result<(), String> {
    let macos_dir = app.join("Contents/MacOS");
    let bin = std::fs::read_dir(&macos_dir)
        .map_err(|e| format!("不是有效的 .app（缺少 Contents/MacOS）: {e}"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
        .ok_or_else(|| "不是有效的 .app（Contents/MacOS 为空）".to_string())?;
    let out = std::process::Command::new("lipo")
        .args(["-archs"])
        .arg(&bin)
        .output()
        .map_err(|e| format!("无法读取应用架构: {e}"))?;
    let archs = String::from_utf8_lossy(&out.stdout);
    let has_arm = archs.contains("arm64");
    let has_x86 = archs.contains("x86_64");
    if cfg!(target_arch = "aarch64") && !has_arm {
        return Err(format!(
            "该 .app 不含 Apple Silicon (arm64) 切片（lipo: {}）",
            archs.trim()
        ));
    }
    if cfg!(target_arch = "x86_64") && !has_x86 {
        return Err(format!(
            "该 .app 不含 Intel (x86_64) 切片（lipo: {}）",
            archs.trim()
        ));
    }
    Ok(())
}

/// 文件名中的架构标记是否与当前进程不匹配。无架构信息时不拦截（再由平台校验兜底）。
fn arch_incompatible(filename: &str) -> Option<&'static str> {
    let lower = filename.to_ascii_lowercase();
    let pkg_arm = lower.contains("aarch64") || lower.contains("arm64");
    let pkg_x64 = lower.contains("x86_64")
        || lower.contains("amd64")
        || lower.contains("_x64")
        || lower.contains("-x64");
    #[cfg(target_arch = "aarch64")]
    {
        if pkg_x64 && !pkg_arm {
            return Some("该升级包是 Intel/x64 构建，当前应用为 ARM（Apple Silicon 或 Windows ARM），无法安装");
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if pkg_arm && !pkg_x64 {
            return Some("该升级包是 ARM 构建，当前应用为 x64，无法安装");
        }
    }
    let _ = (pkg_arm, pkg_x64, lower);
    None
}

/// 从安装包文件名解析版本号（形如 x.y.z 或 x.y.z.w）。
fn version_in_filename(name: &str) -> Option<String> {
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
    false
}

fn parse_version(s: &str) -> Vec<u64> {
    s.split('.').filter_map(|p| p.parse::<u64>().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_versions() {
        assert_eq!(version_in_filename("TalkSage_1.2.0_x64-setup.exe"), Some("1.2.0".into()));
        assert_eq!(version_in_filename("TalkSage_0.1.3_x64-setup.exe"), Some("0.1.3".into()));
        assert_eq!(version_in_filename("TalkSage_1.2.3.4_x64.msi"), Some("1.2.3.4".into()));
        assert_eq!(version_in_filename("TalkSage_0.1.4_aarch64.dmg"), Some("0.1.4".into()));
        assert_eq!(version_in_filename("TalkSage.app"), None);
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

    #[test]
    fn arch_mismatch_matches_current_cpu() {
        #[cfg(target_arch = "aarch64")]
        {
            assert!(arch_incompatible("TalkSage_0.1.4_x64-setup.exe").is_some());
            assert!(arch_incompatible("TalkSage_0.1.4_x86_64.dmg").is_some());
            assert!(arch_incompatible("TalkSage_0.1.4_aarch64.dmg").is_none());
            assert!(arch_incompatible("TalkSage.app").is_none());
        }
        #[cfg(target_arch = "x86_64")]
        {
            assert!(arch_incompatible("TalkSage_0.1.4_aarch64.dmg").is_some());
            assert!(arch_incompatible("TalkSage_0.1.4_arm64-setup.exe").is_some());
            assert!(arch_incompatible("TalkSage_0.1.4_x64-setup.exe").is_none());
        }
    }
}
