import { spawnSync } from 'node:child_process';

if (process.env.AUDRAFLOW_SKIP_BEFORE_BUILD === '1') {
  console.log('Skipping Tauri beforeBuildCommand because AUDRAFLOW_SKIP_BEFORE_BUILD=1.');
  process.exit(0);
}

const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const targetTriple = process.env.AUDRAFLOW_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET || '';
const isWindowsTarget = process.platform === 'win32' || targetTriple.includes('windows');
const steps = [
  ['build', ['run', 'build']],
  ['prepare runtime assets', ['run', 'prepare:runtime-assets']],
];

if (isWindowsTarget) {
  steps.push(['prepare Windows runtime', ['run', 'prepare:windows-runtime']]);
}

steps.push(['stage sidecars', ['run', 'stage:sidecars']]);

for (const [label, args] of steps) {
  console.log(`\n== Tauri beforeBuild: ${label}`);
  const result = spawnSync(npmCommand, args, {
    stdio: 'inherit',
    env: process.env,
  });

  if (result.error) {
    console.error(`Failed to start ${npmCommand} ${args.join(' ')}: ${result.error.message}`);
    process.exit(1);
  }

  if (result.status !== 0) {
    console.error(`${npmCommand} ${args.join(' ')} failed with exit code ${result.status}.`);
    process.exit(result.status ?? 1);
  }
}
