import { createWriteStream } from 'node:fs';
import { chmod, mkdir, readFile, rename, rm, stat } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { spawnSync } from 'node:child_process';

const workspaceRoot = process.cwd();
const targetTriple = process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET || detectHostTriple();
const isWindowsTarget = targetTriple.includes('windows');
const isLinuxTarget = targetTriple.includes('linux');
const isMacosTarget = targetTriple.includes('apple-darwin') || targetTriple.includes('darwin');

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

await ensureYtDlp();
