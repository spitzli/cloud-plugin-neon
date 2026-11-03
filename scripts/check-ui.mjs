#!/usr/bin/env node
// The UI is a host ABI peer, so type-check against the real installed Console SDK.
import { mkdtemp, writeFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, dirname } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
const host = process.env.WEBDOCK_CONSOLE_ROOT;
if (!host) throw Error('Set WEBDOCK_CONSOLE_ROOT to a checked-out apps/console with its locked dependencies installed.');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const consoleRoot = resolve(host);
const require = createRequire(resolve(consoleRoot, 'package.json'));
await access(resolve(consoleRoot, 'src/plugins/sdk.ts'));
const typescript = require.resolve('typescript/bin/tsc');
const reactTypes = dirname(require.resolve('@types/react/package.json'));
const zod = dirname(require.resolve('zod/package.json'));
const vite = dirname(require.resolve('vite/package.json'));
const nodeTypes = dirname(require.resolve('@types/node/package.json'));
const temporary = await mkdtemp(resolve(tmpdir(), 'neon-ui-abi-'));
try {
  const config = resolve(temporary, 'tsconfig.json');
  await writeFile(config, JSON.stringify({
    extends: resolve(consoleRoot, 'tsconfig.json'),
    compilerOptions: { noEmit: true, baseUrl: consoleRoot, types: [resolve(vite, 'client.d.ts'), resolve(nodeTypes, 'index.d.ts')], paths: {
      '@webdock/plugin-ui': [resolve(consoleRoot, 'src/plugins/sdk.ts')],
      '@plugins/*': [resolve(consoleRoot, '../../plugins/*')],
      react: [resolve(reactTypes, 'index.d.ts')],
      'react/jsx-runtime': [resolve(reactTypes, 'jsx-runtime.d.ts')],
      zod: [resolve(zod, 'index.d.ts')],
    } },
    include: [resolve(root, 'ui/index.tsx'), resolve(root, 'ui/nav.ts')],
    exclude: [],
  }));
  const result = spawnSync(process.execPath, [typescript, '--project', config], { stdio: 'inherit', cwd: consoleRoot });
  if (result.status !== 0) process.exitCode = 1;
} finally { await rm(temporary, { recursive: true, force: true }); }
