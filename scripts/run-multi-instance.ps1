# =============================================================================
#  多开测试：在同一台 Windows 电脑上同时运行 N 个 gosslan 实例
#  每个实例用 --instance N 使用独立数据库 / TCP 端口 / 设备指纹，
#  UDP 端口共享（SO_REUSEADDR），从而在单机模拟多个局域网节点。
#
#  用法：项目根目录执行
#    .\scripts\run-multi-instance.ps1              # 默认 3 个实例
#    .\scripts\run-multi-instance.ps1 -Count 2     # 2 个实例
#    .\scripts\run-multi-instance.ps1 -Exe "path\to\gosslan.exe"
# =============================================================================
param(
    [int]$Count = 3,
    [string]$Exe = ""
)

$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) {
    $Exe = Join-Path $root "src-tauri\target\release\gosslan.exe"
}
if (-not (Test-Path $Exe)) {
    Write-Error "未找到可执行文件：$Exe（请先执行 .\scripts\build-windows-portable.ps1）"
    exit 1
}

Write-Host ("==> 启动 {0} 个 gosslan 实例（1 台电脑模拟 {0} 个节点）" -f $Count) -ForegroundColor Cyan
for ($i = 1; $i -le $Count; $i++) {
    $proc = Start-Process -FilePath $Exe -ArgumentList @("--instance", $i) -PassThru
    Write-Host ("  实例 {0} 已启动（PID {1}，device_id 后缀 -i{0}，TCP 端口 {2}）" -f $i, $proc.Id, (59992 + $i * 10)) -ForegroundColor Green
    Start-Sleep -Milliseconds 800
}

Write-Host ""
Write-Host "提示：关掉对应进程即可停止；要查看节点互发现，在每个实例里打开「添加好友」触发 who_has 探测。" -ForegroundColor Yellow
Write-Host "Windows 防火墙首次会弹窗，选择「允许访问」并勾选专用网络。" -ForegroundColor Yellow
