import { copyFile, mkdir, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const workspaceRoot = process.cwd();
const targetTriple = process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET || detectHostTriple();
const isWindowsTarget = targetTriple.includes('windows');
const isMacosTarget = targetTriple.includes('apple-darwin') || targetTriple.includes('darwin');
const extension = isWindowsTarget ? '.exe' : '';
const targetDir = process.env.CARGO_TARGET_DIR
  ? process.env.CARGO_TARGET_DIR
  : join(workspaceRoot, 'target');
const releaseDir = process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET
  ? join(targetDir, targetTriple, 'release')
  : join(targetDir, 'release');
const binariesDir = join(workspaceRoot, 'src-tauri', 'binaries');
const sidecars = [
  { packageName: 'audraflow-orchestrator', binaryName: 'audraflow-orchestrator' },
  { packageName: 'audraflow-asr-runtime', binaryName: 'audraflow-asr-runtime' },
];
const linuxToolSources = [
  {
    bundleName: 'whisper-cli',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'whisper-cli'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build-linux', 'bin', 'whisper-cli'),
    ],
  },
  {
    bundleName: 'llama-funasr-cli',
    optional: true,
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'llama-funasr-cli'),
      join(workspaceRoot, 'external', 'Fun-ASR', 'runtime', 'llama.cpp', 'build', 'bin', 'llama-funasr-cli'),
      join(workspaceRoot, 'external', 'funasr-llamacpp', 'bin', 'llama-funasr-cli'),
    ],
  },
  {
    bundleName: 'ffmpeg',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'ffmpeg'),
      join(workspaceRoot, 'external', 'ffmpeg', 'bin', 'ffmpeg'),
      '/usr/local/bin/ffmpeg',
      '/usr/bin/ffmpeg',
    ],
  },
  {
    bundleName: 'ffprobe',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'ffprobe'),
      join(workspaceRoot, 'external', 'ffmpeg', 'bin', 'ffprobe'),
      '/usr/local/bin/ffprobe',
      '/usr/bin/ffprobe',
    ],
  },
  {
    bundleName: 'libwhisper.so.1',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'libwhisper.so.1'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build-linux', 'bin', 'libwhisper.so.1'),
    ],
  },
  {
    bundleName: 'libggml.so.0',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'libggml.so.0'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build-linux', 'bin', 'libggml.so.0'),
    ],
  },
  {
    bundleName: 'libggml-base.so.0',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'libggml-base.so.0'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build-linux', 'bin', 'libggml-base.so.0'),
    ],
  },
  {
    bundleName: 'libggml-cpu.so.0',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'libggml-cpu.so.0'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build-linux', 'bin', 'libggml-cpu.so.0'),
    ],
  },
];
const windowsToolSources = [
  {
    bundleName: 'whisper-cli',
    sources: [
      join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'whisper-cli.exe'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'whisper-cli.exe'),
    ],
  },
  {
    bundleName: 'ffmpeg',
    sources: [
      join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ffmpeg.exe'),
    ],
  },
  {
    bundleName: 'ffprobe',
    sources: [
      join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ffprobe.exe'),
    ],
  },
];
const macosToolSources = [
  {
    bundleName: 'whisper-cli',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'whisper-cli'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'whisper-cli'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'whisper-cli'),
      '/opt/homebrew/bin/whisper-cli',
      '/usr/local/bin/whisper-cli',
    ],
  },
  {
    bundleName: 'ffmpeg',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'ffmpeg'),
      '/opt/homebrew/bin/ffmpeg',
      '/usr/local/bin/ffmpeg',
    ],
  },
  {
    bundleName: 'ffprobe',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'ffprobe'),
      '/opt/homebrew/bin/ffprobe',
      '/usr/local/bin/ffprobe',
    ],
  },
];

function detectHostTriple() {
  const result = spawnSync('rustc', ['-vV'], {
    cwd: workspaceRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  if (result.status !== 0) {
    throw new Error('Failed to detect Rust host target with `rustc -vV`.');
  }

  const hostLine = result.stdout.split('\n').find((line) => line.startsWith('host: '));
  const host = hostLine?.slice('host: '.length).trim();
  if (!host) {
    throw new Error('Could not parse host target from `rustc -vV` output.');
  }
  return host;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: workspaceRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.status !== 0) {
    throw new Error(`Command failed: ${command} ${args.join(' ')}`);
  }
}

async function ensureFile(path) {
  const metadata = await stat(path);
  if (!metadata.isFile() || metadata.size === 0) {
    throw new Error(`Sidecar binary is missing or empty: ${path}`);
  }
}

async function findExistingFile(candidates) {
  for (const candidate of candidates) {
    try {
      await ensureFile(candidate);
      return candidate;
    } catch {
      // Try the next candidate.
    }
  }
  return null;
}

async function stageExternalTool({ bundleName, sources }) {
  const source = await findExistingFile(sources);
  if (!source) {
    throw new Error(`Could not find required bundled tool: ${bundleName}`);
  }

  const destination = join(binariesDir, `${bundleName}-${targetTriple}${extension}`);
  await copyFile(source, destination);
  console.log(`staged ${destination}`);
}

async function stageOptionalExternalTool(tool) {
  const source = await findExistingFile(tool.sources);
  if (!source) {
    console.log(`skipped optional bundled tool: ${tool.bundleName}`);
    return;
  }

  const destination = join(binariesDir, `${tool.bundleName}-${targetTriple}${extension}`);
  await copyFile(source, destination);
  console.log(`staged ${destination}`);
}

const cargoArgs = ['build', '--release'];
if (process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET) {
  cargoArgs.push('--target', targetTriple);
}
for (const sidecar of sidecars) {
  cargoArgs.push('-p', sidecar.packageName);
}

run('cargo', cargoArgs);
await mkdir(binariesDir, { recursive: true });

for (const sidecar of sidecars) {
  const source = join(releaseDir, `${sidecar.binaryName}${extension}`);
  const destination = join(binariesDir, `${sidecar.binaryName}-${targetTriple}${extension}`);
  await ensureFile(source);
  await copyFile(source, destination);
  console.log(`staged ${destination}`);
}

if (targetTriple.includes('linux')) {
  for (const tool of linuxToolSources) {
    if (tool.optional) {
      await stageOptionalExternalTool(tool);
    } else {
      await stageExternalTool(tool);
    }
  }
} else if (targetTriple.includes('windows')) {
  for (const tool of windowsToolSources) {
    await stageExternalTool(tool);
  }
} else if (isMacosTarget) {
  for (const tool of macosToolSources) {
    await stageExternalTool(tool);
  }
}
