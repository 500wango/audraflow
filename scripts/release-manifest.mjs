import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, readdir, readFile, stat, writeFile } from 'node:fs/promises';
import { basename, join, relative } from 'node:path';

const workspaceRoot = process.cwd();
const rawArgs = process.argv.slice(2);
const args = new Set(rawArgs);
const verify = args.has('--verify');
const requireLinux = args.has('--require-linux');
const requireWindows = args.has('--require-windows');
const requireMacos = args.has('--require-macos');
const artifactSet = argValue('--artifact-set');
const packageJson = JSON.parse(await readFile(join(workspaceRoot, 'package.json'), 'utf8'));
const version = packageJson.version;
const releaseDir = join(workspaceRoot, 'release');
const bundleDir = join(workspaceRoot, 'target', 'release', 'bundle');
const manifestPath = join(
  releaseDir,
  artifactSet
    ? `AudraFlow_${version}_${artifactSet}_manifest.json`
    : `AudraFlow_${version}_manifest.json`,
);
const checksumsPath = join(releaseDir, artifactSet ? `SHA256SUMS.${artifactSet}` : 'SHA256SUMS');

function argValue(name) {
  const index = rawArgs.indexOf(name);
  if (index === -1) {
    return null;
  }
  const value = rawArgs[index + 1];
  if (!value || value.startsWith('--')) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}

function toRelativePath(path) {
  return relative(workspaceRoot, path).replaceAll('\\', '/');
}

function isReleaseArchive(fileName) {
  return fileName.endsWith('.zip') || fileName.endsWith('.tar.gz');
}

function isInstallerArtifact(fileName) {
  return (
    fileName.endsWith('.deb') ||
    fileName.endsWith('.rpm') ||
    fileName.endsWith('.AppImage') ||
    fileName.endsWith('.dmg') ||
    fileName.endsWith('.msi') ||
    fileName.endsWith('.exe')
  );
}

function artifactType(fileName) {
  if (fileName.endsWith('.tar.gz')) return 'portable-tar-gz';
  if (fileName.endsWith('.zip')) return 'portable-zip';
  if (fileName.endsWith('.AppImage')) return 'appimage';
  if (fileName.endsWith('.dmg')) return 'dmg';
  if (fileName.endsWith('.deb')) return 'deb';
  if (fileName.endsWith('.rpm')) return 'rpm';
  if (fileName.endsWith('.msi')) return 'msi';
  if (fileName.endsWith('.exe')) return 'windows-exe';
  return 'unknown';
}

async function fileExists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

async function walkFiles(root) {
  const files = [];
  if (!(await fileExists(root))) {
    return files;
  }
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...await walkFiles(path));
    } else if (entry.isFile()) {
      files.push(path);
    }
  }
  return files;
}

async function sha256File(path) {
  return new Promise((resolve, reject) => {
    const hash = createHash('sha256');
    const stream = createReadStream(path);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('error', reject);
    stream.on('end', () => resolve(hash.digest('hex')));
  });
}

async function collectArtifacts() {
  const artifacts = [];

  if (await fileExists(releaseDir)) {
    const entries = await readdir(releaseDir, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isFile() || !isReleaseArchive(entry.name)) {
        continue;
      }
      artifacts.push(join(releaseDir, entry.name));
    }
  }

  for (const path of await walkFiles(bundleDir)) {
    if (isInstallerArtifact(basename(path))) {
      artifacts.push(path);
    }
  }

  return [...new Set(artifacts)].sort((a, b) => toRelativePath(a).localeCompare(toRelativePath(b)));
}

async function buildManifest() {
  const artifactPaths = await collectArtifacts();
  const artifacts = [];
  for (const path of artifactPaths) {
    const metadata = await stat(path);
    artifacts.push({
      path: toRelativePath(path),
      fileName: basename(path),
      type: artifactType(basename(path)),
      sizeBytes: metadata.size,
      sha256: await sha256File(path),
    });
  }

  return {
    productName: 'AudraFlow',
    version,
    generatedAt: new Date().toISOString(),
    generatedOn: process.platform,
    artifacts,
  };
}

function assertRequiredArtifacts(manifest) {
  const types = new Set(manifest.artifacts.map((artifact) => artifact.type));
  const missing = [];
  if (requireLinux) {
    for (const type of ['deb', 'rpm', 'appimage']) {
      if (!types.has(type)) missing.push(type);
    }
  }
  if (requireWindows) {
    if (!types.has('msi')) missing.push('msi');
    if (!manifest.artifacts.some((artifact) => artifact.path.includes('/nsis/') || artifact.fileName.toLowerCase().includes('setup'))) {
      missing.push('nsis setup exe');
    }
  }
  if (requireMacos) {
    if (!types.has('dmg')) missing.push('dmg');
  }
  if (missing.length > 0) {
    throw new Error(`Missing required release artifact(s): ${missing.join(', ')}`);
  }
}

async function writeManifest() {
  const manifest = await buildManifest();
  assertRequiredArtifacts(manifest);
  await mkdir(releaseDir, { recursive: true });
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  const checksumLines = manifest.artifacts
    .map((artifact) => `${artifact.sha256}  ${artifact.path}`)
    .join('\n');
  await writeFile(checksumsPath, `${checksumLines}\n`);
  console.log(`Wrote ${toRelativePath(manifestPath)} (${manifest.artifacts.length} artifacts)`);
  console.log(`Wrote ${toRelativePath(checksumsPath)}`);
}

async function verifyManifest() {
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  assertRequiredArtifacts(manifest);
  for (const artifact of manifest.artifacts) {
    const path = join(workspaceRoot, artifact.path);
    const metadata = await stat(path);
    if (metadata.size !== artifact.sizeBytes) {
      throw new Error(`Size mismatch for ${artifact.path}: expected ${artifact.sizeBytes}, got ${metadata.size}`);
    }
    const sha256 = await sha256File(path);
    if (sha256 !== artifact.sha256) {
      throw new Error(`SHA256 mismatch for ${artifact.path}: expected ${artifact.sha256}, got ${sha256}`);
    }
  }
  console.log(`Verified ${manifest.artifacts.length} artifacts from ${toRelativePath(manifestPath)}`);
}

if (verify) {
  await verifyManifest();
} else {
  await writeManifest();
}
