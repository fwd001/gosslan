# =============================================================================
#  生成 Android 工程并应用 gosslan 自定义权限清单
#  用法：项目根目录执行  .\scripts\setup-android.ps1
# =============================================================================
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$gen = Join-Path $root "src-tauri\gen\android"
$manifest = Join-Path $gen "app\src\main\AndroidManifest.xml"
$template = Join-Path $PSScriptRoot "android\AndroidManifest.xml"

Write-Host "==> 1/2 生成 Android 工程（若已存在则跳过）" -ForegroundColor Cyan
if (-not (Test-Path $gen)) {
    npm run tauri android init
    if ($LASTEXITCODE -ne 0) { Write-Error "tauri android init 失败"; exit 1 }
} else {
    Write-Host "  已存在 $gen，跳过 init。" -ForegroundColor Yellow
}

Write-Host "==> 2/2 应用权限清单（局域网 + 蓝牙 + 前台服务）" -ForegroundColor Cyan
if (-not (Test-Path $manifest)) {
    Write-Error "未找到生成的 AndroidManifest.xml：$manifest"; exit 1
}
Copy-Item -Path $template -Destination $manifest -Force
Write-Host ("  已写入 " + $manifest) -ForegroundColor Green
Write-Host ""
Write-Host "完成。下一步：npm run tauri android build -- --apk" -ForegroundColor Yellow
