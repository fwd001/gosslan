# gosslan 本机编译环境配置指南（Windows 11 / 10）

本指南把一台全新 Windows 机器配到「能跑 Windows 调试 + 能打出 Android APK」的完整步骤。
gosslan 技术栈：Tauri v2 · Rust · Vue 3 · TypeScript · Node。

> 快速自检：配置完成后在项目根目录执行 `.\scripts\check-env.ps1`，逐项确认是否就绪。

---

## 0. 总览：需要装哪些东西

| 组件 | 用途 | 安装方式 |
| --- | --- | --- |
| **Node.js ≥ 18** | 前端构建（Vite） | 你已装（fnm / node 22） |
| **Rust (rustup)** | 后端编译 | `winget install Rustlang.Rustup` |
| **MSVC Build Tools** | Windows 端链接（必须） | `winget install Microsoft.VisualStudio.2022.BuildTools` |
| **JDK 17** | Android 构建（Gradle） | `winget install EclipseAdoptium.Temurin.17.JDK` |
| **Android SDK + NDK** | Android 打包 | Android Studio 或命令行 tools + sdkmanager |
| **Rust Android target** | 交叉编译到安卓 | `rustup target add ...` |

> 若只是先跑 **Windows 端**，只需 Node + Rust + MSVC 三项；要出 **Android APK** 再加 JDK + SDK/NDK + target。

---

## 1. 安装 MSVC 编译工具链（Windows 端必需）

在 **管理员** PowerShell 执行：

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools `
  --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

- 会弹 UAC 确认，正常安装「C++ 生成工具」+ Windows SDK。
- 装完**重开终端**让 PATH 生效。
- 验证：`where cl` 能找到 `cl.exe`（通常在 `C:\Program Files (x86)\Microsoft Visual Studio\...`）。

> 若 winget 没有：手动去 https://visualstudio.microsoft.com/visual-cpp-build-tools/ 下载「Visual Studio Build Tools」，勾选「使用 C++ 的桌面开发」工作负载。

---

## 2. 安装 Rust（rustup）

```powershell
winget install Rustlang.Rustup
```

装完重开终端，验证：

```powershell
rustc --version
cargo --version
```

### 2.1 （中国大陆推荐）配置 crates.io 镜像

新建 `C:\Users\<你>\.cargo\config.toml`（注意是 `config.toml`，不是 `config`）：

```toml
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"

[net]
git-fetch-with-cli = true
```

> 备选镜像：清华 `sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/`。

（可选）加速 rustup 自身下载，临时设置环境变量：

```powershell
$env:RUSTUP_DIST_SERVER = "https://mirrors.ustc.edu.cn/rust-static"
$env:RUSTUP_UPDATE_ROOT = "https://mirrors.ustc.edu.cn/rust-static/rustup"
```

---

## 3. 安装 JDK 17（Android 端必需）

```powershell
winget install EclipseAdoptium.Temurin.17.JDK
```

安装路径一般为 `C:\Program Files\Eclipse Adoptium\jdk-17.x.x.x-hotspot`，记下实际路径，后面设 `JAVA_HOME`。

验证：`java -version`（应是 17）。

---

## 4. 安装 Android SDK + NDK（Android 端必需）

### 方案 A：Android Studio（省事，有图形界面）

```powershell
winget install Google.AndroidStudio
```

首次打开 → SDK Manager → 勾选安装：
- **SDK Platform**：Android 14（API 34）
- **SDK Tools**：`Android SDK Build-Tools`、`NDK (Side by side)`、`Android SDK Platform-Tools`

### 方案 B：命令行 tools + sdkmanager（轻量，无 GUI）

```powershell
# 1) 创建目录并下载 commandline-tools
$sdk = "$env:LOCALAPPDATA\Android\Sdk"
New-Item -ItemType Directory -Force -Path "$sdk\cmdline-tools" | Out-Null

# 2) 从 https://developer.android.com/studio#command-line-tools-only 下载
#    commandlinetools-win-*_latest.zip，解压到 $sdk\cmdline-tools\latest
#    （确保 sdkmanager.bat 位于 $sdk\cmdline-tools\latest\bin\）

# 3) 安装平台 + 构建工具 + NDK
$sdkmanager = "$sdk\cmdline-tools\latest\bin\sdkmanager.bat"
& $sdkmanager --sdk_root="$sdk" "platform-tools" "platforms;android-34" "build-tools;34.0.0" "ndk;26.3.11579264"

# 4) 接受许可
& $sdkmanager --sdk_root="$sdk" --licenses
```

---

## 5. 配置环境变量（Android 端必需）

把下面 4 个环境变量设成**系统/用户变量**（PowerShell，注意路径替换成你机器上的实际值）：

```powershell
# 注意：setx 不展开 %VAR%，这里用 $env: 先展开成绝对路径再 setx
$jdk   = "C:\Program Files\Eclipse Adoptium\jdk-17.0.14.7-hotspot"   # 改成你的实际路径
$sdk   = "$env:LOCALAPPDATA\Android\Sdk"

setx JAVA_HOME          "$jdk"
setx ANDROID_HOME       "$sdk"
setx ANDROID_SDK_ROOT   "$sdk"
setx NDK_HOME           "$sdk\ndk\26.3.11579264"

# 追加 PATH（platform-tools 给 adb 用，cmdline-tools 给 sdkmanager 用）
$old = [Environment]::GetEnvironmentVariable("Path", "User")
[Environment]::SetEnvironmentVariable("Path", "$old;$sdk\platform-tools;$sdk\cmdline-tools\latest\bin", "User")
```

> 设完**重开终端**。验证：`adb --version`、`sdkmanager --version`、`echo $env:NDK_HOME`。

---

## 6. 安装 Rust 的 Android 交叉编译 target

```powershell
rustup target add aarch64-linux-android x86_64-linux-android armv7-linux-androideabi i686-linux-android
```

验证：`rustup target list --installed` 应包含上述 4 个。

---

## 7. 项目依赖安装

```powershell
cd D:\code\lanct
npm install
```

---

## 8. 生成 Android 工程（首次打包前执行一次）

```powershell
npm run tauri android init
```

会生成 `src-tauri/gen/android/`。然后应用本项目自定义的权限清单：

```powershell
.\scripts\setup-android.ps1
```

> 该脚本会把 `scripts/android/AndroidManifest.xml`（含局域网 + 蓝牙 + 前台服务权限）合入生成的清单。

---

## 9. 最终启动 / 打包命令

### 9.1 Windows 端开发调试

```powershell
npm run tauri dev
```

### 9.2 打 Windows 便携版（周一多机即点即用）

```powershell
npm run dist:win:portable
```

产物：`src-tauri/target/release/bundle/` 下，或脚本会打包成 zip。

### 9.3 打出第一个 Android APK（Debug，用于本机 + 手机直连调试）

```powershell
npm run tauri android build -- --apk
```

Debug APK 路径：`src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`

### 9.4 Release APK（周一正式测试）

```powershell
npm run tauri android build -- --release --apk
```

### 9.5 手机安装（adb）

```powershell
adb install -r src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release.apk
```

---

## 10. 常见问题

| 现象 | 解决 |
| --- | --- |
| `error: linker 'link.exe' not found` | MSVC 未装或未重开终端（第 1 步） |
| `JAVA_HOME is not set` | 第 5 步设 `JAVA_HOME` 后重开终端 |
| `NDK not found` / `ANDROID_HOME` 报错 | 第 4、5 步装 NDK 并设 `NDK_HOME` |
| `could not find target android` | 第 6 步 `rustup target add` |
| crates 下载很慢 | 第 2.1 步配镜像 |
| adb 找不到设备 | 手机开「开发者选项 + USB 调试」，`adb devices` 确认 |
