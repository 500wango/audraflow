import { createWriteStream } from 'node:fs';
import { chmod, copyFile, mkdir, readFile, rename, rm, stat } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { spawnSync } from 'node:child_process';

const workspaceRoot = process.cwd();
const targetTriple = process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET || detectHostTriple();
const isWindowsTarget = targetTriple.includes('windows');
const isLinuxTarget = targetTriple.includes('linux');
const isMacosTarget = targetTriple.includes('apple-darwin') || targetTriple.includes('darwin');

// Must match src-tauri/src/lib.rs DEFAULT_WHISPER_MODEL_* constants.
const DEFAULT_MODEL_NAME = 'ggml-base.bin';
const DEFAULT_MODEL_MIN_BYTES = 100 * 1024 * 1024;
const DEFAULT_MODEL_EXPECTED_BYTES = 147_951_465;
const DEFAULT_MODEL_URL =
  process.env.AUDRAFLOW_DEFAULT_MODEL_URL ||
  'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin';

const ytDlpAsset = (() => {
  if (isWindowsTarget) {
    return null;
  }
  if (isLinuxTarget) {
    return {
      url: 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux',
      path: join(workspaceRoot, 'release', 'linux-portable', 'AudraFlow', 'bin', 'yt-dlp'),
    };
  }
  if (isMacosTarget) {
    return {
      url: 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos',
      path: join(workspaceRoot, 'release', 'macos-portable', 'AudraFlow', 'bin', 'yt-dlp'),
    };
  }
  return null;
})();

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

async function pathExists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

async function fileSize(path) {
  return (await stat(path)).size;
}

async function downloadFile(url, destination) {
  await mkdir(dirname(destination), { recursive: true });
  const tempPath = `${destination}.tmp`;
  await rm(tempPath, { force: true });

  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok || !response.body) {
    throw new Error(`Download failed (${response.status}): ${url}`);
  }

  await pipeline(response.body, createWriteStream(tempPath));
  await assertNotHtml(tempPath, url);
  await rename(tempPath, destination);
}

async function assertNotHtml(path, url) {
  const head = await readFile(path, { encoding: null });
  const preview = head.subarray(0, 512).toString('utf8').trimStart().toLowerCase();
  if (preview.startsWith('<!doctype html') || preview.startsWith('<html')) {
    await rm(path, { force: true });
    throw new Error(`Downloaded HTML instead of a binary asset: ${url}`);
  }
}

async function isUsableFile(path, minBytes) {
  if (!(await pathExists(path))) {
    return false;
  }
  const size = await fileSize(path);
  return size >= minBytes;
}

async function ensureYtDlp() {
  if (!ytDlpAsset) {
    console.log(`skipping yt-dlp asset for unsupported target: ${targetTriple}`);
    return;
  }

  if (await pathExists(ytDlpAsset.path)) {
    const size = await fileSize(ytDlpAsset.path);
    if (size > 1024 * 1024) {
      if (!isWindowsTarget) {
        await chmod(ytDlpAsset.path, 0o755);
      }
      console.log(`yt-dlp ready: ${ytDlpAsset.path}`);
      return;
    }
    await rm(ytDlpAsset.path, { force: true });
  }

  console.log(`downloading yt-dlp: ${ytDlpAsset.url}`);
  await downloadFile(ytDlpAsset.url, ytDlpAsset.path);
  const size = await fileSize(ytDlpAsset.path);
  if (size <= 1024 * 1024) {
    await rm(ytDlpAsset.path, { force: true });
    throw new Error(`yt-dlp asset is too small: ${size} bytes`);
  }
  if (!isWindowsTarget) {
    await chmod(ytDlpAsset.path, 0o755);
  }
  console.log(`yt-dlp ready: ${ytDlpAsset.path}`);
}

/**
 * Bundled default Whisper base model for first-run transcription.
 * Tauri packages `src-tauri/default-models/ggml-base.bin` as a resource.
 * Also mirror under release/default-models for portable/docs paths.
 */
async function ensureDefaultWhisperModel() {
  const bundledPath = join(workspaceRoot, 'src-tauri', 'default-models', DEFAULT_MODEL_NAME);
  const releasePath = join(workspaceRoot, 'release', 'default-models', DEFAULT_MODEL_NAME);
  const candidates = [
    process.env.AUDRAFLOW_DEFAULT_MODEL_BIN,
    releasePath,
    join(workspaceRoot, 'external', 'whisper.cpp', 'models', DEFAULT_MODEL_NAME),
  ].filter(Boolean);

  let source = null;
  for (const candidate of candidates) {
    if (await isUsableFile(candidate, DEFAULT_MODEL_MIN_BYTES)) {
      source = candidate;
      break;
    }
  }

  if (await isUsableFile(bundledPath, DEFAULT_MODEL_MIN_BYTES)) {
    console.log(`default Whisper model ready: ${bundledPath}`);
  } else if (source) {
    await mkdir(dirname(bundledPath), { recursive: true });
    await copyFile(source, bundledPath);
    console.log(`default Whisper model copied: ${source} -> ${bundledPath}`);
  } else {
    console.log(`downloading default Whisper model: ${DEFAULT_MODEL_URL}`);
    await downloadFile(DEFAULT_MODEL_URL, bundledPath);
    const size = await fileSize(bundledPath);
    if (size < DEFAULT_MODEL_MIN_BYTES) {
      await rm(bundledPath, { force: true });
      throw new Error(`default Whisper model is too small: ${size} bytes`);
    }
    if (size !== DEFAULT_MODEL_EXPECTED_BYTES) {
      console.log(
        `warning: default model size ${size} != expected ${DEFAULT_MODEL_EXPECTED_BYTES} (continuing)`,
      );
    }
    console.log(`default Whisper model ready: ${bundledPath} (${size} bytes)`);
  }

  // Keep release/ mirror for docs and portable packaging checks.
  if (!(await isUsableFile(releasePath, DEFAULT_MODEL_MIN_BYTES))) {
    await mkdir(dirname(releasePath), { recursive: true });
    await copyFile(bundledPath, releasePath);
    console.log(`default Whisper model mirrored: ${releasePath}`);
  }
}

await ensureDefaultWhisperModel();
await ensureYtDlp();
