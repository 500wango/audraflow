#!/usr/bin/env bash
set -euo pipefail

audio_path=""
model_path=""
timeout_seconds=60
skip_build=0
keep_record=0

usage() {
  cat <<'USAGE'
Usage: scripts/smoke-e2e.sh [--audio-path PATH] [--model-path PATH] [--timeout-seconds N] [--skip-build] [--keep-record]
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --audio-path)
      audio_path="${2:?Missing value for --audio-path}"
      shift 2
      ;;
    --model-path)
      model_path="${2:?Missing value for --model-path}"
      shift 2
      ;;
    --timeout-seconds)
      timeout_seconds="${2:?Missing value for --timeout-seconds}"
      shift 2
      ;;
    --skip-build)
      skip_build=1
      shift
      ;;
    --keep-record)
      keep_record=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace="$(cd "$script_dir/.." && pwd)"
app_data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/com.audraflow.app"
socket_path="${XDG_RUNTIME_DIR:-/tmp}/audraflow-orchestrator.sock"
db_path="$app_data_dir/audraflow.db"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

require_command cargo
require_command python3

if [[ -z "$audio_path" ]]; then
  audio_path="$workspace/external/whisper.cpp/samples/jfk.wav"
fi
if [[ ! -f "$audio_path" ]]; then
  echo "Audio file not found: $audio_path" >&2
  exit 1
fi
audio_path="$(realpath "$audio_path")"

if [[ -z "$model_path" ]]; then
  model_path="$(python3 - "$app_data_dir" <<'PY'
import json
import pathlib
import sys

app_data = pathlib.Path(sys.argv[1])
selection_path = app_data / "models" / "selected-model.json"
if not selection_path.is_file():
    sys.exit(0)
selection = json.loads(selection_path.read_text())
model_json = app_data / "models" / f"{selection['name']}-v{selection['version']}" / "model.json"
if not model_json.is_file():
    sys.exit(0)
installed = json.loads(model_json.read_text())
path = pathlib.Path(installed.get("path", ""))
if path.is_file():
    print(path)
PY
)"
fi
if [[ -z "$model_path" || ! -f "$model_path" ]]; then
  echo "Model file not found. Pass --model-path PATH or select a model in the app first." >&2
  exit 1
fi
model_path="$(realpath "$model_path")"

if ! command -v ffmpeg >/dev/null 2>&1 \
  && [[ -z "${AUDRAFLOW_FFMPEG_BIN:-}" && -z "${FT_FFMPEG_BIN:-}" ]] \
  && [[ ! -x "$workspace/external/ffmpeg/bin/ffmpeg" ]]; then
  echo "Missing ffmpeg. Install it or set AUDRAFLOW_FFMPEG_BIN." >&2
  exit 1
fi
if ! command -v ffprobe >/dev/null 2>&1 \
  && [[ -z "${AUDRAFLOW_FFPROBE_BIN:-}" && -z "${FT_FFPROBE_BIN:-}" ]] \
  && [[ ! -x "$workspace/external/ffmpeg/bin/ffprobe" ]]; then
  echo "Missing ffprobe. Install it or set AUDRAFLOW_FFPROBE_BIN." >&2
  exit 1
fi
if ! command -v whisper-cli >/dev/null 2>&1 \
  && [[ -z "${AUDRAFLOW_WHISPER_CLI:-}" && -z "${FT_WHISPER_CLI:-}" ]] \
  && [[ ! -f "$workspace/external/whisper.cpp/build-linux/bin/whisper-cli" ]] \
  && [[ ! -f "$workspace/external/whisper.cpp/build/bin/whisper-cli" ]]; then
  echo "Missing Linux whisper-cli. Build whisper.cpp or set AUDRAFLOW_WHISPER_CLI." >&2
  exit 1
fi

if [[ "$skip_build" -eq 0 ]]; then
  cargo build -p audraflow-orchestrator -p audraflow-asr-runtime
fi

orchestrator_bin="$workspace/target/debug/audraflow-orchestrator"
if [[ ! -x "$orchestrator_bin" ]]; then
  echo "Orchestrator binary not found: $orchestrator_bin" >&2
  exit 1
fi

rm -f "$socket_path"
"$orchestrator_bin" &
orchestrator_pid=$!
cleanup() {
  if kill -0 "$orchestrator_pid" >/dev/null 2>&1; then
    kill "$orchestrator_pid" >/dev/null 2>&1 || true
    wait "$orchestrator_pid" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

for _ in {1..50}; do
  [[ -S "$socket_path" ]] && break
  sleep 0.1
done
if [[ ! -S "$socket_path" ]]; then
  echo "Timed out waiting for orchestrator socket: $socket_path" >&2
  exit 1
fi

python3 - "$socket_path" "$audio_path" "$model_path" "$db_path" "$timeout_seconds" "$keep_record" <<'PY'
import hashlib
import json
import os
import socket
import sqlite3
import sys
import time
import uuid

socket_path, audio_path, model_path, db_path, timeout_seconds, keep_record = sys.argv[1:]
timeout_seconds = int(timeout_seconds)
keep_record = keep_record == "1"
job_id = "ipc-smoke-" + uuid.uuid4().hex

def send(payload):
    envelope = {
        "messageId": str(uuid.uuid4()),
        "timestampMs": int(time.time() * 1000),
        **payload,
    }
    data = json.dumps(envelope, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.connect(socket_path)
        client.sendall(data)
        client.shutdown(socket.SHUT_WR)
        chunks = []
        while True:
            chunk = client.recv(65536)
            if not chunk:
                break
            chunks.append(chunk)
    if not chunks:
        raise RuntimeError("No IPC response bytes received")
    return json.loads(b"".join(chunks))

with open(audio_path, "rb") as file:
    file_hash = hashlib.sha256(file.read()).hexdigest()

create = send({
    "type": "jobCreate",
    "jobId": job_id,
    "filePath": audio_path,
    "fileHash": file_hash,
    "extremeAccuracy": False,
    "exportFormats": ["json"],
    "modelPath": model_path,
    "modelName": "smoke",
    "modelVersion": "local",
    "language": "en",
    "audioDurationS": 11.0,
    "snrDb": None,
    "estimatedSpeakers": 1,
})
if create.get("type") != "jobPlan":
    raise RuntimeError(f"Expected jobPlan response, got: {json.dumps(create, separators=(',', ':'))}")
print(f"Job planned: {create.get('jobId')} plan={create.get('planId')}")

deadline = time.time() + timeout_seconds
final = None
while time.time() < deadline:
    time.sleep(1)
    status = send({
        "type": "jobStatus",
        "jobId": job_id,
        "state": "pending",
        "progressPct": 0,
        "message": None,
        "estimatedRemainingS": None,
        "rtfCurrent": None,
        "ttfvS": None,
    })
    print(f"Status: {status.get('state')} {status.get('progressPct', 0):.1f}% {status.get('message')}")
    if status.get("state") in {"completed", "failed", "cancelled"}:
        final = status
        break

if final is None:
    raise RuntimeError(f"Timed out after {timeout_seconds} seconds waiting for job completion.")
if final.get("state") != "completed":
    raise RuntimeError(f"Smoke job did not complete: {json.dumps(final, separators=(',', ':'))}")

with sqlite3.connect(db_path) as con:
    row = con.execute(
        """
        select jobs.state, count(segments.segment_id), coalesce(group_concat(segments.text, ' | '), '')
        from jobs
        left join segments on segments.job_id = jobs.job_id
        where jobs.job_id = ?
        group by jobs.job_id
        """,
        (job_id,),
    ).fetchone()
    print("SQLite:", "|".join(str(value) for value in row))
    if row is None or row[0] != "completed" or int(row[1]) < 1:
        raise RuntimeError(f"SQLite verification failed: {row}")
    if not keep_record:
        con.execute("pragma foreign_keys=on")
        con.execute("delete from checkpoints where job_id = ?", (job_id,))
        con.execute("delete from jobs where job_id = ?", (job_id,))

print(f"Smoke test passed: {job_id}")
PY
