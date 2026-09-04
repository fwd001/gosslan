# =============================================================================
#  gosslan 编译环境一键安装脚本（Windows）
#  用法：在「管理员」PowerShell 执行  .\scripts\install-env.ps1
#  作用：安装 MSVC / Rust / JDK17，配置 cargo 镜像，添加 Rust Android target，
#        并输出 Android SDK 与环境变量的剩余手配步骤。
# =============================================================================
$ErrorActionPreference = "Stop"
$cargoHome = Join-Path $HOME ".cargo"

function Step([string]$t) {
    Write-Host ""
    Write-Host ("==> " + $t) -ForegroundColor Cyan
}

function HasCmd([string]$c) {
    return [bool](Get-Command $c -ErrorAction SilentlyContinue)
}

# ---- 1. MSVC Build Tools ----
Step "1/6 安装 MSVC C++ 生成工具（Windows 端链接必需）"
if (HasCmd "cl.exe") {
    Write-Host "  已检测到 cl.exe，跳过。" -ForegroundColor Green
} else {
    Write-Host "  正在通过 winget 安装（会弹 UAC，请确认）..." -ForegroundColor Yellow
    winget install Microsoft.VisualStudio.2022.BuildTools `
        --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    Write-Host "  MSVC 安装完成，稍后需重开终端。" -ForegroundColor Green
}

# ---- 2. Rust ----
Step "2/6 安装 Rust (rustup)"
if (HasCmd "rustc") {
    Write-Host "  已检测到 rustc，跳过。" -ForegroundColor Green
} else {
    winget install Rustlang.Rustup
    # 刷新当前会话 PATH
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")
    Write-Host "  Rust 安装完成，稍后需重开终端。" -ForegroundColor Green
}

# ---- 3. JDK 17 ----
Step "3/6 安装 JDK 17（Android 构建必需）"
if (HasCmd "java") {
    Write-Host "  已检测到 java，跳过。" -ForegroundColor Green
} else {
    winget install EclipseAdoptium.Temurin.17.JDK
    Write-Host "  JDK 安装完成。" -ForegroundColor Green
}

# ---- 4. cargo 镜像 ----
Step "4/6 配置 crates.io 镜像（中国大陆加速）"
$configPath = Join-Path $cargoHome "config.toml"
if (Test-Path $configPath) {
    Write-Host "  已存在 $configPath，跳过（如需换源请手动编辑）。" -ForegroundColor Yellow
} else {
    New-Item -ItemType Directory -Force -Path $cargoHome | Out-Null
    @'
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"

[net]
git-fetch-with-cli = true
'@ | Set-Content -Path $configPath -Encoding UTF8
    Write-Host ("  已写入 " + $configPath) -ForegroundColor Green
}

# ---- 5. Rust Android target ----
Step "5/6 添加 Rust Android 交叉编译 target"
if (HasCmd "rustup") {
    rustup target add aarch64-linux-android x86_64-linux-android armv7-linux-androideabi i686-linux-android
} else {
    Write-Host "  未检测到 rustup（请重开终端后手动执行）：" -ForegroundColor Yellow
    Write-Host "  rustup target add aarch64-linux-android x86_64-linux-android armv7-linux-androideabi i686-linux-android" -ForegroundColor Gray
}

# ---- 6. Android SDK + 环境变量（输出手配步骤）----
Step "6/6 Android SDK / NDK 与环境变量（需手动完成一次）"
Write-Host @"
  方案 A（推荐，有 GUI）：
    winget install Google.AndroidStudio
    → 打开 SDK Manager 安装：SDK Platform 34、Build-Tools、NDK、Platform-Tools

  方案 B（命令行）：
    见 docs/setup-windows.md 第 4 节，用 sdkmanager 安装 platform-tools / platforms;android-34 / build-tools;34.0.0 / ndk;26.3.11579264

  然后设置环境变量（把路径替换成实际值）：
    setx JAVA_HOME        "C:\Program Files\Eclipse Adoptium\jdk-17.x.x.x-hotspot"
    setx ANDROID_HOME     "%LOCALAPPDATA%\Android\Sdk"
    setx ANDROID_SDK_ROOT "%LOCALAPPDATA%\Android\Sdk"
    setx NDK_HOME         "%LOCALAPPDATA%\Android\Sdk\ndk\26.3.11579264"
"@ -ForegroundColor Gray

Write-Host ""
Write-Host "========== 安装流程结束 ==========" -ForegroundColor Cyan
Write-Host "1. 重开终端；2. 执行 .\scripts\check-env.ps1 自检；3. 全部 [OK] 后即可 npm run tauri dev" -ForegroundColor Yellow
Write-Host ""
