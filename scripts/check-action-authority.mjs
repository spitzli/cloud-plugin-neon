import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const manifest = JSON.parse(readFileSync(new URL('../manifest.json', import.meta.url), 'utf8'));
assert.deepEqual(manifest.httpBindings, ['neon']);
for (const action of manifest.actions) {
  assert.ok(!Object.hasOwn(action, 'httpBinding'), `${action.name}: provider actions must not request website HTTP authority`);
  assert.ok(action.capabilities.every(cap => ['plugin.http.read', 'plugin.http.request'].includes(cap)));
}
console.log(`Verified named provider authority for ${manifest.actions.length} released actions`);
