//! 用系统默认应用打开本地文件（跨平台）。三端各有一条真实踩过的坑，所以这里按平台分派。
//!
//! - **macOS**：走 `NSWorkspace.openURL` 而非 `tauri-plugin-opener`。后者在 macOS 底层
//!   调 `open` crate → `Command::new("/usr/bin/open")`，而 **App Sandbox 禁止沙盒应用 fork
//!   外部可执行文件**，`/usr/bin/open` 会被拦 → 「打开文件失败」。`NSWorkspace.openURL`
//!   是纯 Foundation API，不 fork 子进程，沙盒应用允许调用。
//! - **Android**：走 FileProvider + `ACTION_VIEW`（见 [`crate::android_open`]）。opener 在
//!   Android 上只有 `Intent(ACTION_VIEW, url.toUri())`，而应用私有目录的文件以 `file://`
//!   暴露会被系统直接拒绝（Android 7+ 的 `FileUriExposedException`）→「文件打开失败」。
//! - **Windows / Linux**：无沙盒，继续用 opener（`ShellExecuteW` / `xdg-open`）。

/// 打开一个本地文件。
///
/// 三个平台都**先做一次存在性检查**：文件还没同步完 / 已被清理时，用户该看到
/// 「文件不存在」，而不是一个笼统的「打开失败」。
pub fn open_path_native(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    open_existing(path)
}

#[cfg(target_os = "macos")]
fn open_existing(path: &std::path::Path) -> Result<(), String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let path_str = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path_str);
    let workspace = NSWorkspace::sharedWorkspace();
    if workspace.openURL(&url) {
        Ok(())
    } else {
        Err("系统没有可打开此文件的默认应用".to_string())
    }
}

#[cfg(target_os = "android")]
fn open_existing(path: &std::path::Path) -> Result<(), String> {
    // MIME 交给 Kotlin 侧按扩展名推断（`MimeTypeMap` 是系统表，比我们在 Rust 里维护一份准）。
    crate::android_open::open_path(&path.to_string_lossy(), "")
}

#[cfg(all(not(target_os = "macos"), not(target_os = "android")))]
fn open_existing(path: &std::path::Path) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}
