# Runs EmuBoot.py on a flash image in a throwaway headless Ghidra project.
#   .\emu_boot.ps1 -Flash <flash.bin> -Out <report.txt> [-Max 15000000] [-Force FFFFED18:80,...]
# Ghidra is taken from -Ghidra or from GHIDRA_INSTALL_DIR.
param(
  [Parameter(Mandatory)][string]$Flash,
  [Parameter(Mandatory)][string]$Out,
  [int]$Max = 15000000,
  [string[]]$Force = @(),
  [string]$Ghidra = $env:GHIDRA_INSTALL_DIR
)
if (-not $Ghidra -or -not (Test-Path "$Ghidra\support\analyzeHeadless.bat")) {
  throw 'Ghidra not found: pass -Ghidra or set GHIDRA_INSTALL_DIR'
}
$proj = Join-Path ([IO.Path]::GetTempPath()) 'emu_boot_gproj'
if (Test-Path $proj) { Remove-Item -Recurse -Force $proj -Confirm:$false }
New-Item -ItemType Directory $proj | Out-Null
$log = "$Out.log"
& "$Ghidra\support\analyzeHeadless.bat" $proj emu -import (Resolve-Path $Flash) `
  -processor SuperH:BE:32:SH-2A -loader BinaryLoader -loader-baseAddr 0x0 -noanalysis `
  -scriptPath $PSScriptRoot -postScript EmuBoot.py ([IO.Path]::GetFullPath($Out)) $Max @Force `
  -deleteProject *> $log
Remove-Item -Recurse -Force $proj -Confirm:$false -ErrorAction SilentlyContinue
Select-String -Path $log -Pattern 'Traceback|EmuBoot:' -Context 0, 6 | Select-Object -First 1
