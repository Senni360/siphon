import test from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as wait } from 'node:timers/promises';
import { NativeManager } from '../../src/background/native.js';
import { createChromeMock } from '../helpers/chrome-mock.js';

function manager(chrome, extra = {}) { return new NativeManager(chrome, { getSettings: async () => ({ maxConcurrent: 4 }), ...extra }); }
test('native manager is lazy, handshakes once, resyncs and correlates out-of-order UUID replies', async () => {
  const chrome = createChromeMock();
  let resynced;
  const native = manager(chrome, { onResync: jobs => { resynced = jobs; } });
  assert.equal(chrome.calls.ports.length, 0);
  await Promise.all([native.connect(), native.connect()]);
  assert.equal(chrome.calls.ports.length, 1);
  assert.deepEqual(resynced, []);
  assert.equal(chrome.calls.native[0].maxConcurrent, 4);
  assert.equal(chrome.calls.native[0].extVersion, '1.0.0');
  chrome.nativeHandler = null;
  const first = native.request('probe', { url: 'https://a.test' });
  const second = native.request('open', { path: 'a' });
  await wait(0);
  const port = chrome.calls.ports[0];
  const [a, b] = port.sent.slice(-2);
  assert.notEqual(a.id, b.id);
  port.reply(b, { second: true }); port.reply(a, { first: true });
  assert.deepEqual(await first, { first: true }); assert.deepEqual(await second, { second: true });
  assert.equal(native.pending.size, 0); native.disconnect();
});
test('missing host reads runtime.lastError during disconnect and rejects waiting callers', async () => {
  const chrome = createChromeMock(); chrome.nativeError = 'Specified native messaging host not found.';
  const native = manager(chrome);
  await assert.rejects(native.connect(), { code: 'HOST_NOT_FOUND' });
  assert.equal(native.status.state, 'missing'); assert.equal(native.reconnectTimer, undefined);
});
test('probe and normal request timeouts are distinct and release pending map', async () => {
  const chrome = createChromeMock(); const native = manager(chrome, { timeoutMs: 10, probeTimeoutMs: 35 });
  await native.connect(); chrome.nativeHandler = null;
  const normal = assert.rejects(native.request('open'), { code: 'NETWORK' });
  const probe = assert.rejects(native.request('probe'), { code: 'NETWORK' });
  await wait(20); assert.equal(native.pending.size, 1);
  await wait(30); await Promise.all([normal, probe]); assert.equal(native.pending.size, 0); native.disconnect();
});
test('disconnect reconnects with active demand, then stops without jobs or UI', async () => {
  const chrome = createChromeMock(); const native = manager(chrome, { retryMs: 10 });
  await native.connect(); native.setActiveJobs(1);
  chrome.calls.ports[0].fail('Native host has exited.');
  assert.equal(native.status.state, 'error');
  await wait(30); assert.equal(chrome.calls.ports.length, 2); assert.equal(native.status.state, 'ready');
  native.setActiveJobs(0); native.disconnect();
  await wait(30); assert.equal(chrome.calls.ports.length, 2);
});
test('idle disconnect leaves active job ports alive', async () => {
  const chrome = createChromeMock(); const native = manager(chrome, { idleMs: 15 });
  await native.connect(); native.setActiveJobs(1); await wait(25); assert.ok(native.port);
  native.setActiveJobs(0); await wait(25); assert.equal(native.port, null);
});
test('resync callbacks can send native requests without waiting on their own handshake', async () => {
  const chrome = createChromeMock();
  let native;
  native = manager(chrome, { onResync: async () => native.request('reveal', { path: 'C:\\Music\\song.mp3' }) });
  await native.connect();
  assert.equal(chrome.calls.native.at(-1).type, 'reveal');
  native.disconnect();
});
test('reconnect uses actual open extension UI contexts, and stops when UI closes', async () => {
  const chrome = createChromeMock();
  let contexts = [{ documentUrl: chrome.runtime.getURL('src/options/options.html') }];
  chrome.runtime.getContexts = async () => contexts;
  const native = manager(chrome, { retryMs: 10 });
  await native.connect();
  chrome.calls.ports[0].fail('Native host has exited.');
  await wait(30);
  assert.equal(chrome.calls.ports.length, 2);
  contexts = [];
  native.touchUI(); // A closed context takes precedence over the fallback lease.
  chrome.calls.ports[1].fail('Native host has exited.');
  await wait(30);
  assert.equal(chrome.calls.ports.length, 2);
  native.disconnect();
});
