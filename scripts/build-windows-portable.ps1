# =============================================================================
#  构建 Windows 便携版（绿色版）：生成单个 gosslan.exe 并打包成 zip
#  用途：周一多台 Windows 电脑「即点即用」，无需安装依赖（Win10/11 已内置 WebView2）
#  用法：项目根目录执行  .\scripts\build-windows-portable.ps1
# =============================================================================
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "==> 1/2 构建 Windows 便携版（无安装包，仅原始 exe）" -ForegroundColor Cyan
npm run tauri -- build --no-bundle
if ($LASTEXITCODE -ne 0) { Write-Error "构建失败"; exit $LASTEXITCODE }

$exe = Join-Path $root "src-tauri\target\release\gosslan.exe"
if (-not (Test-Path $exe)) {
    Write-Error "未找到 $exe"
    exit 1
}

Write-Host "==> 2/2 打包 zip" -ForegroundColor Cyan
$version = (Get-Content (Join-Path $root "package.json") -Raw | ConvertFrom-Json).version
$outDir = Join-Path $root "dist-portable"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$zip = Join-Path $outDir "gosslan_${version}_x64-portable.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $exe -DestinationPath $zip -Force

Write-Host ("==> 产物：" + $zip) -ForegroundColor Green
Write-Host ("==> 解压后双击 gosslan.exe 即可运行；多开见 scripts\run-multi-instance.ps1") -ForegroundColor Green
