<#
.SYNOPSIS
Registers the sh705x J2534 DLL as a pass-thru device, so J2534 applications (GDS2,
PCMflash...) list it next to real adapters. Run as administrator.

.EXAMPLE
# On the machine (or virtual machine) where the application runs:
.\register.ps1 -Dll32 C:\sh705x\sh705x_j2534_x86.dll -Server 192.168.56.1:777

.EXAMPLE
.\register.ps1 -Remove

.NOTES
32-bit applications (GDS2 is one) read HKLM\SOFTWARE\WOW6432Node\PassThruSupport.04.04
on 64-bit Windows; 64-bit ones read HKLM\SOFTWARE\PassThruSupport.04.04. Give the DLL
built for each: --target i686-pc-windows-msvc and x86_64-pc-windows-msvc.
The server address goes to %ProgramData%\sh705x\j2534.ini, where the DLL reads it.
#>
param(
    [string]$Dll32,
    [string]$Dll64,
    [string]$Server = '127.0.0.1:777',
    [string]$Log,
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'
$name = 'sh705x emulated SM3'
$views = @(
    @{ Key = "HKLM:\SOFTWARE\WOW6432Node\PassThruSupport.04.04\$name"; Dll = $Dll32 },
    @{ Key = "HKLM:\SOFTWARE\PassThruSupport.04.04\$name"; Dll = $Dll64 }
)
if (-not [Environment]::Is64BitOperatingSystem) {
    $views = @(@{ Key = "HKLM:\SOFTWARE\PassThruSupport.04.04\$name"; Dll = $Dll32 })
}

if ($Remove) {
    foreach ($v in $views) {
        if (Test-Path $v.Key) { Remove-Item $v.Key -Recurse; "removed $($v.Key)" }
    }
    return
}

foreach ($v in $views) {
    if (-not $v.Dll) { continue }
    $dll = (Resolve-Path $v.Dll).Path
    New-Item -Path $v.Key -Force | Out-Null
    $strings = @{
        Name               = $name
        Vendor             = 'sh705x'
        FunctionLibrary    = $dll
        ConfigApplication  = ''
        APIVersion         = '04.04'
        ProductVersion     = '0.4.2'
        ProtocolsSupported = 'CAN,ISO15765'
    }
    foreach ($k in $strings.Keys) {
        New-ItemProperty -Path $v.Key -Name $k -Value $strings[$k] -PropertyType String -Force | Out-Null
    }
    $protocols = @{ CAN = 1; ISO15765 = 1; J1850VPW = 0; J1850PWM = 0; ISO9141 = 0; ISO14230 = 0; SCI_A_ENGINE = 0; SCI_A_TRANS = 0; SCI_B_ENGINE = 0; SCI_B_TRANS = 0 }
    foreach ($k in $protocols.Keys) {
        New-ItemProperty -Path $v.Key -Name $k -Value $protocols[$k] -PropertyType DWord -Force | Out-Null
    }
    "registered $($v.Key) -> $dll"
}

$dir = Join-Path $env:ProgramData 'sh705x'
New-Item -ItemType Directory -Force $dir | Out-Null
$ini = @("server=$Server")
if ($Log) { $ini += "log=$Log" }
Set-Content -Path (Join-Path $dir 'j2534.ini') -Value $ini -Encoding ascii
"server $Server written to $dir\j2534.ini"
