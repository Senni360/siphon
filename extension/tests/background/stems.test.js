import { stemPathKey } from '../../src/shared/stems.js';
import test from 'node:test';
import assert from 'node:assert/strict';
import { Stems } from '../../src/background/stems.js';
import { Jobs } from '../../src/background/jobs.js';
import { NativeManager } from '../../src/background/native.js';
import { createRouter } from '../../src/background/router.js';
import { CMD } from '../../src/shared/protocol.js';
import { createChromeMock } from '../helpers/chrome-mock.js';

async function fixture(capable = true) {
  const history = [{ id: 'source', state: 'done', file: 'C:\\Music\\track.wav', title: 'Track' }];
  const chrome = createChromeMock({ history, nativeHandler(request, port) {
    const data = request.type === 'hello' ? { stems: capable, dropzone: true } : ['listJobs', 'listStems'].includes(request.type) ? { jobs: [] } : request.type === 'splitStems' ? { jobId: request.jobId, accepted: true } : request.type === 'cancelStems' ? { jobId: request.jobId, cancelled: true } : {};
    port.reply(request, data);
  } });
  const counts = [];
  let stems;
  const getSettings = async () => ({ maxConcurrent: 2 });
  const native = new NativeManager(chrome, { getSettings, onStem: event => stems.event(event), onStemResync: snapshot => stems.resync(snapshot) });
  const jobs = new Jobs(chrome, { native, getSettings }); await jobs.init();
  stems = new Stems(chrome, { native, jobs, onCount: count => { counts.push(count); native.setActiveJobs(count); } }); await stems.init();
  const { route } = createRouter({ chrome, ready: Promise.resolve(), native, jobs, stems, getSettings });
  return { chrome, native, stems, jobs, counts, route };
}

test('stem routing gates capability and never sends listStems to older hosts', async () => {
  const f = await fixture(false);
  try {
    await assert.rejects(f.route({ cmd: CMD.SPLIT_STEMS, id: 'source' }), /newer Siphon helper/);
    assert.equal(f.chrome.calls.native.some(request => request.type === 'listStems'), false);
    assert.deepEqual(f.stems.list(), []);
  } finally { f.native.disconnect(); }
});
test('split resolves latest history path, rejects pending analysis, deduplicates and preserves downloads', async () => {
  const f = await fixture();
  try {
    f.jobs.history[0].analysisPending = 1;
    await assert.rejects(f.stems.split('source'), /analysis/);
    await f.jobs.analysis({ jobId: 'source', source: 'C:\\Music\\track.wav', file: 'C:\\Music\\track [128 BPM].wav', pending: 0 });
    const [a, b] = await Promise.all([f.route({ cmd: CMD.SPLIT_STEMS, id: 'source' }), f.stems.split('source')]);
    assert.equal(a.jobId, b.jobId);
    const request = f.chrome.calls.native.find(request => request.type === 'splitStems');
    assert.equal(request.path, f.jobs.history[0].file);
    assert.equal(f.jobs.history.length, 1);
    assert.equal(f.jobs.list().length, 0);
    assert.equal(f.native.activeJobs, 1);
    const state = await f.route({ cmd: CMD.GET_STATE });
    assert.equal(state.stems[0].sourceId, 'source');
  } finally { f.native.disconnect(); }
});
test('stem events stay separate, cancel waits for termination, late progress cannot resurrect jobs', async () => {
  const f = await fixture();
  try {
    const { jobId } = await f.stems.split('source');
    await f.route({ cmd: CMD.CANCEL_STEMS, jobId });
    assert.equal(f.native.activeJobs, 1);
    await f.stems.event({ type: 'stem', jobId, state: 'cancelled', updatedAt: Date.now() });
    await f.stems.event({ type: 'stem', jobId, state: 'running', stage: 'separating', updatedAt: Date.now() + 10 });
    assert.equal(f.stems.list()[0].state, 'cancelled');
    assert.equal(f.native.activeJobs, 0);
    assert.equal(f.jobs.history[0].state, 'done');
    assert.notEqual((await f.stems.split('source')).jobId, jobId);
  } finally { f.native.disconnect(); }
});
test('persisted active stems recover mappings and missing jobs terminate after helper restart', async () => {
  const f = await fixture();
  try {
    const { jobId } = await f.stems.split('source');
    const recovered = new Stems(f.chrome, { native: f.native, jobs: f.jobs });
    await recovered.init();
    assert.equal(recovered.list()[0].sourceId, 'source');
    await recovered.resync([{ jobId, state: 'running', stage: 'setup', updatedAt: Date.now() }]);
    assert.equal(recovered.list()[0].sourceId, 'source');
    await recovered.resync([]);
    assert.equal(recovered.list()[0].error.code, 'STEMS_INTERRUPTED');
    await f.native.reconnect();
    assert.equal(f.native.activeJobs, 0);
    assert.equal(f.stems.list()[0].state, 'error');
  } finally { f.native.disconnect(); }
});
test('native stem events are routed and all four completed files enter one pinned drag card', async () => {
  const f = await fixture();
  try {
    const { jobId } = await f.stems.split('source');
    const files = ['vocals', 'drums', 'bass', 'other'].map(name => `C:\\Music\\stems\\${name}.wav`);
    f.native.receive({ type: 'stem', jobId, state: 'done', files, outputDir: 'C:\\Music\\stems', updatedAt: Date.now() });
    await f.stems.writes;
    await f.route({ cmd: CMD.SHOW_STEMS, jobId });
    const request = f.chrome.calls.native.at(-1);
    assert.equal(request.type, 'showDrop'); assert.equal(request.pinned, true);
    assert.deepEqual(request.items[0].files, files);
    assert.equal(f.native.activeJobs, 0);
  } finally { f.native.disconnect(); }
});

test('ISO host timestamps sort retry attempts correctly and reject stale snapshots', async () => {
  const f = await fixture();
  try {
    await f.native.connect();
    const now = Date.now();
    await f.stems.event({ jobId: 'iso', sourceFile: f.jobs.history[0].file, state: 'running', stage: 'separating', createdAt: new Date(now - 100).toISOString(), updatedAt: new Date(now).toISOString() });
    await f.stems.event({ jobId: 'iso', state: 'queued', updatedAt: new Date(now - 50).toISOString() });
    assert.equal(f.stems.list()[0].state, 'running');
    assert.equal(f.stems.list()[0].createdAt, now - 100);
    assert.equal(f.stems.list()[0].sourceId, 'source');
  } finally { f.native.disconnect(); }
});

test('lost submission acknowledgement queries the same host instead of killing unrelated work', async () => {
  const f = await fixture();
  try {
    await f.native.connect();
    let accepted;
    f.native.timeoutMs = 10;
    f.chrome.nativeHandler = (request, port) => {
      if (request.type === 'splitStems') { accepted = request.jobId; return; }
      port.reply(request, { jobs: [{ jobId: accepted, state: 'running', stage: 'separating', updatedAt: new Date().toISOString() }] });
    };
    // Hold a regular timer: production timeout timers intentionally do not pin Node.
    const keepAlive = setTimeout(() => {}, 1000);
    try { await assert.rejects(f.stems.split('source'), { code: 'NETWORK' }); }
    finally { clearTimeout(keepAlive); }
    assert.equal(f.chrome.calls.ports.length, 1);
    assert.equal(f.native.activeJobs, 1);
    assert.equal(f.stems.list()[0].state, 'running');
  } finally { f.native.disconnect(); }
});


test('canonical Windows host paths associate and deduplicate concurrent submissions', async () => {
  const f = await fixture();
  try {
    await f.native.connect();
    await f.stems.event({ jobId: 'host-only', sourceFile: '\\\\?\\c:\\MUSIC\\TRACK.wav', state: 'running', createdAt: new Date().toISOString() });
    assert.equal(f.stems.list()[0].sourceId, 'source');
    const replies = await Promise.all(Array.from({ length: 8 }, () => f.stems.split('source', 'C:/music/Track.wav')));
    assert.ok(replies.every(reply => reply.jobId === 'host-only'));
    assert.equal(f.chrome.calls.native.filter(r => r.type === 'splitStems').length, 0);
    await f.stems.event({ jobId: 'host-only', state: 'cancelled' });
    const starts = await Promise.all(Array.from({ length: 8 }, () => f.stems.split('source')));
    assert.equal(new Set(starts.map(r => r.jobId)).size, 1);
    assert.equal(f.chrome.calls.native.filter(r => r.type === 'splitStems').length, 1);
  } finally { f.native.disconnect(); }
});
test('playlist selections require current membership after analysis renames', async () => {
  const f = await fixture();
  try {
    const item = f.jobs.history[0];
    item.files = ['C:\\Music\\first.wav', item.file];
    await assert.rejects(f.stems.split('source'), /Choose a track/);
    await assert.rejects(f.route({ cmd: CMD.SPLIT_STEMS, id: 'source', path: 'C:\\wrong.wav' }), /no longer/);
    item.analysisPending = 1;
    await assert.rejects(f.stems.split('source', item.files[0]), /analysis/);
    const old = item.files[0]; item.files[0] = 'C:\\Music\\first [128 BPM].wav'; item.analysisPending = 0;
    await assert.rejects(f.stems.split('source', old), /no longer/);
    await f.route({ cmd: CMD.SPLIT_STEMS, id: 'source', path: item.files[0].toLowerCase().replaceAll('\\', '/') });
    assert.equal(f.chrome.calls.native.at(-1).path, item.files[0]);
  } finally { f.native.disconnect(); }
});
test('removed history retains persisted active jobs and cancellation releases native liveness only on termination', async () => {
  const f = await fixture();
  try {
    const { jobId } = await f.stems.split('source');
    f.jobs.history = [];
    const restored = new Stems(f.chrome, { native: f.native, jobs: f.jobs }); await restored.init();
    assert.equal(restored.list()[0].jobId, jobId);
    assert.equal((await f.route({ cmd: CMD.GET_STATE })).stems[0].jobId, jobId);
    await f.route({ cmd: CMD.CANCEL_STEMS, jobId });
    assert.equal(f.native.activeJobs, 1);
    await f.stems.event({ jobId, state: 'cancelled', updatedAt: Date.now() });
    assert.equal(f.native.activeJobs, 0);
  } finally { f.native.disconnect(); }
});


test('Windows path keys normalize extended UNC names while POSIX paths retain case', () => {
  assert.equal(stemPathKey('\\\\?\\UNC\\Server\\Share\\Track.wav'), stemPathKey('//server/share/track.wav'));
  assert.equal(stemPathKey('\\\\?\\C:\\Music\\last.wav'), stemPathKey('c:/music/LAST.wav'));
  assert.notEqual(stemPathKey('/Music/Track.wav'), stemPathKey('/music/track.wav'));
});
