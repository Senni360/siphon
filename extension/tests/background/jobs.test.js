import test from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as wait } from 'node:timers/promises';
import { Jobs } from '../../src/background/jobs.js';
import { DEFAULT_SETTINGS } from '../../src/shared/settings.js';
import { createChromeMock } from '../helpers/chrome-mock.js';

async function setup(options = {}) {
  const chrome = createChromeMock(options);
  const requests = []; const finished = []; const counts = [];
  const native = { request: async (type, payload) => { requests.push({ type, ...payload }); return type === 'cancel' ? { cancelled: true } : { accepted: true }; } };
  const jobs = new Jobs(chrome, { native, getSettings: async () => ({ ...DEFAULT_SETTINGS, ...options.settings }), onTerminal: job => finished.push(job), onCount: count => counts.push(count) });
  await jobs.init();
  return { jobs, chrome, native, requests, finished, counts };
}
test('download returns accepted job, merges defaults, exports cookies without persisting secrets', async () => {
  const { jobs, chrome, requests } = await setup({ cookies: [{ domain: '.youtube.com', name: 'SID', value: 'secret' }] });
  const { jobId } = await jobs.download({ url: 'https://youtu.be/abc', title: 'Song', options: { format: 'flac', trim: { start: 3, end: 9 } } });
  assert.equal(requests[0].jobId, jobId); assert.equal(requests[0].options.quality, '320'); assert.equal(requests[0].options.format, 'flac');
  assert.deepEqual(requests[0].options.trim, { start: 3, end: 9 });
  assert.ok(requests[0].cookies.includes('secret'));
  assert.ok(!JSON.stringify(chrome.storage.session.data).includes('secret'));
  assert.equal(chrome.storage.session.data.jobs[0].submitted, true);
});
test('terminal event moves jobs to deduplicated capped history and retry preserves URL/options', async () => {
  const history = Array.from({ length: 500 }, (_, id) => ({ id: String(id), state: 'done' }));
  const { jobs, chrome, requests, finished } = await setup({ history, settings: { sendSiteCookies: false } });
  const { jobId } = await jobs.download({ url: 'https://a.test/track', options: { format: 'opus', quality: '128' } });
  await jobs.event({ jobId, state: 'error', error: { code: 'NETWORK', message: 'Oops' } });
  await jobs.event({ jobId, state: 'error', error: { code: 'NETWORK', message: 'Duplicate' } });
  assert.equal(jobs.list().length, 0); assert.equal(jobs.history.length, 500); assert.equal(jobs.history[0].id, jobId); assert.equal(finished.length, 1);
  assert.equal(chrome.storage.local.data.history[0].id, jobId);
  const retry = await jobs.retry(jobId); assert.notEqual(retry.jobId, jobId);
  assert.equal(requests[1].url, requests[0].url); assert.deepEqual(requests[1].options, requests[0].options);
  assert.equal(chrome.calls.cookies.length, 0);
});
test('resync restores mirrored jobs and terminates jobs lost with a host process', async () => {
  const { jobs, chrome } = await setup({ jobs: [{ jobId: 'old', url: 'https://a.test/a', state: 'downloading', options: {} }, { jobId: 'lost', url: 'https://a.test/b', state: 'queued' }] });
  await jobs.resync([{ jobId: 'old', state: 'converting', progress: 1 }]);
  assert.equal(jobs.list()[0].state, 'converting'); assert.equal(jobs.list()[0].url, 'https://a.test/a');
  assert.equal(jobs.history[0].id, 'lost'); assert.equal(chrome.storage.session.data.jobs.length, 1);
});
test('cancel terminates a job and history clears selected ids or all', async () => {
  const { jobs } = await setup();
  const { jobId } = await jobs.download({ url: 'https://a.test' });
  await jobs.cancel(jobId);
  assert.equal(jobs.list().length, 1, 'cancel acknowledgement is not a terminal job event');
  await jobs.event({ jobId, state: 'cancelled' });
  assert.equal(jobs.history[0].state, 'cancelled');
  await jobs.clearHistory([jobId]); assert.equal(jobs.history.length, 0);
  jobs.history = [{ id: 'another' }]; await jobs.clearHistory(); assert.equal(jobs.history.length, 0);
  await assert.rejects(jobs.retry('missing'), { code: 'BAD_REQUEST' });
});
test('host missing opens setup, stores friendly error and rejects download', async () => {
  const { jobs, chrome, native } = await setup();
  native.request = async () => { throw Object.assign(new Error('Host not installed'), { code: 'HOST_NOT_FOUND' }); };
  await assert.rejects(jobs.download({ url: 'https://a.test' }), { code: 'HOST_NOT_FOUND' });
  assert.ok(chrome.calls.tabs[0].url.endsWith('src/options/options.html#setup'));
  assert.equal(jobs.history[0].error.code, 'HOST_NOT_FOUND'); assert.equal(jobs.list().length, 0);
});
test('job broadcasts coalesce progress across multiple jobs to at most four per second', async () => {
  const { jobs, chrome } = await setup();
  for (let index = 0; index < 30; index++) await jobs.event({ jobId: 'a', state: 'downloading', progress: index / 30 });
  assert.equal(chrome.calls.broadcasts.filter(message => message.evt === 'jobs').length, 1);
  await wait(280);
  const broadcasts = chrome.calls.broadcasts.filter(message => message.evt === 'jobs');
  assert.equal(broadcasts.length, 2); assert.equal(broadcasts[1].jobs[0].progress, 29 / 30);
});
