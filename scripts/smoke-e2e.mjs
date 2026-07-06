import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const forwardedArgs = process.argv.slice(2);
const windowsArgMap = new Map([
  ['--audio-path', '-AudioPath'],
  ['--model-path', '-ModelPath'],
  ['--timeout-seconds', '-TimeoutSeconds'],
  ['--skip-build', '-SkipBuild'],
  ['--keep-record', '-KeepRecord'],
]);
const platformArgs = process.platform === 'win32'
  ? forwardedArgs.map((arg) => windowsArgMap.get(arg) ?? arg)
  : forwardedArgs;

const command = process.platform === 'win32' ? 'powershell' : 'bash';
const args = process.platform === 'win32'
  ? [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      join(scriptDir, 'smoke-e2e.ps1'),
      ...platformArgs,
    ]
  : [join(scriptDir, 'smoke-e2e.sh'), ...platformArgs];

const result = spawnSync(command, args, { stdio: 'inherit' });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);
