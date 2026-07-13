import { copyFile, mkdir, readdir, rm, stat } from 'node:fs/promises';
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

console.log(`staging sidecars for target: ${targetTriple}`);
console.log(`cargo release directory: ${releaseDir}`);

const linuxToolSources = [
  {
    bundleName: 'audraflow-whisper-cli',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'whisper-cli'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build-linux', 'bin', 'whisper-cli'),
    ],
  },
  {
    bundleName: 'audraflow-llama-funasr-cli',
    optional: true,
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'llama-funasr-cli'),
      join(workspaceRoot, 'external', 'Fun-ASR', 'runtime', 'llama.cpp', 'build', 'bin', 'llama-funasr-cli'),
      join(workspaceRoot, 'external', 'funasr-llamacpp', 'bin', 'llama-funasr-cli'),
    ],
  },
  {
    bundleName: 'audraflow-ffmpeg',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'ffmpeg'),
      join(workspaceRoot, 'external', 'ffmpeg', 'bin', 'ffmpeg'),
      '/usr/local/bin/ffmpeg',
      '/usr/bin/ffmpeg',
    ],
  },
  {
    bundleName: 'audraflow-ffprobe',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'ffprobe'),
      join(workspaceRoot, 'external', 'ffmpeg', 'bin', 'ffprobe'),
      '/usr/local/bin/ffprobe',
      '/usr/bin/ffprobe',
    ],
  },
  {
    bundleName: 'audraflow-yt-dlp',
    sources: [
      join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'yt-dlp'),
      '/usr/local/bin/yt-dlp',
      '/usr/bin/yt-dlp',
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

const macosToolSources = [
  {
    bundleName: 'audraflow-whisper-cli',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'whisper-cli'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'whisper-cli'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'whisper-cli'),
      '/opt/homebrew/bin/whisper-cli',
      '/usr/local/bin/whisper-cli',
    ],
  },
  {
    bundleName: 'audraflow-ffmpeg',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'ffmpeg'),
      '/opt/homebrew/bin/ffmpeg',
      '/usr/local/bin/ffmpeg',
    ],
  },
  {
    bundleName: 'audraflow-ffprobe',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'ffprobe'),
      '/opt/homebrew/bin/ffprobe',
      '/usr/local/bin/ffprobe',
    ],
  },
  {
    bundleName: 'audraflow-yt-dlp',
    sources: [
      join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'yt-dlp'),
      '/opt/homebrew/bin/yt-dlp',
      '/usr/local/bin/yt-dlp',
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

  // For Linux shared libraries (.so), copy them without target triple suffix
  // so that whisper-cli can find them during development/run.
  if (bundleName.includes('.so')) {
    const devDestination = join(binariesDir, bundleName);
    await copyFile(source, devDestination);
    console.log(`staged library for development: ${devDestination}`);
  }
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

  // For Linux shared libraries (.so), copy them without target triple suffix
  // so that whisper-cli can find them during development/run.
  if (tool.bundleName.includes('.so')) {
    const devDestination = join(binariesDir, tool.bundleName);
    await copyFile(source, devDestination);
    console.log(`staged library for development: ${devDestination}`);
  }
}

async function clearStagedTargetBinaries() {
  let entries = [];
  try {
    entries = await readdir(binariesDir);
  } catch {
    return;
  }

  const targetSuffix = `-${targetTriple}${extension}`;
  await Promise.all(entries
    .filter((entry) => entry.endsWith(targetSuffix))
    .map((entry) => rm(join(binariesDir, entry), { force: true })));
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
await clearStagedTargetBinaries();

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
  const windowsToolSources = [
    {
      bundleName: 'audraflow-whisper-cli',
      sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'whisper-cli.exe'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'whisper-cli.exe'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'whisper-cli.exe'),
      ],
    },
    {
      bundleName: 'audraflow-ffmpeg',
      sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ffmpeg.exe'),
        join(workspaceRoot, 'external', 'ffmpeg', 'bin', 'ffmpeg.exe'),
      ],
    },
    {
      bundleName: 'audraflow-ffprobe',
      sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ffprobe.exe'),
        join(workspaceRoot, 'external', 'ffmpeg', 'bin', 'ffprobe.exe'),
      ],
    },
    {
      bundleName: 'audraflow-yt-dlp',
      optional: true,
      sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'yt-dlp.exe'),
        join(workspaceRoot, 'external', 'yt-dlp', 'yt-dlp.exe'),
      ],
    },
  ];

  // Whisper DLLs that whisper-cli.exe depends on at runtime.
  const windowsWhisperDlls = [
    { name: 'whisper.dll', sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'whisper.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'whisper.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'whisper.dll'),
      ] },
    { name: 'ggml.dll', sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ggml.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'ggml.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'ggml.dll'),
      ] },
    { name: 'ggml-base.dll', sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ggml-base.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'ggml-base.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'ggml-base.dll'),
      ] },
    { name: 'ggml-cpu.dll', sources: [
        join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'ggml-cpu.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'ggml-cpu.dll'),
        join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'ggml-cpu.dll'),
      ] },
  ];

  for (const tool of windowsToolSources) {
    if (tool.optional) {
      await stageOptionalExternalTool(tool);
    } else {
      await stageExternalTool(tool);
    }
  }

  // Stage whisper + ffmpeg tools into windows-runtime/ for:
  // 1) NSIS post-install pre-seed into %APPDATA%\com.audraflow.app\runtime\components
  // 2) App first-run seed (also covers MSI installs without NSIS hooks)
  const whisperRuntimeDir = join(workspaceRoot, 'src-tauri', 'windows-runtime');
  await mkdir(whisperRuntimeDir, { recursive: true });
  let dllsStaged = 0;
  for (const dll of windowsWhisperDlls) {
    const source = await findExistingFile(dll.sources);
    if (source) {
      const dest = join(whisperRuntimeDir, dll.name);
      await copyFile(source, dest);
      console.log(`staged whisper DLL ${dest}`);
      dllsStaged++;
    } else {
      console.log(`skipped whisper DLL (not found): ${dll.name}`);
    }
  }
  // Always try to stage whisper-cli.exe into windows-runtime/
  const whisperCliSource = await findExistingFile(windowsToolSources[0].sources);
  if (whisperCliSource) {
    await copyFile(whisperCliSource, join(whisperRuntimeDir, 'whisper-cli.exe'));
    console.log(`staged whisper-cli.exe to windows-runtime/`);
  } else if (dllsStaged > 0) {
    console.log('warning: whisper DLLs staged but whisper-cli.exe was not found');
  }

  // Stage ffmpeg/ffprobe into windows-runtime for install-time pre-seed.
  for (const tool of [
    { name: 'ffmpeg.exe', sources: windowsToolSources[1].sources },
    { name: 'ffprobe.exe', sources: windowsToolSources[2].sources },
  ]) {
    const source = await findExistingFile(tool.sources);
    if (source) {
      const dest = join(whisperRuntimeDir, tool.name);
      await copyFile(source, dest);
      console.log(`staged ${tool.name} to windows-runtime/`);
    } else {
      console.log(`skipped ${tool.name} for windows-runtime (not found)`);
    }
  }
} else if (isMacosTarget) {
  for (const tool of macosToolSources) {
    await stageExternalTool(tool);
  }
}
