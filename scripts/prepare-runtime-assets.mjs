import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream } from 'node:fs';
import { chmod, mkdir, readFile, rename, rm, stat } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { spawnSync } from 'node:child_process';

const workspaceRoot = process.cwd();
const targetTriple = process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET || detectHostTriple();
const isWindowsTarget = targetTriple.includes('windows');
const isLinuxTarget = targetTriple.includes('linux');
const isMacosTarget = targetTriple.includes('apple-darwin') || targetTriple.includes('darwin');

const defaultModel = {
  url: 'https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin',
  path: join(workspaceRoot, 'release', 'default-models', 'ggml-base.bin'),
  sha256: '60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe',
  minBytes: 140 * 1024 * 1024,
};

const ytDlpAsset = (() => {
  if (isWindowsTarget) {
    return {
      url: 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe',
      path: join(workspaceRoot, 'release', 'windows-portable', 'AudraFlow', 'bin', 'yt-dlp.exe'),
    };
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

async function sha256File(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) {
    hash.update(chunk);
  }
  return hash.digest('hex');
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

async function ensureDefaultModel() {
  if (await pathExists(defaultModel.path)) {
    const size = await fileSize(defaultModel.path);
    const hash = await sha256File(defaultModel.path);
    if (size >= defaultModel.minBytes && hash === defaultModel.sha256) {
      console.log(`default model ready: ${defaultModel.path}`);
      return;
    }
    console.log('default model exists but failed validation; downloading a fresh copy');
    await rm(defaultModel.path, { force: true });
  }

  console.log(`downloading default model: ${defaultModel.url}`);
  await downloadFile(defaultModel.url, defaultModel.path);
  const size = await fileSize(defaultModel.path);
  const hash = await sha256File(defaultModel.path);
  if (size < defaultModel.minBytes || hash !== defaultModel.sha256) {
    await rm(defaultModel.path, { force: true });
    throw new Error(`Default model validation failed: size=${size}, sha256=${hash}`);
  }
  console.log(`default model ready: ${defaultModel.path}`);
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

await ensureDefaultModel();
await ensureYtDlp();
