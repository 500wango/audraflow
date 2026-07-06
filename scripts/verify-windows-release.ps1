param(
  [string]$PortableDir,
  [string]$InstalledAppDir,
  [string]$InstallerDir,
  [string]$AudioPath,
  [string]$ModelPath,
  [int]$TimeoutSeconds = 90,
  [switch]$BuildInstallers,
  [switch]$SkipSmoke,
  [switch]$SkipInstallerCheck
)

$ErrorActionPreference = 'Stop'

function Resolve-WorkspaceRoot {
  $scriptDir = Split-Path -Parent $PSCommandPath
  return (Resolve-Path (Join-Path $scriptDir '..')).Path
}

function Write-Step {
  param([string]$Message)
  Write-Host ""
  Write-Host "== $Message"
}

function Assert-WindowsHost {
  if ($PSVersionTable.PSVersion.Major -ge 6 -and -not $IsWindows) {
    throw 'Windows release verification must run on Windows.'
  }
}

function Assert-Command {
  param([string]$Name)
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "Missing required command: $Name"
  }
}

function Assert-File {
  param(
    [string]$Path,
    [long]$MinBytes = 1
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "Missing file: $Path"
  }
  $item = Get-Item -LiteralPath $Path
  if ($item.Length -lt $MinBytes) {
    throw "File is too small: $Path ($($item.Length) bytes)"
  }
  return $item
}

function Invoke-Checked {
  param(
    [string]$FilePath,
    [string[]]$Arguments,
    [string]$WorkingDirectory,
    [int[]]$AllowedExitCodes = @(0)
  )

  $process = Start-Process `
    -FilePath $FilePath `
    -ArgumentList $Arguments `
    -WorkingDirectory $WorkingDirectory `
    -NoNewWindow `
    -Wait `
    -PassThru
  if ($AllowedExitCodes -notcontains $process.ExitCode) {
    throw "Command failed with exit code $($process.ExitCode): $FilePath $($Arguments -join ' ')"
  }
}

function Test-AudraFlowAppLayout {
  param(
    [string]$AppDir,
    [string]$Label
  )

  $resolved = (Resolve-Path $AppDir).Path
  Write-Step "Checking $Label layout"
  $required = @(
    'AudraFlow.exe',
    'audraflow-orchestrator.exe',
    'audraflow-asr-runtime.exe',
    'bin\whisper-cli.exe',
    'bin\ffmpeg.exe',
    'bin\ffprobe.exe',
    'bin\yt-dlp.exe'
  )

  foreach ($relativePath in $required) {
    $item = Assert-File -Path (Join-Path $resolved $relativePath) -MinBytes 1024
    Write-Host ("OK {0} ({1:N0} bytes)" -f $relativePath, $item.Length)
  }

  $requiredDlls = @(
    'bin\vcruntime140.dll',
    'bin\vcruntime140_1.dll',
    'bin\msvcp140.dll',
    'bin\whisper.dll',
    'bin\ggml.dll',
    'bin\ggml-base.dll',
    'bin\ggml-cpu.dll'
  )
  foreach ($relativePath in $requiredDlls) {
    $item = Assert-File -Path (Join-Path $resolved $relativePath) -MinBytes 1024
    Write-Host ("OK {0} ({1:N0} bytes)" -f $relativePath, $item.Length)
  }

  $optionalDlls = @(
    'bin\concrt140.dll',
    'bin\libgcc_s_seh-1.dll',
    'bin\libgomp-1.dll',
    'bin\libstdc++-6.dll',
    'bin\libwinpthread-1.dll',
    'bin\msvcp140_1.dll',
    'bin\msvcp140_2.dll',
    'bin\msvcp140_atomic_wait.dll',
    'bin\msvcp140_codecvt_ids.dll',
    'bin\vcomp140.dll',
    'bin\vcruntime140_threads.dll'
  )
  foreach ($relativePath in $optionalDlls) {
    $path = Join-Path $resolved $relativePath
    if (Test-Path -LiteralPath $path -PathType Leaf) {
      $item = Assert-File -Path $path -MinBytes 1024
      Write-Host ("OK {0} ({1:N0} bytes)" -f $relativePath, $item.Length)
    }
  }

  Write-Step "Checking bundled command-line tools"
  Invoke-Checked -FilePath (Join-Path $resolved 'bin\ffmpeg.exe') -Arguments @('-version') -WorkingDirectory $resolved
  Invoke-Checked -FilePath (Join-Path $resolved 'bin\ffprobe.exe') -Arguments @('-version') -WorkingDirectory $resolved
  Invoke-Checked -FilePath (Join-Path $resolved 'bin\whisper-cli.exe') -Arguments @('--help') -WorkingDirectory $resolved -AllowedExitCodes @(0, 1)
  Invoke-Checked -FilePath (Join-Path $resolved 'bin\yt-dlp.exe') -Arguments @('--version') -WorkingDirectory $resolved

  return $resolved
}

function Test-NsisHookConfig {
  param([string]$Workspace)

  Write-Step 'Checking NSIS runtime installer hook'
  $configPath = Join-Path $Workspace 'src-tauri\tauri.conf.json'
  $config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
  $hookRelativePath = $config.bundle.windows.nsis.installerHooks
  if (-not $hookRelativePath) {
    throw 'Missing bundle.windows.nsis.installerHooks in src-tauri\tauri.conf.json'
  }

  $hookPath = Join-Path (Join-Path $Workspace 'src-tauri') $hookRelativePath
  Assert-File -Path $hookPath -MinBytes 128 | Out-Null
  $hookText = Get-Content -LiteralPath $hookPath -Raw
  if ($hookText -notmatch 'vc_redist\.x64\.exe') {
    throw "NSIS installer hook does not download the x64 VC++ Redistributable: $hookPath"
  }
  if ($hookText -notmatch 'SOFTWARE\\Microsoft\\VisualStudio\\14\.0\\VC\\Runtimes\\x64') {
    throw "NSIS installer hook does not check the x64 VC++ Runtime registry key: $hookPath"
  }
  if ($hookText -match "(?m)^\s*ExecWait\s+'") {
    throw "NSIS ExecWait commands must use backtick-quoted command lines: $hookPath"
  }
  Write-Host "OK $hookRelativePath"
}

Assert-WindowsHost
$workspace = Resolve-WorkspaceRoot
Set-Location $workspace
Test-NsisHookConfig -Workspace $workspace

if (-not $PortableDir) {
  $PortableDir = Join-Path $workspace 'release\windows-portable\AudraFlow'
}
if (-not $InstallerDir) {
  $InstallerDir = Join-Path $workspace 'target\release\bundle'
}
if (-not $AudioPath) {
  $AudioPath = Join-Path $workspace 'external\whisper.cpp\samples\jfk.mp3'
}
if (-not $ModelPath) {
  $candidate = Join-Path $workspace 'external\whisper.cpp\models\ggml-tiny.bin'
  if (Test-Path -LiteralPath $candidate -PathType Leaf) {
    $ModelPath = $candidate
  }
}

Write-Step 'Checking host tools'
Assert-Command npm
Assert-Command cargo

if ($BuildInstallers) {
  Write-Step 'Building Windows installers'
  Invoke-Checked -FilePath 'npm' -Arguments @('run', 'desktop:build:windows') -WorkingDirectory $workspace
}

$portableRoot = Test-AudraFlowAppLayout -AppDir $PortableDir -Label 'portable package'

if ($InstalledAppDir) {
  Test-AudraFlowAppLayout -AppDir $InstalledAppDir -Label 'installed application' | Out-Null
}

if (-not $SkipInstallerCheck) {
  Write-Step 'Checking installer artifacts'
  $installerRoot = (Resolve-Path $InstallerDir).Path
  $installers = @()
  $installers += Get-ChildItem -LiteralPath $installerRoot -Recurse -File -Filter '*.msi' -ErrorAction SilentlyContinue
  $installers += Get-ChildItem -LiteralPath $installerRoot -Recurse -File -Filter '*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\nsis\\|setup|installer' }

  if ($installers.Count -eq 0) {
    throw "No Windows installer artifacts found under $installerRoot"
  }
  foreach ($installer in $installers | Sort-Object FullName) {
    Assert-File -Path $installer.FullName -MinBytes 1048576 | Out-Null
    Write-Host ("OK {0} ({1:N0} bytes)" -f $installer.FullName, $installer.Length)
  }
}

if (-not $SkipSmoke) {
  Write-Step 'Running packaged orchestrator smoke test'
  Assert-File -Path $AudioPath -MinBytes 1024 | Out-Null
  Assert-File -Path $ModelPath -MinBytes 1024 | Out-Null
  $orchestratorPath = Join-Path $portableRoot 'audraflow-orchestrator.exe'
  Invoke-Checked `
    -FilePath 'powershell' `
    -Arguments @(
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      (Join-Path $workspace 'scripts\smoke-e2e.ps1'),
      '-SkipBuild',
      '-OrchestratorPath',
      $orchestratorPath,
      '-OrchestratorWorkingDirectory',
      $portableRoot,
      '-AudioPath',
      $AudioPath,
      '-ModelPath',
      $ModelPath,
      '-TimeoutSeconds',
      $TimeoutSeconds.ToString()
    ) `
    -WorkingDirectory $workspace
}

Write-Step 'Windows release verification passed'
