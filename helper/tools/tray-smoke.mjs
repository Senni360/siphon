// Real process/pipe/lifetime checks using an isolated tray identity. No clicks
// or browser automation. Briefly shows one fixture card without taking focus.
// Run after cargo build --release: node helper/tools/tray-smoke.mjs [tray.exe]
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';

const root = await mkdtemp(join(tmpdir(), 'siphon-tray-smoke-'));
const binary = resolve(process.argv[2] || 'helper/target/release/siphon-tray.exe');
const env = { ...process.env, USERNAME: `siphon-test-${randomUUID()}`, LOCALAPPDATA: root };
const children = new Set();
function launch(payload) {
  const child = spawn(binary, [], { env, stdio: ['pipe', 'ignore', 'pipe'], windowsHide: true });
  children.add(child);
  child.done = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => { children.delete(child); resolve(code); });
  });
  child.stdin.on('error', () => {});
  child.stdin.end(JSON.stringify(payload));
  return child;
}
async function finished(child, timeout = 5000) {
  let timer;
  try {
    return await Promise.race([child.done, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Tray ${child.pid} did not exit in ${timeout}ms`)), timeout);
    })]);
  } finally { clearTimeout(timer); }
}
try {
  const invalid = launch({ items: [{ files: [join(root, 'missing.wav')] }] });
  assert.equal(await finished(invalid), 0);
  console.log('PASS: vanished file does not leave a hidden one-shot process');

  const file = join(root, 'fixture.wav');
  await writeFile(file, 'fixture');
  const item = { files: [file], title: 'Siphon Rust tray test', subtitle: 'Temporary fixture' };
  const started = performance.now();
  const single = launch({ items: [item] });
  assert.equal(await finished(single, 20000), 0);
  const lifetime = performance.now() - started;
  assert(lifetime >= 6500 && lifetime < 15000, `Unexpected card lifetime: ${lifetime}ms`);
  console.log(`PASS: one-shot card expires and process exits (${Math.round(lifetime)}ms)`);

  const resident = launch({ resident: true, hostPid: process.pid });
  await delay(200);
  for (let i = 0; i < 20; i++) {
    assert.equal(await finished(launch({ resident: true, hostPid: process.pid })), 0);
  }
  assert.equal(resident.exitCode, null);
  const log = await readFile(join(root, 'Siphon/logs/tray.log'), 'utf8');
  assert.equal((log.match(/tray started \(resident=true\)/g) || []).length, 1);
  console.log('PASS: 20 handoffs reuse one resident process');
  // The existing process adopts the latest host PID. PID zero cannot be a
  // user-mode helper, so the watchdog must retire this empty resident.
  assert.equal(await finished(launch({ resident: true, hostPid: 0 })), 0);
  assert.equal(await finished(resident, 7000), 0);
  console.log('PASS: resident exits when its helper is gone');
} finally {
  for (const child of children) child.kill();
  await Promise.allSettled([...children].map(child => child.done));
  await rm(root, { recursive: true, force: true });
}
