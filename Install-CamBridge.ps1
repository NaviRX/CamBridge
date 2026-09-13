# CamBridge Windows 11 x64 installer. Run from an extracted release folder.
$ErrorActionPreference = 'Stop'
trap { $_ | Out-File (Join-Path $PSScriptRoot 'CamBridge-install-error.txt'); exit 1 }
$app = Join-Path $PSScriptRoot 'CamBridge.exe'
$source = Join-Path $PSScriptRoot 'VirtualCameraMediaSource.dll'
if (-not (Test-Path -LiteralPath $app) -or -not (Test-Path -LiteralPath $source)) {
    throw 'CamBridge.exe and VirtualCameraMediaSource.dll must be beside this installer.'
}
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    throw 'CamBridge virtual camera requires Windows 11 build 22000 or newer.'
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    $command = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`""
    $process = Start-Process powershell.exe -ArgumentList $command -Verb RunAs -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Installation failed with exit code $($process.ExitCode)." }
    return
}

$folder = Join-Path $env:ProgramFiles 'CamBridge'
$hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.Substring(0, 12)
$nativeTarget = Join-Path $folder "VirtualCameraMediaSource.$hash.dll"
if (Get-Process CamBridge -ErrorAction SilentlyContinue) {
    throw 'Quit CamBridge from its tray icon before installing an update.'
}
New-Item -ItemType Directory -Path $folder -Force | Out-Null
Copy-Item -LiteralPath $app -Destination (Join-Path $folder 'CamBridge.exe') -Force
if (-not (Test-Path -LiteralPath $nativeTarget)) {
    Copy-Item -LiteralPath $source -Destination $nativeTarget
}
$key = 'HKLM:\SOFTWARE\Classes\CLSID\{1483AAF4-E019-46C7-BFDC-322077BA141F}\InprocServer32'
New-Item -Path $key -Force | Out-Null
Set-Item -Path $key -Value $nativeTarget
New-ItemProperty -Path $key -Name 'ThreadingModel' -Value 'Both' -PropertyType String -Force | Out-Null
if (-not (Get-NetFirewallRule -DisplayName 'CamBridge UDP receiver' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName 'CamBridge UDP receiver' -Direction Inbound -Action Allow `
        -Protocol UDP -LocalPort 45831 -RemoteAddress LocalSubnet -Profile Private | Out-Null
}
if (-not (Get-NetFirewallRule -DisplayName 'CamBridge UDP discovery' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName 'CamBridge UDP discovery' -Direction Inbound -Action Allow `
        -Protocol UDP -LocalPort 45832 -RemoteAddress LocalSubnet -Profile Private | Out-Null
}
Write-Host "CamBridge installed to $folder"
Write-Host 'Run CamBridge.exe. On the game PC choose Receive and enter the sender PC IP.'
