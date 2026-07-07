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

function Find-AppFile {
  param(
    [string]$AppDir,
    [string]$FileName
  )

  $roots = @(
    $AppDir,
    (Join-Path $AppDir 'resources'),
    (Join-Path $AppDir 'resources\bin')
  )
  foreach ($root in $roots) {
    $candidate = Join-Path $root $FileName
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
      return $candidate
    }
  }
  return $null
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
    'audraflow-asr-runtime.exe'
  )

  foreach ($relativePath in $required) {
    $path = Find-AppFile -AppDir $resolved -FileName $relativePath
    if (-not $path) {
      throw "Missing $relativePath under $resolved, $resolved\resources, or $resolved\resources\bin"
    }
    $item = Assert-File -Path $path -MinBytes 1024
    Write-Host ("OK {0} ({1:N0} bytes)" -f $path, $item.Length)
  }

  return $resolved
}

function Test-RuntimeComponentArchives {
  param([string]$Workspace)

  Write-Step 'Checking runtime component archives'
  $version = (Get-Content -LiteralPath (Join-Path $Workspace 'package.json') -Raw | ConvertFrom-Json).version
  $archives = @(
    "release\AudraFlow_${version}_windows_whisper-runtime.zip",
    "release\AudraFlow_${version}_windows_ffmpeg-runtime.zip"
  )
  foreach ($relativePath in $archives) {
    $item = Assert-File -Path (Join-Path $Workspace $relativePath) -MinBytes 1024
    Write-Host ("OK {0} ({1:N0} bytes)" -f $relativePath, $item.Length)
  }
}

function Install-TestRuntimeComponents {
  param([string]$Workspace)

  Write-Step 'Installing runtime components into temporary AppData for smoke test'
  $version = (Get-Content -LiteralPath (Join-Path $Workspace 'package.json') -Raw | ConvertFrom-Json).version
  $root = Join-Path $env:TEMP 'audraflow-verify-appdata'
  Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $root | Out-Null
  $env:AUDRAFLOW_APP_DATA_DIR = $root

  $components = @(
    @{ Id = 'whisper'; Archive = "release\AudraFlow_${version}_windows_whisper-runtime.zip" },
    @{ Id = 'ffmpeg'; Archive = "release\AudraFlow_${version}_windows_ffmpeg-runtime.zip" }
  )
  foreach ($component in $components) {
    $archivePath = Join-Path $Workspace $component.Archive
    Assert-File -Path $archivePath -MinBytes 1024 | Out-Null
    $destination = Join-Path $root ("runtime\components\{0}\bin" -f $component.Id)
    New-Item -ItemType Directory -Force $destination | Out-Null
    Expand-Archive -LiteralPath $archivePath -DestinationPath $destination -Force
    Write-Host "OK $($component.Id): $destination"
  }
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
  Test-RuntimeComponentArchives -Workspace $workspace

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
  Install-TestRuntimeComponents -Workspace $workspace

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
