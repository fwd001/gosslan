# =============================================================================
#  构建 Android APK（Debug 用于本机+手机直连调试，Release 用于压测分发）
#  用法：项目根目录执行
#    .\scripts\build-android.ps1            # Debug APK
#    .\scripts\build-android.ps1 -Release   # Release APK
# =============================================================================
param([switch]$Release)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 前置自检：Android 环境
if (-not $env:ANDROID_HOME) {
    Write-Error "ANDROID_HOME 未设置，请先完成 docs/setup-windows.md 第 4、5 步"
    exit 1
}
if (-not $env:JAVA_HOME) {
    Write-Error "JAVA_HOME 未设置（需要 JDK 17）"
    exit 1
}

if ($Release) {
    Write-Host "==> 构建 Android Release APK" -ForegroundColor Cyan
    npm run tauri -- android build --apk
} else {
    Write-Host "==> 构建 Android Debug APK" -ForegroundColor Cyan
    npm run tauri -- android build --debug --apk
}
$code = $LASTEXITCODE
if ($code -ne 0) { Write-Error "构建失败"; exit $code }

$variant = if ($Release) { "release" } else { "debug" }
$apk = Join-Path $root "src-tauri\gen\android\app\build\outputs\apk\universal\$variant\app-universal-$variant.apk"
if (Test-Path $apk) {
    Write-Host ("==> 产物：" + $apk) -ForegroundColor Green
    Write-Host ('==> 安装到手机：adb install -r "' + $apk + '"') -ForegroundColor Green
} else {
    Write-Host "未在预期路径找到 APK，请在 src-tauri\gen\android\app\build\outputs\apk\ 下查找。" -ForegroundColor Yellow
}
