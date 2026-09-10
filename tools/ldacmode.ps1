<#
.SYNOPSIS
Switches the Bluetooth radio between the Microsoft stack and ldacsrc.

.DESCRIPTION
One radio cannot serve both stacks at once, so ownership is a deliberate switch:
'bt' gives the dongle back to Windows, 'ldac' hands it to WinUSB for ldacsrc.
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Position = 0)]
    [ValidateSet('status', 'bt', 'ldac')]
    [string]$Mode = 'status',

    [switch]$Run,

    [string]$Addr = '14:3F:A6:35:D0:AA',

    [ValidateSet('hq', 'sq', 'mq')]
    [string]$Quality = 'sq',

    [string]$WinUsbInf = "$env:USERPROFILE\usb_driver\Generic_Bluetooth_Radio.inf",

    # ldacsrc passes its own PID when --auto-bt-fallback launches this on stream
    # loss: it is still exiting, so the "already running" check would refuse.
    [int]$WaitForPid = 0
)

$ErrorActionPreference = 'Stop'
Import-Module "$env:WINDIR\System32\WindowsPowerShell\v1.0\Modules\PnpDevice\PnpDevice.psd1"

$HardwareId = 'USB\VID_0A12&PID_0001'
$BtInf = (Get-ChildItem "$env:WINDIR\System32\DriverStore\FileRepository\bth.inf_*\bth.inf" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
$RepoRoot = Split-Path $PSScriptRoot -Parent
$Binary = Join-Path $RepoRoot 'target\release\ldacsrc.exe'

function Start-Ldac {
    if (-not (Test-Path -LiteralPath $Binary)) {
        Write-Error "Build missing: $Binary. Run cargo build --release from $RepoRoot."
        exit 127
    }
    Push-Location $RepoRoot
    try { & $Binary --addr $Addr --quality $Quality }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "ldacsrc failed with exit code $LASTEXITCODE" }
}

function Get-Radio {
    $radios = @(Get-PnpDevice -PresentOnly |
        Where-Object { $_.InstanceId -like "$HardwareId*" })
    if ($radios.Count -gt 1) { throw 'Multiple matching radios present; disconnect the spare before switching.' }
    $radios | Select-Object -First 1
}

function Get-RadioState {
    $dev = Get-Radio
    if (-not $dev) {
        return [pscustomobject]@{ Present = $false; Mode = 'absent' }
    }
    $props = $dev | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_Service, DEVPKEY_Device_DriverInfPath, DEVPKEY_Device_Children -ErrorAction SilentlyContinue
    $service = ($props | Where-Object KeyName -eq 'DEVPKEY_Device_Service').Data
    $inf = ($props | Where-Object KeyName -eq 'DEVPKEY_Device_DriverInfPath').Data
    $children = @(($props | Where-Object KeyName -eq 'DEVPKEY_Device_Children').Data)

    $mode = switch ($service) {
        'BTHUSB' { 'bt' }
        'WinUSB' { 'ldac' }
        default { "unknown ($service)" }
    }
    if ($service -eq 'WinUSB' -and @($children | Where-Object { $_ }).Count -gt 0) {
        $mode = 'pending-ldac (unplug/replug USB dongle)'
    }
    [pscustomobject]@{
        Present     = $true
        Mode        = $mode
        InstanceId  = $dev.InstanceId
        Service     = $service
        Inf         = $inf
        Status      = $dev.Status
        Children    = $children | Where-Object { $_ }
        BtDeviceCount = (Get-PnpDevice -Class Bluetooth -PresentOnly -ErrorAction SilentlyContinue | Measure-Object).Count
        AppRunning  = [bool](Get-Process -Name ldacsrc -ErrorAction SilentlyContinue)
    }
}

function Show-Status {
    $s = Get-RadioState
    if (-not $s.Present) {
        Write-Host "radio: not present" -ForegroundColor Red
        return
    }
    $colour = if ($s.Mode -eq 'bt') { 'Cyan' } else { 'Yellow' }
    Write-Host "mode:      $($s.Mode)" -ForegroundColor $colour
    Write-Host "device:    $($s.InstanceId)"
    Write-Host "driver:    $($s.Service) via $($s.Inf)"
    Write-Host "children:  $(if ($s.Children) { $s.Children -join ', ' } else { '(none)' })"
    Write-Host "bluetooth: $($s.BtDeviceCount) device(s) visible to Windows"
    Write-Host "ldacsrc:   $(if ($s.AppRunning) { 'running' } else { 'not running' })"
}

function Assert-Elevated {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Switch-Radio {
    param([ValidateSet('bt', 'ldac')][string]$Target)

    $inf = if ($Target -eq 'bt') { $BtInf } else { $WinUsbInf }
    $wantService = if ($Target -eq 'bt') { 'BTHUSB' } else { 'WinUSB' }

    if (-not (Test-Path -LiteralPath $inf)) { throw "Driver package missing: $inf" }
    if ($Target -eq 'ldac') {
        $text = Get-Content -Raw -LiteralPath $inf
        if ($text -notmatch 'VID_0A12&PID_0001' -or $text -notmatch 'ServiceBinary\s*=.*WinUSB.sys') {
            throw "Not the expected Bluetooth WinUSB package: $inf"
        }
    }
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LdacDriver {
    [DllImport("newdev.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool UpdateDriverForPlugAndPlayDevicesW(
        IntPtr hwnd, string hardwareId, string inf, uint flags,
        [MarshalAs(UnmanagedType.Bool)] out bool rebootRequired);
}
'@
    $rebootRequired = $false
    if (-not [LdacDriver]::UpdateDriverForPlugAndPlayDevicesW(
        [IntPtr]::Zero, $HardwareId, $inf, 1, [ref]$rebootRequired)) {
        throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
    }
    if ($rebootRequired) { throw 'Driver switch requires a USB unplug/replug before streaming. No reboot was started.' }

    # the device node is rebuilt asynchronously; poll rather than guess a delay
    $deadline = (Get-Date).AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 500
        $state = Get-RadioState
    } while (($state.Service -ne $wantService -or $state.Status -ne 'OK' -or
        ($Target -eq 'ldac' -and $state.Children)) -and (Get-Date) -lt $deadline)

    if ($state.Service -ne $wantService -or $state.Status -ne 'OK' -or
        ($Target -eq 'ldac' -and $state.Children)) {
        throw "switch to '$Target' incomplete: service=$($state.Service), status=$($state.Status), children=$($state.Children)"
    }
    $state
}

if ($Mode -eq 'status') {
    Show-Status
    return
}

if ($WaitForPid -gt 0) {
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Process -Id $WaitForPid -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 200
    }
    if (Get-Process -Id $WaitForPid -ErrorAction SilentlyContinue) {
        throw "process $WaitForPid still running after 15s; not switching while it may own the radio"
    }
}

$current = Get-RadioState
if (-not $current.Present) { throw "radio $HardwareId is not plugged in" }

if ($current.AppRunning) {
    throw 'ldacsrc is running and owns the radio. Close it first.'
}

if ($Mode -eq 'ldac' -and $current.Mode -like 'pending-ldac*') {
    throw 'WinUSB is staged but Windows still owns the radio. Unplug/replug the USB dongle, then retry.'
}

if ($current.Mode -eq $Mode) {
    Write-Host "already in '$Mode' mode" -ForegroundColor DarkGray
    Show-Status
    if ($Mode -eq 'ldac' -and $Run -and -not $WhatIfPreference) { Start-Ldac }
    return
}

if (-not $PSCmdlet.ShouldProcess($current.InstanceId, "Switch Bluetooth adapter to $Mode")) { return }
if ($Run -and -not (Test-Path -LiteralPath $Binary)) { throw "Build missing: $Binary" }

if (-not (Assert-Elevated)) {
    $switchLog = Join-Path $RepoRoot 'driver-switch.log'
    $cmd = "& '{0}' '{1}' -WinUsbInf '{2}' *> '{3}'" -f
        $PSCommandPath.Replace("'", "''"), $Mode, $WinUsbInf.Replace("'", "''"), $switchLog.Replace("'", "''")
    [scriptblock]::Create($cmd) | Out-Null
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($cmd))
    $argv = @('-NoProfile', '-EncodedCommand', $encoded)
    $child = Start-Process (Join-Path $PSHOME 'pwsh.exe') -Verb RunAs -WindowStyle Hidden -ArgumentList $argv -Wait -PassThru
    if ($child.ExitCode -ne 0) {
        if (Test-Path -LiteralPath $switchLog) { Get-Content -LiteralPath $switchLog | Write-Host }
        throw "Elevated switch failed with exit code $($child.ExitCode); see $switchLog"
    }
    Show-Status
    if ($Mode -eq 'ldac' -and $Run) { Start-Ldac }
    return
}

$new = Switch-Radio -Target $Mode
Show-Status

if ($Mode -eq 'bt') {
    Write-Host ""
    Write-Host "Link keys are per stack: a headset last paired through ldacsrc may need" -ForegroundColor DarkGray
    Write-Host "re-pairing in Windows, and vice versa." -ForegroundColor DarkGray
}
if ($Mode -eq 'ldac' -and $Run) {
    Start-Ldac
}
