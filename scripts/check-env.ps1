# =============================================================================
#  gosslan (Tauri v2 + Vue 3 + Rust) 编译环境一键检查脚本
#  用法：在项目根目录 PowerShell 执行  .\scripts\check-env.ps1
#  检查：rustc / cargo / tauri CLI / JDK / Android SDK / NDK / adb / Rust Android target
# =============================================================================
$ErrorActionPreference = "SilentlyContinue"

Write-Host ""
Write-Host "========== gosslan 编译环境检查 ==========" -ForegroundColor Cyan
Write-Host ""

# 工具函数：运行命令并打印结果
function Check([string]$label, [string]$cmd, [string[]]$args) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $cmd -ArgumentList $args -NoNewWindow -PassThru -Wait -RedirectStandardOutput "$env:TEMP\gosslan_check_out.txt" -RedirectStandardError "$env:TEMP\gosslan_check_err.txt"
    $sw.Stop()
    $out = (Get-Content "$env:TEMP\gosslan_check_out.txt" -Raw -ErrorAction SilentlyContinue)
    $err = (Get-Content "$env:TEMP\gosslan_check_err.txt" -Raw -ErrorAction SilentlyContinue)
    $ver = ($out + $err).Trim().Split("`n")[0]
    if ($p.ExitCode -eq 0 -and $ver) {
        Write-Host ("  [OK]   {0,-28} {1}" -f $label, $ver) -ForegroundColor Green
    } else {
        Write-Host ("  [MISS] {0,-28} 未安装或不在 PATH 中" -f $label) -ForegroundColor Red
    }
}

function CheckDir([string]$label, [string]$path) {
    if (Test-Path $path) {
        Write-Host ("  [OK]   {0,-28} {1}" -f $label, $path) -ForegroundColor Green
        return $true
    } else {
        Write-Host ("  [MISS] {0,-28} 未找到：{1}" -f $label, $path) -ForegroundColor Red
        return $false
    }
}

# ---- 1. Rust 工具链 ----
Write-Host "【Rust 工具链】" -ForegroundColor Yellow
Check "rustc" "rustc" @("--version")
Check "cargo" "cargo" @("--version")
Check "rustup" "rustup" @("--version")

# ---- 2. Tauri CLI（npm 或 cargo 安装其一即可）----
Write-Host ""
Write-Host "【Tauri CLI】" -ForegroundColor Yellow
$tauriOk = $false
$p = Start-Process -FilePath "npx" -ArgumentList @("--no-install","tauri","--version") -NoNewWindow -PassThru -Wait -RedirectStandardOutput "$env:TEMP\gosslan_check_out.txt" -RedirectStandardError "$env:TEMP\gosslan_check_err.txt"
$out = (Get-Content "$env:TEMP\gosslan_check_out.txt" -Raw -ErrorAction SilentlyContinue)
if ($p.ExitCode -eq 0 -and $out.Trim()) {
    Write-Host ("  [OK]   {0,-28} {1}" -f "tauri (npm)" , $out.Trim().Split("`n")[0]) -ForegroundColor Green
    $tauriOk = $true
} else {
    Check "tauri (cargo)" "cargo" @("tauri","--version")
}

# ---- 3. Java / JDK ----
Write-Host ""
Write-Host "【Java / JDK 17】" -ForegroundColor Yellow
Check "java" "java" @("-version")
$javaHome = $env:JAVA_HOME
if ($javaHome) {
    Write-Host ("  [OK]   {0,-28} {1}" -f "JAVA_HOME", $javaHome) -ForegroundColor Green
} else {
    Write-Host "  [MISS] JAVA_HOME            未设置" -ForegroundColor Red
}

# ---- 4. Android SDK / NDK / adb ----
Write-Host ""
Write-Host "【Android SDK / NDK】" -ForegroundColor Yellow
$sdkRoot = $env:ANDROID_HOME
if (-not $sdkRoot) { $sdkRoot = $env:ANDROID_SDK_ROOT }
if ($sdkRoot) {
    Write-Host ("  [OK]   {0,-28} {1}" -f "ANDROID_HOME", $sdkRoot) -ForegroundColor Green
    # adb
    $adb = Join-Path $sdkRoot "platform-tools\adb.exe"
    if (Test-Path $adb) {
        $adbVer = (& $adb --version 2>&1 | Select-Object -First 1)
        Write-Host ("  [OK]   {0,-28} {1}" -f "adb", $adbVer) -ForegroundColor Green
    } else {
        Write-Host "  [MISS] adb                   未找到 platform-tools" -ForegroundColor Red
    }
    # SDK 平台
    $platforms = Get-ChildItem (Join-Path $sdkRoot "platforms") -Directory -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
    if ($platforms) {
        Write-Host ("  [OK]   {0,-28} {1}" -f "SDK platforms", ($platforms -join ", ")) -ForegroundColor Green
    } else {
        Write-Host "  [MISS] SDK platforms         未安装任何 android 平台" -ForegroundColor Red
    }
    # NDK
    $ndk = $env:NDK_HOME
    if (-not $ndk) {
        $ndkDirs = Get-ChildItem (Join-Path $sdkRoot "ndk") -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending
        if ($ndkDirs) { $ndk = $ndkDirs[0].FullName }
    }
    if ($ndk -and (Test-Path (Join-Path $ndk "source.properties"))) {
        $ver = Select-String -Path (Join-Path $ndk "source.properties") -Pattern "Pkg.Revision" | ForEach-Object { $_.Line }
        Write-Host ("  [OK]   {0,-28} {1}  ({2})" -f "NDK", $ndk, $ver) -ForegroundColor Green
    } else {
        Write-Host "  [MISS] NDK                   未找到（需安装 NDK）" -ForegroundColor Red
    }
} else {
    Write-Host "  [MISS] ANDROID_HOME          未设置（Android SDK 未配置）" -ForegroundColor Red
}

# ---- 5. Rust Android 交叉编译 target ----
Write-Host ""
Write-Host "【Rust Android target】" -ForegroundColor Yellow
$targets = (rustup target list --installed 2>&1) -join " "
foreach ($t in @("aarch64-linux-android", "x86_64-linux-android", "armv7-linux-androideabi", "i686-linux-android")) {
    if ($targets -match $t) {
        Write-Host ("  [OK]   {0,-28} 已安装" -f $t) -ForegroundColor Green
    } else {
        Write-Host ("  [MISS] {0,-28} 未安装" -f $t) -ForegroundColor Red
    }
}

# ---- 汇总 ----
Write-Host ""
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "提示：" -ForegroundColor Yellow
Write-Host "  若某项 [MISS]，参见 docs/setup-windows.md 补齐对应环境。" -ForegroundColor Gray
Write-Host "  一键安装（需联网，可能触发 UAC）： .\scripts\install-env.ps1" -ForegroundColor Gray
Write-Host ""
