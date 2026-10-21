import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { setTimeout as wait } from 'node:timers/promises';
import vm from 'node:vm';
import { createChromeMock, createEvent } from '../helpers/chrome-mock.js';
import { CMD, ERR } from '../../src/shared/protocol.js';
import { DEFAULT_SETTINGS, getSettings, setSettings, toDownloadOptions } from '../../src/shared/settings.js';
import { ERROR_MESSAGES, friendlyError } from '../../src/shared/errors.js';
import { NativeManager } from '../../src/background/native.js';
import { Jobs } from '../../src/background/jobs.js';
import { createRouter } from '../../src/background/router.js';

const hello = { hostVersion: '0.1.0', ytdlpVersion: '2026.08.19', ffmpeg: { found: true, version: '8.1', path: 'C:\\ffmpeg.exe' }, jsRuntime: { name: 'node', version: 'v24.0.0' }, defaultOutputDir: 'C:\\Music\\Siphon', platform: 'win32' };
const media = { url: 'https://www.youtube.com/watch?v=abcdefghijk&list=PL123', webpageUrl: 'https://www.youtube.com/playlist?list=PL123', extractor: 'youtube:tab', id: 'PL123', title: 'Album', uploader: 'Artist', duration: null, thumbnail: null, isPlaylist: true, entries: [{ url: 'https://www.youtube.com/watch?v=abcdefghijk', title: 'Song', duration: 60 }], entryCount: 1, isLive: false, formatsAudio: [] };
const hostJob = (jobId, state = 'downloading') => ({ type: 'job', jobId, url: media.url, source: 'youtube', options: toDownloadOptions(DEFAULT_SETTINGS, { format: 'opus' }), createdAt: '2026-10-17T12:00:00+00:00', updatedAt: '2026-10-17T12:00:01+00:00', state, stage: state, progress: .4, speed: 123456, eta: 5, downloadedBytes: 400, totalBytes: 1000, item: { index: 1, count: 3, title: 'Song' }, file: null, files: null, error: null, title: 'Song', thumbnail: 'https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg' });

async function integration(extra = {}) {
  const settings = { ...DEFAULT_SETTINGS, sendSiteCookies: false, ...extra.settings };
  const chrome = createChromeMock({ tabs: [{ id: 1, active: true, url: media.url, title: 'Page' }], nativeHandler: (request, port) => {
    const data = request.type === 'hello' ? hello : request.type === 'listJobs' ? { jobs: extra.jobs || [] } : request.type === 'probe' ? media : request.type === 'download' ? { jobId: request.jobId, accepted: true } : request.type === 'cancel' ? { jobId: request.jobId, cancelled: true } : request.type === 'update' ? { before: 'old', after: 'new', restartRequired: true, ejsVersion: '1' } : {};
    port.reply(request, data);
  } });
  let jobs;
  const native = new NativeManager(chrome, { getSettings: async () => settings, onJob: event => jobs.event(event), onResync: snapshot => jobs.resync(snapshot), ...extra.native });
  jobs = new Jobs(chrome, { native, getSettings: async () => settings, onCount: count => native.setActiveJobs(count) });
  const ready = jobs.init();
  const { route } = createRouter({ chrome, ready, native, jobs, getSettings: async () => settings, setSettings: async patch => Object.assign(settings, patch) });
  await ready;
  return { chrome, native, jobs, route, settings };
}

test('router preserves playlist preview and host/default/history shapes consumed by the UIs', async () => {
  const { chrome, native, jobs, route } = await integration();
  try {
    const state = await route({ cmd: CMD.GET_STATE });
    assert.equal(state.host.defaultOutputDir, hello.defaultOutputDir);
    const probe = await route({ cmd: CMD.PROBE_TAB });
    assert.deepEqual(probe, { url: media.url, info: media, error: null });
    assert.equal(chrome.calls.native.findLast(request => request.type === 'probe').flatPlaylist, true);
    const { jobId } = await route({ cmd: CMD.DOWNLOAD, url: media.url, title: 'Fallback title', options: { format: 'opus', quality: '128' } });
    const request = chrome.calls.native.findLast(request => request.type === 'download');
    assert.equal(request.jobId, jobId);
    assert.equal(request.options.quality, '128');
    assert.ok(!('cookies' in request));
    await jobs.event({ type: 'job', jobId, state: 'queued', title: '' });
    assert.equal(jobs.list()[0].title, 'Fallback title');
    await jobs.event({ ...hostJob(jobId), options: request.options });
    const queue = (await route({ cmd: CMD.GET_STATE })).jobs[0];
    for (const key of ['stage', 'progress', 'speed', 'eta', 'item', 'jobId']) assert.ok(key in queue, key);
    await jobs.event({ ...hostJob(jobId, 'done'), options: request.options, progress: 1, file: 'C:\\Music\\song.opus', files: ['C:\\Music\\song.opus'] });
    const history = (await route({ cmd: CMD.GET_STATE })).history[0];
    for (const key of ['id', 'url', 'title', 'thumbnail', 'file', 'files', 'format', 'source', 'options', 'finishedAt']) assert.ok(key in history, key);
    assert.equal(history.format, 'opus');
    assert.equal(history.id, jobId);
    assert.equal((await route({ cmd: CMD.RETRY, jobId })).jobId !== jobId, true);
  } finally { native.disconnect(); }
});

test('single-tab download uses playing media; collection preview and whole-playlist download keep page context', async () => {
  const { chrome, native, route } = await integration();
  chrome.tabMessageHandler = () => ({ url: 'https://www.youtube.com/watch?v=playing1234', title: 'Playing song' });
  try {
    await route({ cmd: CMD.DOWNLOAD_TAB });
    assert.equal(chrome.calls.native.findLast(request => request.type === 'download').url, 'https://www.youtube.com/watch?v=playing1234');
    await route({ cmd: CMD.DOWNLOAD_TAB, options: { playlist: 'all' } });
    assert.equal(chrome.calls.native.findLast(request => request.type === 'download').url, media.url);
    chrome.tabs.items[0].url = 'https://music.youtube.com/watch?v=page1234567';
    assert.equal((await route({ cmd: CMD.PROBE_TAB })).url, 'https://www.youtube.com/watch?v=playing1234');
  } finally { native.disconnect(); }
});

test('download overrides never persist cookies or extension-only settings', () => {
  const options = toDownloadOptions(DEFAULT_SETTINGS, { format: 'ogg', quality: '128', trim: { start: 1, end: 3 }, cookies: 'secret', sendSiteCookies: true });
  assert.equal(options.format, 'ogg');
  assert.equal(options.quality, '128');
  assert.deepEqual(options.trim, { start: 1, end: 3 });
  assert.ok(!('cookies' in options));
  assert.ok(!('sendSiteCookies' in options));
});

test('listJobs restores host fields and cancellation waits for the authoritative event', async () => {
  const { native, jobs, route } = await integration({ jobs: [hostJob('restored')] });
  try {
    await route({ cmd: CMD.GET_STATE });
    assert.equal(jobs.list()[0].item.count, 3);
    assert.equal((await route({ cmd: CMD.CANCEL, jobId: 'restored' })).cancelled, true);
    assert.equal(native.activeJobs, 1);
    assert.equal(jobs.history.length, 0);
    await jobs.event({ ...hostJob('restored', 'cancelled'), error: null });
    assert.equal(native.activeJobs, 0);
    assert.equal(jobs.history[0].state, 'cancelled');
  } finally { native.disconnect(); }
});

test('host-only resynced jobs become retryable errors if the next host process loses them', async () => {
  const { native, jobs } = await integration({ jobs: [hostJob('host-only')] });
  try {
    await native.connect();
    assert.equal(jobs.list()[0].submitted, true);
    await jobs.resync([]);
    assert.equal(jobs.list().length, 0);
    assert.equal(jobs.history[0].id, 'host-only');
    assert.equal(jobs.history[0].error.code, 'INTERNAL');
  } finally { native.disconnect(); }
});

test('maxConcurrent reconfigures the same port; updater restarts only when active jobs finish', async () => {
  const { chrome, native, jobs, route } = await integration();
  try {
    const { jobId } = await route({ cmd: CMD.DOWNLOAD, url: media.url });
    const port = native.port;
    await native.configure({ maxConcurrent: 16 });
    assert.equal(chrome.calls.native.at(-1).maxConcurrent, 16);
    assert.equal(native.port, port);
    assert.equal((await route({ cmd: CMD.UPDATE_HOST })).restartRequired, true);
    assert.equal(native.status.restartRequired, true);
    assert.equal(port.disconnected, false);
    await jobs.event({ jobId, state: 'done', file: 'C:\\song.mp3', files: ['C:\\song.mp3'] });
    await wait(0);
    assert.equal(port.disconnected, true);
    assert.equal(chrome.calls.ports.length, 2);
    assert.equal(native.status.state, 'ready');
    assert.equal(native.restartRequired, false);
  } finally { native.disconnect(); }
});

test('folder picker and pip update outlive the ordinary native request timeout', async () => {
  const { chrome, native } = await integration({ native: { timeoutMs: 5, pickerTimeoutMs: 100, updateTimeoutMs: 100 } });
  try {
    await native.connect();
    chrome.nativeHandler = (request, port) => { setTimeout(() => port.reply(request, { path: null }), 25); };
    assert.deepEqual(await native.request('pickFolder'), { path: null });
    assert.deepEqual(await native.request('update'), { path: null });
  } finally { native.disconnect(); }
});

test('settings writes preserve concurrent patches and independent per-site toggles', async () => {
  const previousChrome = globalThis.chrome;
  globalThis.chrome = createChromeMock({ settings: { siteButtons: { youtube: false } } });
  try {
    await Promise.all([setSettings({ format: 'ogg' }), setSettings({ theme: 'light' }), setSettings({ siteButtons: { bandcamp: false } })]);
    const settings = await getSettings();
    assert.equal(settings.format, 'ogg');
    assert.equal(settings.theme, 'light');
    assert.deepEqual(settings.siteButtons, { youtube: false, youtubeMusic: true, soundcloud: true, bandcamp: false });
  } finally { globalThis.chrome = previousChrome; }
});

test('every protocol error has friendly advice shared by notification, popup, and options', () => {
  assert.deepEqual(Object.keys(ERROR_MESSAGES).sort(), Object.values(ERR).sort());
  for (const code of Object.values(ERR)) assert.ok(friendlyError({ code }).length > 20, code);
  for (const module of ['background/notify', 'popup/popup', 'options/options']) {
    assert.match(readFileSync(new URL(`../../src/${module}.js`, import.meta.url), 'utf8'), /shared\/errors\.js/);
  }
});

test('manifest files, ordered classic scripts, page modules, and CSP-sensitive sources are valid', () => {
  const root = new URL('../../', import.meta.url);
  const manifest = JSON.parse(readFileSync(new URL('manifest.json', root), 'utf8'));
  assert.ok(manifest.permissions.includes('cookies'));
  assert.ok(manifest.host_permissions.includes('<all_urls>'));
  assert.equal(manifest.background.type, 'module');
  const files = [manifest.background.service_worker, manifest.action.default_popup, manifest.options_page, ...Object.values(manifest.icons)];
  for (const script of manifest.content_scripts) {
    assert.deepEqual(script.js.slice(0, 2), ['src/content/media.js', 'src/content/core.js']);
    files.push(...script.js);
  }
  for (const file of files) assert.ok(existsSync(new URL(file, root)), file);
  for (const page of [manifest.action.default_popup, manifest.options_page]) {
    const html = readFileSync(new URL(page, root), 'utf8');
    assert.match(html, /<script type="module" src="[^\"]+"><\/script>/);
    assert.doesNotMatch(html, /\son\w+\s*=|javascript:|<script(?![^>]*\bsrc=)[^>]*>[^<]+/i);
    for (const [, path] of html.matchAll(/(?:src|href)="([^"#]+)"/g)) assert.ok(existsSync(new URL(path, new URL(page, root))), path);
  }
});

// Minimal isolated-world DOM: runtime broadcasts deliberately do not reach it.
// This catches both the Chrome transport seam and the actual button consumer.
function contentDOM(chrome) {
  class Element {
    constructor() { this.children = []; this.dataset = {}; this.style = { setProperty(name, value) { this[name] = value; } }; this.listeners = {}; this.isConnected = true; }
    get firstChild() { return this.children[0] || null; }
    append(...children) { for (const child of children) { child.parentElement = this; this.children.push(child); } }
    attachShadow() { return this.shadowRoot = new Element(); }
    setAttribute() {}
    addEventListener(name, listener) { this.listeners[name] = listener; }
    remove() { this.isConnected = false; }
    querySelector() { return null; }
  }
  const mount = new Element(), contentMessages = createEvent();
  const document = { title: 'Song', documentElement: new Element(), createElement: () => new Element(), createElementNS: () => new Element(), addEventListener() {} };
  const context = vm.createContext({ document, URL, location: new URL(media.url), window: { addEventListener() {} }, MutationObserver: class { observe() {} }, setTimeout: callback => { queueMicrotask(callback); return 1; }, chrome: { storage: chrome.storage, runtime: { onMessage: contentMessages, sendMessage: message => chrome.runtime.sendMessage(message) } } });
  vm.runInContext(readFileSync(new URL('../../src/content/core.js', import.meta.url), 'utf8'), context);
  context.SiphonContent.start({ site: 'youtube', dark: () => true, current: () => media, collect: () => [{ mount, key: 'watch', resolve: () => ({ url: media.url, title: 'Song' }) }] });
  chrome.tabMessageHandler = (_, message) => { contentMessages.emit(message, {}, () => {}); };
  return mount;
}

test('real content control receives host progress and terminal states via tabs.sendMessage', async () => {
  const { chrome, native, jobs } = await integration();
  const mount = contentDOM(chrome);
  try {
    await wait(0);
    const host = mount.children[0], main = host.shadowRoot.children[1].children[0];
    main.listeners.click({ preventDefault() {}, stopPropagation() {} });
    await wait(0);
    const jobId = chrome.calls.native.findLast(request => request.type === 'download').jobId;
    native.port.emit(hostJob(jobId));
    await wait(280);
    assert.equal(host.dataset.state, 'downloading');
    assert.equal(main.children[1].textContent, '40%');
    assert.equal(main.disabled, true);
    await jobs.event({ ...hostJob(jobId, 'analyzing'), stage: 'analyzing' });
    await wait(280);
    assert.equal(main.children[1].textContent, 'Analyzing');
    assert.equal(main.disabled, true);
    await jobs.event({ ...hostJob(jobId, 'done'), file: 'C:\\song.opus', files: ['C:\\song.opus'], analysis: {bpm:128,keyShort:'Am'} });
    await wait(280);
    assert.equal(host.dataset.state, 'done');
    assert.equal(main.disabled, false);
    assert.equal(main.title, 'Saved · 128 BPM · Am');
    assert.equal(jobs.list().length, 0);
    assert.ok(chrome.calls.broadcasts.some(event => event.evt === 'jobs' && event.jobs.some(job => job.jobId === jobId && job.state === 'done')));
  } finally { native.disconnect(); }
});
