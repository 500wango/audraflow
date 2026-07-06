param(
  [string]$AudioPath,
  [string]$ModelPath,
  [string]$OrchestratorPath,
  [string]$OrchestratorWorkingDirectory,
  [string]$DbPath,
  [int]$TimeoutSeconds = 60,
  [switch]$SkipBuild,
  [switch]$KeepRecord
)

$ErrorActionPreference = 'Stop'

function Resolve-WorkspaceRoot {
  $scriptDir = Split-Path -Parent $PSCommandPath
  return (Resolve-Path (Join-Path $scriptDir '..')).Path
}

function Get-SelectedModelPath {
  $modelsDir = Join-Path $env:APPDATA 'com.audraflow.app\models'
  $selectionPath = Join-Path $modelsDir 'selected-model.json'
  if (-not (Test-Path -LiteralPath $selectionPath)) {
    return $null
  }

  $selection = Get-Content -LiteralPath $selectionPath -Raw | ConvertFrom-Json
  $modelJson = Join-Path $modelsDir ("{0}-v{1}\model.json" -f $selection.name, $selection.version)
  if (-not (Test-Path -LiteralPath $modelJson)) {
    return $null
  }

  $installed = Get-Content -LiteralPath $modelJson -Raw | ConvertFrom-Json
  if ($installed.path -and (Test-Path -LiteralPath $installed.path -PathType Leaf)) {
    return $installed.path
  }
  return $null
}

function Send-AudraFlowIpc {
  param([hashtable]$Payload)

  $envelope = [ordered]@{
    messageId = [guid]::NewGuid().ToString()
    timestampMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
  }
  foreach ($key in $Payload.Keys) {
    $envelope[$key] = $Payload[$key]
  }

  $json = $envelope | ConvertTo-Json -Depth 8 -Compress
  $client = [System.IO.Pipes.NamedPipeClientStream]::new(
    '.',
    'audraflow-orchestrator',
    [System.IO.Pipes.PipeDirection]::InOut
  )

  try {
    $client.Connect(10000)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $client.Write($bytes, 0, $bytes.Length)
    $client.Flush()

    $buffer = New-Object byte[] 65536
    $read = $client.Read($buffer, 0, $buffer.Length)
    if ($read -le 0) {
      throw 'No IPC response bytes received'
    }

    $responseJson = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $read)
    return $responseJson | ConvertFrom-Json
  } finally {
    $client.Dispose()
  }
}

function Read-SmokeDbRow {
  param([string]$DbPath, [string]$JobId)

  if (Get-Command sqlite3 -ErrorAction SilentlyContinue) {
    return sqlite3 $DbPath "select jobs.state, count(segments.segment_id), coalesce(group_concat(segments.text, ' | '), '') from jobs left join segments on segments.job_id = jobs.job_id where jobs.job_id = '$JobId' group by jobs.job_id;"
  }

  if (Get-Command python -ErrorAction SilentlyContinue) {
    $env:AUDRAFLOW_SMOKE_DB = $DbPath
    $env:AUDRAFLOW_SMOKE_JOB = $JobId
    return @'
import os, sqlite3
con = sqlite3.connect(os.environ['AUDRAFLOW_SMOKE_DB'])
row = con.execute("""
select jobs.state, count(segments.segment_id), coalesce(group_concat(segments.text, ' | '), '')
from jobs left join segments on segments.job_id = jobs.job_id
where jobs.job_id = ?
group by jobs.job_id
""", (os.environ['AUDRAFLOW_SMOKE_JOB'],)).fetchone()
if row is None:
    raise SystemExit("no row")
print(f"{row[0]}|{row[1]}|{row[2]}")
'@ | python -
  }

  throw 'Install sqlite3 or python to verify SQLite smoke-test output.'
}

function Remove-SmokeRecord {
  param([string]$DbPath, [string]$JobId)

  if (Get-Command sqlite3 -ErrorAction SilentlyContinue) {
    sqlite3 $DbPath "pragma foreign_keys=on; delete from checkpoints where job_id = '$JobId'; delete from jobs where job_id = '$JobId';" | Out-Null
    return
  }

  if (Get-Command python -ErrorAction SilentlyContinue) {
    $env:AUDRAFLOW_SMOKE_DB = $DbPath
    $env:AUDRAFLOW_SMOKE_JOB = $JobId
    @'
import os, sqlite3
con = sqlite3.connect(os.environ['AUDRAFLOW_SMOKE_DB'])
job = os.environ['AUDRAFLOW_SMOKE_JOB']
con.execute("pragma foreign_keys=on")
con.execute("delete from checkpoints where job_id = ?", (job,))
con.execute("delete from jobs where job_id = ?", (job,))
con.commit()
'@ | python -
  }
}

if (-not $IsWindows -and $PSVersionTable.PSVersion.Major -ge 6) {
  throw 'This smoke test currently targets the Windows Named Pipe orchestrator.'
}

$workspace = Resolve-WorkspaceRoot
Set-Location $workspace

if (-not $AudioPath) {
  $AudioPath = Join-Path $workspace 'external\whisper.cpp\samples\jfk.mp3'
}
if (-not (Test-Path -LiteralPath $AudioPath -PathType Leaf)) {
  throw "Audio file not found: $AudioPath"
}
$AudioPath = (Resolve-Path $AudioPath).Path

if (-not $ModelPath) {
  $ModelPath = Get-SelectedModelPath
}
if (-not $ModelPath) {
  $candidate = Join-Path $workspace 'external\whisper.cpp\models\ggml-tiny.bin'
  if (Test-Path -LiteralPath $candidate -PathType Leaf) {
    $ModelPath = $candidate
  }
}
if (-not $ModelPath -or -not (Test-Path -LiteralPath $ModelPath -PathType Leaf)) {
  throw 'No model file found. Import a model in the app or pass -ModelPath <model.bin>.'
}
$ModelPath = (Resolve-Path $ModelPath).Path

if (-not $OrchestratorPath -and -not $SkipBuild) {
  cargo build -p audraflow-orchestrator -p audraflow-asr-runtime
  if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
  }
}

if (-not $OrchestratorPath) {
  $OrchestratorPath = Join-Path $workspace 'target\debug\audraflow-orchestrator.exe'
}
$orchestratorExe = (Resolve-Path $OrchestratorPath).Path
if (-not (Test-Path -LiteralPath $orchestratorExe -PathType Leaf)) {
  throw "Orchestrator binary not found: $orchestratorExe"
}
if (-not $OrchestratorWorkingDirectory) {
  $OrchestratorWorkingDirectory = Split-Path -Parent $orchestratorExe
}
$OrchestratorWorkingDirectory = (Resolve-Path $OrchestratorWorkingDirectory).Path

if (-not $DbPath) {
  $DbPath = Join-Path $env:APPDATA 'AudraFlow\audraflow.db'
}
$dbPath = $DbPath
$jobId = 'ipc-smoke-' + [guid]::NewGuid().ToString('N')
$fileHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $AudioPath).Hash.ToLowerInvariant()
$proc = $null

try {
  $proc = Start-Process -FilePath $orchestratorExe -WorkingDirectory $OrchestratorWorkingDirectory -WindowStyle Hidden -PassThru
  Start-Sleep -Milliseconds 800

  $create = Send-AudraFlowIpc @{
    type = 'jobCreate'
    jobId = $jobId
    filePath = $AudioPath
    fileHash = $fileHash
    extremeAccuracy = $false
    exportFormats = @('json')
    modelPath = $ModelPath
    modelName = 'smoke'
    modelVersion = 'local'
    language = 'en'
    audioDurationS = 11.0
    snrDb = $null
    estimatedSpeakers = 1
  }

  if ($create.type -ne 'jobPlan') {
    throw "Expected jobPlan response, got: $($create | ConvertTo-Json -Depth 8 -Compress)"
  }
  Write-Output "Job planned: $($create.jobId) plan=$($create.planId)"

  $deadline = [DateTimeOffset]::UtcNow.AddSeconds($TimeoutSeconds)
  $final = $null
  while ([DateTimeOffset]::UtcNow -lt $deadline) {
    Start-Sleep -Seconds 1
    $status = Send-AudraFlowIpc @{
      type = 'jobStatus'
      jobId = $jobId
      state = 'pending'
      progressPct = 0
      message = $null
      estimatedRemainingS = $null
      rtfCurrent = $null
      ttfvS = $null
    }
    Write-Output ("Status: {0} {1:N1}% {2}" -f $status.state, $status.progressPct, $status.message)
    if ($status.state -in @('completed', 'failed', 'cancelled')) {
      $final = $status
      break
    }
  }

  if ($null -eq $final) {
    throw "Timed out after $TimeoutSeconds seconds waiting for job completion."
  }
  if ($final.state -ne 'completed') {
    throw "Smoke job did not complete: $($final | ConvertTo-Json -Depth 8 -Compress)"
  }

  $dbRow = Read-SmokeDbRow -DbPath $dbPath -JobId $jobId
  Write-Output "SQLite: $dbRow"
  if ($dbRow -notmatch '^completed\|[1-9]') {
    throw "SQLite verification failed: $dbRow"
  }

  Write-Output "Smoke test passed: $jobId"
} finally {
  if ($proc -and -not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force
    $proc.WaitForExit()
  }
  if (-not $KeepRecord) {
    Remove-SmokeRecord -DbPath $dbPath -JobId $jobId
  }
}
