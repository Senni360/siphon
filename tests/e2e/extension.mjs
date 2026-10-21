import { chromium } from 'playwright';
import { mkdtemp, rm, mkdir, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { ROOT, Report, check, command, sleep, skip, TERMINAL } from './lib.mjs';
import { MEDIA } from './fixtures.mjs';

const args = new Set(process.argv.slice(2));
if (args.has('--help')) { console.log('node tests/e2e/extension.mjs [--live] [--headless]\nUses real Helium, temporary profile, unpacked extension. --live: open CC YouTube watch page and download if host is registered. Optional SIPHON_HELIUM_PATH. No personal profile is used.'); process.exit(0); }
const ID = 'bieofegakabfdjplcnpiaegdifmempmf', origin = `chrome-extension://${ID}`;
const executablePath = process.env.SIPHON_HELIUM_PATH || join(process.env.LOCALAPPDATA, 'imput/Helium/Application/chrome.exe');
const extension = resolve(ROOT, 'extension'), temp = await mkdtemp(join(tmpdir(), 'siphon-e2e-extension-'));
const output = join(temp, 'downloads'), report = new Report('extension');
const errors = [], consoleMessages = [];
let context, worker, popup, options, registered = false;
const message = (page, cmd, payload = {}) => page.evaluate(({ cmd, payload }) => chrome.runtime.sendMessage({ cmd, ...payload }), { cmd, payload });
async function snapshotJob(page, id) {
  const state = await message(page, 'getState');
  return state.jobs?.find(j => j.jobId === id) || state.history?.find(j => j.id === id || j.jobId === id);
}
async function waitJob(page, id, timeout = 240000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { const job = await snapshotJob(page, id); if (job && TERMINAL.has(job.state)) return job; await sleep(500); }
  throw new Error(`Extension job ${id} did not finish within ${timeout}ms`);
}
async function dismissConsent(page) {
  // YouTube injects the EU consent dialog after DOMContentLoaded; a one-shot
  // count can miss it even when the page controls are already present.
  const reject = page.getByRole('button', { name: /^(Reject|Alles afwijzen|Alle ablehnen|Tout refuser|Alle weigeren)/i });
  for (let i = 0; i < 30; i++) {
    if (await reject.first().isVisible().catch(() => false)) { await reject.first().click(); await sleep(750); return; }
    await sleep(200);
  }
}
try {
  await report.test('launch real Helium with temporary profile and unpacked extension', async () => {
    check(existsSync(executablePath), `Helium missing: ${executablePath}`);
    await mkdir(output);
    await mkdir(resolve(ROOT, 'tests/artifacts'), { recursive: true });
    context = await chromium.launchPersistentContext(join(temp, 'profile'), {
      executablePath, headless: args.has('--headless'), viewport: { width: 1100, height: 850 },
      args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--no-first-run', '--no-default-browser-check'],
      timeout: 45000,
    });
    context.on('console', msg => {
      const entry = { type: msg.type(), text: msg.text(), location: msg.location() };
      consoleMessages.push(entry);
      if (entry.type === 'error' && entry.location.url?.startsWith(origin)) errors.push(entry);
    });
    context.on('weberror', error => { errors.push({ type: 'weberror', text: error.error().stack, location: { url: error.page()?.url() || origin } }); });
    context.on('page', page => page.on('pageerror', error => { if (page.url().startsWith(origin)) errors.push({ type: 'pageerror', text: error.stack, location: { url: page.url() } }); }));
    return await context.browser()?.version() || executablePath;
  });
  if (!context) throw new Error('Helium launch failed');
  await report.test('service worker registers with pinned ID', async () => {
    worker = context.serviceWorkers().find(w => w.url().startsWith(origin)) || await context.waitForEvent('serviceworker', { predicate: w => w.url().startsWith(origin), timeout: 30000 });
    check(worker.url() === `${origin}/src/background/service-worker.js`, `Unexpected worker ${worker.url()}`);
    const actual = await worker.evaluate(() => ({ id: chrome.runtime.id, manifest: chrome.runtime.getManifest() }));
    check(actual.id === ID, `Actual extension ID ${actual.id}`);
    return worker.url();
  });
  await report.test('popup loads without console/page errors and responds to getState', async () => {
    const before = errors.length; popup = await context.newPage();
    await popup.goto(`${origin}/src/popup/popup.html`, { waitUntil: 'load' });
    await popup.locator('h1').waitFor();
    const state = await message(popup, 'getState');
    check(state && !state.error && state.settings && Array.isArray(state.jobs) && Array.isArray(state.history), `Invalid getState ${JSON.stringify(state)}`);
    await sleep(750); check(errors.length === before, JSON.stringify(errors.slice(before)));
    await popup.screenshot({ path: resolve(ROOT, 'tests/artifacts/popup.png'), fullPage: true });
    return `host=${state.host?.state}; title=${await popup.title()}`;
  });
  await report.test('options loads without console/page errors', async () => {
    const before = errors.length; options = await context.newPage();
    await options.goto(`${origin}/src/options/options.html`, { waitUntil: 'load' });
    check((await options.locator('body').innerText()).includes('Siphon'), 'Options body missing');
    await sleep(1000); check(errors.length === before, JSON.stringify(errors.slice(before)));
    check(!await options.evaluate(() => [...document.scripts].some(s => !s.src && s.textContent.trim())), 'Executable inline script');
    await options.screenshot({ path: resolve(ROOT, 'tests/artifacts/options.png'), fullPage: true });
    return `title=${await options.title()}`;
  });
  await report.test('native host registration and connection', async () => {
    try { await command('reg.exe', ['query', 'HKCU\\Software\\Chromium\\NativeMessagingHosts\\com.siphon.host', '/ve']); registered = true; }
    catch { skip('HKCU Chromium native host is not registered; installer still pending'); }
    const status = await message(popup, 'reconnectHost'); check(status.state === 'ready', JSON.stringify(status));
    return `host=${status.hostVersion}; yt-dlp=${status.ytdlpVersion}`;
  });
  if (args.has('--live')) {
    if (registered) {
      await report.test('real CC YouTube download via extension message; file appears', async () => {
        const result = await message(popup, 'download', { url: MEDIA.youtube, title: 'Sintel (Blender Foundation, CC BY)', source: 'e2e', options: { format: 'mp3', quality: '320', outputDir: output, filenameTemplate: 'extension-message.%(ext)s', trim: { start: 0, end: 8 }, sponsorblockRemove: false, embedMetadata: true, embedThumbnail: true } });
        check(result.jobId && !result.error, JSON.stringify(result));
        const job = await waitJob(popup, result.jobId); check(job.state === 'done', JSON.stringify(job.error));
        check(job.file && resolve(job.file).startsWith(resolve(output) + '\\'), `Unexpected file ${job.file}`);
        check((await stat(job.file)).size > 0, 'Downloaded file missing or empty');
        check(job.analysis?.engine === 'siphon-analysis/1', `Missing history analysis: ${JSON.stringify(job)}`);
        check(/ \[[\d.]+ BPM [A-G][#b]?m?\]\.mp3$/.test(job.file), `Missing BPM/key filename suffix: ${job.file}`);
        return job.file.replace(temp, '<temp>');
      });
    } else report.skip('real CC YouTube download', 'Native host not registered');
    let watch;
    await report.test('YouTube watch page has an in-page Siphon button', async () => {
      watch = await context.newPage();
      await watch.goto(MEDIA.youtube, { waitUntil: 'domcontentloaded', timeout: 60000 });
      // Consent is part of a fresh European profile. Select only consent controls.
      await dismissConsent(watch);
      await watch.locator('[data-siphon] button.main').first().waitFor({ state: 'visible', timeout: 60000 });
      check((await watch.locator('[data-siphon]').count()) > 0, 'No injected Siphon controls');
      await watch.screenshot({ path: resolve(ROOT, 'tests/artifacts/youtube.png') });
      return await watch.locator('[data-siphon] button.main').first().getAttribute('aria-label');
    });
    if (registered && watch) await report.test('popup Analyze button shows tempo/key chips for CC YouTube video', async () => {
      // Keep the media tab active while inspecting the popup as a regular
      // extension document. Neither cache nor a synthetic host result can
      // satisfy this check: it must run the installed host's analyze request.
      // Use a different CC video from the completed download: the background
      // cache also lives in memory, so clearing session storage is insufficient.
      const analysisUrl = 'https://www.youtube.com/watch?v=HOfdboHvshg'; // Sintel trailer, CC BY 3.0
      await watch.goto(analysisUrl, { waitUntil: 'domcontentloaded', timeout: 60000 });
      await watch.bringToFront();
      await popup.reload({ waitUntil: 'load' });
      await popup.locator('#analyze:not([disabled])').waitFor({ timeout: 90000 });
      await popup.evaluate(() => document.getElementById('analyze').click());
      await popup.locator('#tab-analysis[aria-busy="false"] .music-chip').first().waitFor({ timeout: 150000 });
      check(await popup.locator('#tab-analysis .music-chip').count() >= 2, 'Tempo/key chips missing');
      const text = await popup.locator('#tab-analysis').innerText();
      check(/BPM/.test(text), `Unexpected analysis: ${text}`);
      const cached = await popup.evaluate(async url => (await chrome.storage.session.get('analyses')).analyses?.[url], analysisUrl);
      check(cached?.analysis?.durationAnalyzed > 8, `Expected full URL excerpt, got ${JSON.stringify(cached)}`);
      await popup.screenshot({ path: resolve(ROOT, 'tests/artifacts/analysis-live-popup.png'), fullPage: true });
      return text;
    });
    if (registered && watch) await report.test('in-page Siphon button starts a real download', async () => {
      const saved = await message(popup, 'getSettings');
      try {
        // No trim default exists in settings. Cancel after first real network bytes
        // rather than downloading a whole 15-minute film from the site button.
        await message(popup, 'setSettings', { patch: { outputDir: output, format: 'mp3', rateLimit: '64K', sponsorblockRemove: false, filenameTemplate: 'extension-button.%(ext)s' } });
        const before = await message(popup, 'getState'); const known = new Set([...before.jobs.map(j => j.jobId), ...before.history.map(j => j.id)]);
        await watch.locator('[data-siphon] button.main').first().click();
        let id, downloading = false;
        const end = Date.now() + 90000;
        while (Date.now() < end) {
          const state = await message(popup, 'getState');
          const job = state.jobs.find(j => !known.has(j.jobId));
          if (job) { id = job.jobId; if (job.state === 'error') throw new Error(JSON.stringify(job.error)); if (job.downloadedBytes > 0 || job.state === 'done') { downloading = true; break; } }
          await sleep(500);
        }
        if (id) await message(popup, 'cancel', { jobId: id });
        check(downloading, 'Button did not start real download bytes within 90s');
        return `job ${id}; real bytes received, then cancelled to keep clip short`;
      } finally { await message(popup, 'setSettings', { patch: saved }); }
    });
  } else report.skip('live download and YouTube in-page button', 'Run --live (CC media only)');
  await report.test('service worker/extension console remains error-free', async () => {
    await sleep(500);
    // Capture exceptions/rejections still pending inside the worker, plus all
    // context console messages Playwright exposes from extension URLs.
    check(errors.length === 0, JSON.stringify(errors));
    check(worker && await worker.evaluate(() => chrome.runtime.id) === ID, 'Service worker unavailable');
    return `${consoleMessages.length} console messages inspected`;
  });
} catch (e) { await report.test('suite prerequisites/orchestration', () => { throw e; }); }
finally {
  if (context) {
    // Cancel all test jobs before closing the native port and removing output.
    if (popup && !popup.isClosed()) {
      const state = await message(popup, 'getState').catch(() => null);
      for (const job of state?.jobs || []) if (!TERMINAL.has(job.state)) await message(popup, 'cancel', { jobId: job.jobId }).catch(() => {});
      await sleep(1000);
    }
    await context.close();
  }
  await rm(temp, { recursive: true, force: true, maxRetries: 20, retryDelay: 500 });
  await report.finish({ executablePath, extensionId: ID, registered, live: args.has('--live'), errors, consoleMessages, cleanedTemporaryProfileAndOutput: !existsSync(temp) });
}
