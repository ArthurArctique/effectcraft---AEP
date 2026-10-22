<#
.SYNOPSIS
  Build and install "EffectCraft AEP": this checkout as a local app that opens After Effects
  projects with no setup, through its own Python venv with py-aep.

.DESCRIPTION
  Assembles, in -Destination (default: %LOCALAPPDATA%\Programs\EffectCraft AEP):
    effectcraft.exe, effectcraft-cli.exe   this checkout's build
    aep_ir_export.py                       the converter, found beside the executables
                                           (crates/engine/src/aep_export.rs)
    aep-python\                            a venv with py-aep (packaging/aep/requirements.txt)
    portable.txt                           its own settings in EffectCraftData beside it, apart
                                           from an installed EffectCraft (kept on reinstall)
  and a Start menu shortcut "EffectCraft AEP" (not with -NoShortcut).

  The venv runs on the Python it is made from (3.11 or newer), which must stay installed.
  Installing py-aep needs network access and git once.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\windows\aep-preview.ps1
  powershell -ExecutionPolicy Bypass -File packaging\windows\aep-preview.ps1 -BuildProfile dev -Destination D:\Tools\EffectCraft-AEP
#>
param(
  [string] $Destination = (Join-Path $env:LOCALAPPDATA 'Programs\EffectCraft AEP'),
  # Interpreter the venv is made from (default: $env:PYTHON, else the py launcher, else python).
  [string] $Python = $env:PYTHON,
  [ValidateSet('release', 'dev')] [string] $BuildProfile = 'release',
  [switch] $SkipBuild,
  [switch] $NoShortcut
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Dest = [IO.Path]::GetFullPath($Destination)
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Bin = Join-Path $TargetDir $(if ($BuildProfile -eq 'release') { 'release' } else { 'debug' })

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

if (-not $Python) { $Python = if (Get-Command py -ErrorAction SilentlyContinue) { 'py' } else { 'python' } }
& $Python -c 'import sys; sys.exit(sys.version_info < (3, 11))'
if ($LASTEXITCODE -ne 0) { throw "$Python is not Python 3.11 or newer (choose one with -Python)" }

$running = Get-Process effectcraft, effectcraft-cli -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -and (Split-Path $_.Path -Parent) -eq $Dest }
if ($running) { throw "EffectCraft AEP is running from $Dest; quit it first" }

if (-not $SkipBuild) {
  $cargoArgs = @('build', '--locked', '-p', 'effectcraft', '-p', 'effectcraft-cli')
  if ($BuildProfile -eq 'release') { $cargoArgs += '--release' }
  Invoke-Native "cargo build ($BuildProfile)" { cargo @cargoArgs }
}
foreach ($exe in 'effectcraft.exe', 'effectcraft-cli.exe') {
  if (-not (Test-Path (Join-Path $Bin $exe))) { throw "$exe is not built in $Bin" }
}

# Assembled beside the destination, then swapped in: a failure leaves the installed app as it was.
$Parent = Split-Path $Dest -Parent
New-Item -ItemType Directory -Force -Path $Parent | Out-Null
$Stage = Join-Path $Parent ('.effectcraft-aep.' + [guid]::NewGuid().ToString('N').Substring(0, 8))
try {
  Write-Output "==> assembling EffectCraft AEP in $Stage"
  New-Item -ItemType Directory -Path $Stage | Out-Null
  Copy-Item (Join-Path $Bin 'effectcraft.exe'), (Join-Path $Bin 'effectcraft-cli.exe') $Stage
  Copy-Item (Join-Path $Root 'packaging\aep\aep_ir_export.py') $Stage
  Copy-Item (Join-Path $Root 'packaging\aep\requirements.txt') (Join-Path $Stage 'aep-requirements.txt')
  Copy-Item (Join-Path $Root 'packaging\windows\portable.txt') $Stage

  $Venv = Join-Path $Stage 'aep-python'
  Invoke-Native 'Python venv' { & $Python -m venv $Venv }
  $VenvPython = Join-Path $Venv 'Scripts\python.exe'
  Invoke-Native 'py-aep' {
    & $VenvPython -m pip install --quiet --disable-pip-version-check -r (Join-Path $Root 'packaging\aep\requirements.txt')
  }
  # Only the interpreter is used; activate scripts and console entry points name the staging path.
  Get-ChildItem (Join-Path $Venv 'Scripts') -File | Where-Object { $_.Name -notlike 'python*' } | Remove-Item -Force

  # The previous install steps aside first (renaming fails, before anything is lost, while a file
  # in it is in use); its settings move into the new one, then it goes.
  $Old = $null
  if (Test-Path $Dest) {
    $Old = "$Stage.old"
    Move-Item $Dest $Old
  }
  try { Move-Item $Stage $Dest } catch { if ($Old) { Move-Item $Old $Dest }; throw }
  if ($Old) {
    $data = Join-Path $Old 'EffectCraftData'
    if (Test-Path $data) { Move-Item $data (Join-Path $Dest 'EffectCraftData') }
    try { Remove-Item -Recurse -Force $Old } catch { Write-Warning "could not remove the previous install at ${Old}: $_" }
  }
} finally {
  if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
}

if (-not $NoShortcut) {
  $Programs = [Environment]::GetFolderPath('Programs')
  $shell = New-Object -ComObject WScript.Shell
  $link = $shell.CreateShortcut((Join-Path $Programs 'EffectCraft AEP.lnk'))
  $link.TargetPath = Join-Path $Dest 'effectcraft.exe'
  $link.WorkingDirectory = $Dest
  $link.Description = 'EffectCraft with After Effects project import'
  $link.Save()
}
Write-Output "==> installed $Dest\effectcraft.exe (open .aep files with File > Open or by dropping them)"
