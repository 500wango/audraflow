param(
  [string]$PythonVersion = '3.11.9',
  [string]$RuntimeDir = 'release\windows-runtime',
  [switch]$Force,
  [switch]$SkipDemucsModelWarmup
)

$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -ge 6 -and -not $IsWindows) {
  throw 'Windows runtime preparation must run on Windows.'
}

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$runtimeRoot = Join-Path $workspace $RuntimeDir
$pythonDir = Join-Path $runtimeRoot 'python'
$pythonExe = Join-Path $pythonDir 'python.exe'
$markerPath = Join-Path $pythonDir '.audraflow-runtime.json'
$pythonMajorMinor = ($PythonVersion -split '\.')[0..1] -join ''
$pythonPthPath = Join-Path $pythonDir "python$pythonMajorMinor._pth"
$pythonZip = Join-Path $env:TEMP "audraflow-python-$PythonVersion-embed-amd64.zip"
$getPipPath = Join-Path $env:TEMP 'audraflow-get-pip.py'
$pythonUrl = "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-embed-amd64.zip"
$getPipUrl = 'https://bootstrap.pypa.io/get-pip.py'
$pythonPackages = @(
  'torch==2.5.1+cpu',
  'torchaudio==2.5.1+cpu',
  'demucs==4.0.1',
  'funasr==1.3.14',
  'modelscope==1.38.1'
)

function Invoke-Checked {
  param(
    [string]$FilePath,
    [string[]]$Arguments,
    [string]$WorkingDirectory = $workspace
  )

  Write-Host "> $FilePath $($Arguments -join ' ')"
  Push-Location $WorkingDirectory
  try {
    & $FilePath @Arguments
    $exitCode = $LASTEXITCODE
  } finally {
    Pop-Location
  }

  if ($exitCode -ne 0) {
    throw "Command failed with exit code ${exitCode}: $FilePath $($Arguments -join ' ')"
  }
}

function Set-RuntimePythonEnvironment {
  $env:PYTHONUTF8 = '1'
  $env:PYTHONNOUSERSITE = '1'
  $env:TORCH_HOME = Join-Path $pythonDir 'torch-cache'
  $env:HF_HOME = Join-Path $pythonDir 'hf-cache'
  $env:MODELSCOPE_CACHE = Join-Path $pythonDir 'modelscope-cache'
}

function Test-RuntimeReady {
  if (-not (Test-Path -LiteralPath $pythonExe -PathType Leaf)) {
    return $false
  }
  if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
    return $false
  }

  $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
  if ($marker.pythonVersion -ne $PythonVersion) {
    return $false
  }
  $expectedPackages = ($pythonPackages -join '|')
  if ($marker.packages -ne $expectedPackages) {
    return $false
  }

  Set-RuntimePythonEnvironment
  & $pythonExe -c "import demucs, funasr, modelscope, torch, torchaudio; print('AudraFlow Windows Python runtime ready')"
  if ($LASTEXITCODE -ne 0) {
    return $false
  }
  & $pythonExe -m demucs --help *> $null
  return $LASTEXITCODE -eq 0
}

function Enable-EmbeddedPythonSitePackages {
  if (-not (Test-Path -LiteralPath $pythonPthPath -PathType Leaf)) {
    throw "Embedded Python ._pth file not found: $pythonPthPath"
  }

  $lines = Get-Content -LiteralPath $pythonPthPath
  $updated = @()
  $hasSitePackages = $false
  $hasImportSite = $false

  foreach ($line in $lines) {
    if ($line.Trim() -eq 'Lib\site-packages') {
      $hasSitePackages = $true
    }
    if ($line.Trim() -eq 'import site' -or $line.Trim() -eq '#import site') {
      $updated += 'import site'
      $hasImportSite = $true
    } else {
      $updated += $line
    }
  }

  if (-not $hasSitePackages) {
    $updated += 'Lib\site-packages'
  }
  if (-not $hasImportSite) {
    $updated += 'import site'
  }

  Set-Content -LiteralPath $pythonPthPath -Value $updated -Encoding ASCII
}

if (-not $Force -and (Test-RuntimeReady)) {
  Write-Host "Windows Python runtime ready: $pythonDir"
  exit 0
}

Remove-Item -LiteralPath $pythonDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $pythonDir | Out-Null

Write-Host "Downloading embedded Python $PythonVersion..."
Invoke-WebRequest -UseBasicParsing -Uri $pythonUrl -OutFile $pythonZip
Expand-Archive -LiteralPath $pythonZip -DestinationPath $pythonDir -Force
Enable-EmbeddedPythonSitePackages

Write-Host 'Installing pip into embedded Python...'
Invoke-WebRequest -UseBasicParsing -Uri $getPipUrl -OutFile $getPipPath
Set-RuntimePythonEnvironment
Invoke-Checked -FilePath $pythonExe -Arguments @($getPipPath, '--no-warn-script-location') -WorkingDirectory $pythonDir
Invoke-Checked -FilePath $pythonExe -Arguments @('-m', 'pip', 'install', '--no-cache-dir', '--upgrade', 'pip', 'setuptools', 'wheel') -WorkingDirectory $pythonDir

Write-Host 'Installing AudraFlow bundled Python packages...'
$installArgs = @(
  '-m',
  'pip',
  'install',
  '--no-cache-dir',
  '--extra-index-url',
  'https://download.pytorch.org/whl/cpu'
) + $pythonPackages
Invoke-Checked -FilePath $pythonExe -Arguments $installArgs -WorkingDirectory $pythonDir

Write-Host 'Checking bundled Python packages...'
Invoke-Checked -FilePath $pythonExe -Arguments @('-c', "import demucs, funasr, modelscope, torch, torchaudio; print('AudraFlow Windows Python runtime ready')") -WorkingDirectory $pythonDir
Invoke-Checked -FilePath $pythonExe -Arguments @('-m', 'demucs', '--help') -WorkingDirectory $pythonDir

if (-not $SkipDemucsModelWarmup -and $env:AUDRAFLOW_SKIP_DEMUCS_MODEL_WARMUP -ne '1') {
  Write-Host 'Preloading Demucs htdemucs model into the bundled runtime cache...'
  $warmupScript = @'
from demucs.pretrained import get_model
model = get_model(name="htdemucs")
print("Demucs model ready:", type(model).__name__)
'@
  Invoke-Checked -FilePath $pythonExe -Arguments @('-c', $warmupScript) -WorkingDirectory $pythonDir
}

$marker = [ordered]@{
  pythonVersion = $PythonVersion
  packages = ($pythonPackages -join '|')
  demucsModelWarmup = (-not $SkipDemucsModelWarmup -and $env:AUDRAFLOW_SKIP_DEMUCS_MODEL_WARMUP -ne '1')
  createdAt = (Get-Date).ToString('o')
}
$marker | ConvertTo-Json | Set-Content -LiteralPath $markerPath -Encoding UTF8

Write-Host "Windows Python runtime ready: $pythonDir"
