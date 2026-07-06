import { copyFile, mkdir, readdir, rm, stat } from 'node:fs/promises';
import { basename, delimiter, dirname, join } from 'node:path';
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
const windowsResourceBinDir = join(binariesDir, 'windows-bin');
const requiredWindowsRuntimeDllNames = [
  'vcruntime140.dll',
  'vcruntime140_1.dll',
  'msvcp140.dll',
];
const optionalWindowsRuntimeDllNames = [
  'concrt140.dll',
  'msvcp140_1.dll',
  'msvcp140_2.dll',
  'msvcp140_atomic_wait.dll',
  'msvcp140_codecvt_ids.dll',
  'vcomp140.dll',
  'vcruntime140_threads.dll',
];
const vcRuntimeFamilies = [
  'Microsoft.VC143.CRT',
  'Microsoft.VC142.CRT',
  'Microsoft.VC141.CRT',
];
const sidecars = [
  { packageName: 'audraflow-orchestrator', binaryName: 'audraflow-orchestrator' },
  { packageName: 'audraflow-asr-runtime', binaryName: 'audraflow-asr-runtime' },
];

console.log(`staging sidecars for target: ${targetTriple}`);
console.log(`cargo release directory: ${releaseDir}`);
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
    bundleName: 'yt-dlp',
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
const windowsToolSources = [
  {
    bundleName: 'whisper-cli',
    sources: [
      join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'whisper-cli.exe'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'whisper-cli.exe'),
      join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release', 'whisper-cli.exe'),
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
  {
    bundleName: 'yt-dlp',
    sources: [
      join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'yt-dlp.exe'),
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
  {
    bundleName: 'yt-dlp',
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
  return source;
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
  return source;
}

async function copyIntoDir(source, destinationDir, destinationName = basename(source)) {
  await mkdir(destinationDir, { recursive: true });
  const destination = join(destinationDir, destinationName);
  await copyFile(source, destination);
  console.log(`staged ${destination}`);
}

async function copySiblingDlls(source, destinationDir) {
  const sourceDir = dirname(source);
  const dirs = [sourceDir];
  const dirName = basename(sourceDir).toLowerCase();
  if (dirName === 'release' || dirName === 'debug') {
    dirs.push(dirname(sourceDir));
  }

  const seen = new Set();
  for (const dir of dirs) {
    let entries = [];
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch {
      continue;
    }

    for (const entry of entries) {
      if (!entry.isFile() || !entry.name.toLowerCase().endsWith('.dll')) {
        continue;
      }
      const dllSource = join(dir, entry.name);
      const key = entry.name.toLowerCase();
      if (seen.has(key)) {
        continue;
      }
      seen.add(key);
      await copyIntoDir(dllSource, destinationDir);
    }
  }
}

function envValue(...names) {
  for (const name of names) {
    const value = process.env[name];
    if (value && value.trim()) {
      return value;
    }
  }
  return null;
}

function addSearchRoot(roots, path, depth = 0) {
  if (!path) {
    return;
  }
  const key = path.toLowerCase();
  if (!roots.some((root) => root.path.toLowerCase() === key)) {
    roots.push({ path, depth });
  }
}

function addVCRedistRoot(roots, redistRoot) {
  if (!redistRoot) {
    return;
  }
  for (const family of vcRuntimeFamilies) {
    addSearchRoot(roots, join(redistRoot, 'x64', family));
  }
  addSearchRoot(roots, redistRoot);
}

function windowsRuntimeDllSearchRoots() {
  const roots = [];
  addSearchRoot(roots, join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin'));
  addSearchRoot(roots, join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin'));
  addSearchRoot(roots, join(workspaceRoot, 'external', 'whisper.cpp', 'build', 'bin', 'Release'));

  const vcToolsRedistDir = envValue('VCToolsRedistDir', 'VCTOOLSREDISTDIR');
  if (vcToolsRedistDir) {
    addVCRedistRoot(roots, vcToolsRedistDir);
  }

  for (const programFiles of [
    envValue('ProgramFiles(x86)', 'PROGRAMFILES(X86)'),
    envValue('ProgramFiles', 'PROGRAMFILES'),
  ]) {
    if (!programFiles) {
      continue;
    }
    for (const edition of ['BuildTools', 'Community', 'Professional', 'Enterprise']) {
      addSearchRoot(
        roots,
        join(programFiles, 'Microsoft Visual Studio', '2022', edition, 'VC', 'Redist', 'MSVC'),
        1,
      );
    }
  }

  const pathDirs = (process.env.PATH || process.env.Path || '')
    .split(delimiter)
    .map((value) => value.trim())
    .filter(Boolean);
  for (const pathDir of pathDirs) {
    addSearchRoot(roots, pathDir);
  }

  const systemRoot = envValue('SystemRoot', 'WINDIR');
  if (systemRoot) {
    addSearchRoot(roots, join(systemRoot, 'System32'));
  }

  return roots;
}

async function findFileRecursive(root, fileName, maxDepth) {
  const direct = join(root, fileName);
  try {
    await ensureFile(direct);
    return direct;
  } catch {
    // Search subdirectories below.
  }

  if (maxDepth <= 0) {
    return null;
  }

  let entries = [];
  try {
    entries = await readdir(root, { withFileTypes: true });
  } catch {
    return null;
  }

  for (const entry of entries) {
    if (!entry.isDirectory()) {
      continue;
    }
    const found = await findFileRecursive(join(root, entry.name), fileName, maxDepth - 1);
    if (found) {
      return found;
    }
  }

  return null;
}

async function findVisualStudioRuntimeDll(root, fileName) {
  let versions = [];
  try {
    versions = await readdir(root, { withFileTypes: true });
  } catch {
    return null;
  }

  for (const version of versions.filter((entry) => entry.isDirectory())) {
    for (const family of vcRuntimeFamilies) {
      const candidate = join(root, version.name, 'x64', family, fileName);
      try {
        await ensureFile(candidate);
        return candidate;
      } catch {
        // Try the next runtime family/version.
      }
    }
  }
  return null;
}

async function stageWindowsRuntimeDlls() {
  const searchRoots = windowsRuntimeDllSearchRoots();
  const missing = [];
  const seen = new Set();
  for (const name of [...requiredWindowsRuntimeDllNames, ...optionalWindowsRuntimeDllNames]) {
    const key = name.toLowerCase();
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    let found = null;
    for (const root of searchRoots) {
      found = root.depth === 1
        ? await findVisualStudioRuntimeDll(root.path, name)
        : await findFileRecursive(root.path, name, root.depth);
      if (found) {
        break;
      }
    }
    if (found) {
      await copyIntoDir(found, windowsResourceBinDir, name);
    } else if (requiredWindowsRuntimeDllNames.includes(name)) {
      missing.push(name);
    } else {
      console.log(`skipped optional Windows runtime DLL: ${name}`);
    }
  }

  if (missing.length > 0) {
    const searchedRoots = searchRoots
      .map((root) => `${root.path}${root.depth > 0 ? ` (depth ${root.depth})` : ''}`)
      .join('\n  ');
    throw new Error(
      `Missing required Windows runtime DLL(s): ${missing.join(', ')}. Install Microsoft Visual C++ Redistributable 2015-2022 x64 or build whisper-cli with a static runtime.\nSearched:\n  ${searchedRoots}`,
    );
  }
}

async function stageWindowsResourceTool(tool) {
  const source = await findExistingFile(tool.sources);
  if (!source) {
    throw new Error(`Could not find required bundled tool: ${tool.bundleName}`);
  }

  await copyIntoDir(source, windowsResourceBinDir, `${tool.bundleName}.exe`);
  await copySiblingDlls(source, windowsResourceBinDir);
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
  await rm(windowsResourceBinDir, { recursive: true, force: true });
  for (const tool of windowsToolSources) {
    await stageExternalTool(tool);
    await stageWindowsResourceTool(tool);
  }
  await stageWindowsRuntimeDlls();
} else if (isMacosTarget) {
  for (const tool of macosToolSources) {
    await stageExternalTool(tool);
  }
}
