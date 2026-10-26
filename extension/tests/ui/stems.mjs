// Isolated extension UI verification. Explicit Helium, no native installation or audio work.
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join, extname } from 'node:path';
import assert from 'node:assert/strict';
const require = createRequire(new URL('../../../tests/package.json', import.meta.url));
const { chromium } = require('playwright');
const root = fileURLToPath(new URL('../../../', import.meta.url));
const server = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, 'http://localhost').pathname;
    const body = await readFile(join(root, path));
    response.setHeader('Content-Type', { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.woff2': 'font/woff2' }[extname(path)] || 'application/octet-stream');
    response.end(body);
  } catch { response.statusCode = 404; response.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ executablePath: join(process.env.LOCALAPPDATA, 'imput/Helium/Application/chrome.exe'), headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 380, height: 580 } });
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  page.on('console', msg => { if (msg.type() === 'error' && !msg.text().includes('404')) errors.push(msg.text()); });
  await page.addInitScript(() => {
    const listeners = [], local = {};
    const saved = JSON.parse(sessionStorage.getItem('stemFixture') || 'null');
    window.fixture = { calls: [], host: { state: 'ready', stems: true, dropzone: true }, history: [{ id: 'one', state: 'done', title: 'A quiet evening', format: 'wav', file: 'C:\\Music\\Evening.wav' }], emit: event => listeners.forEach(fn => fn(event)) };
    if (saved) Object.assign(fixture, saved);
    fixture.stems ||= [];
    const emit = fixture.emit;
    fixture.emit = event => { if (event.evt === 'stems') fixture.stems = event.stems; if (event.evt === 'history') fixture.history = event.history; emit(event); };
    window.chrome = {
      storage: { sync: { get: async () => ({ settings: { theme: 'dark' } }) }, session: { get: async () => ({}) }, local: { get: async () => local, set: async values => Object.assign(local, values) } },
      tabs: { query: async () => [], create: async () => {} }, commands: { getAll: async () => [] },
      runtime: { getManifest: () => ({ version: 'test' }), getURL: path => path, onMessage: { addListener: fn => listeners.push(fn) }, sendMessage: async request => {
        fixture.calls.push(request);
        if (request.cmd === 'getState') return { host: fixture.host, settings: { theme: 'dark' }, history: fixture.history, jobs: [], stems: fixture.stems };
        if (request.cmd === 'probeTab') return {};
        if (request.cmd === 'cancelStems') return { cancelled: true };
        return {};
      } },
    };
  });
  const base = `http://127.0.0.1:${server.address().port}/extension/src/`;
  for (const surface of ['popup/popup.html', 'options/options.html#history']) {
    await page.setViewportSize(surface.startsWith('popup') ? { width: 380, height: 580 } : { width: 1280, height: 900 });
    await page.goto(base + surface);
    const split = page.getByRole('button', { name: 'Split stems', exact: true });
    await split.waitFor({ timeout: 5000 }).catch(async error => { console.log(await page.locator('body').innerText(), errors); throw error; }); assert.equal(await split.isEnabled(), true);
    assert.equal(await page.evaluate(() => fixture.calls.some(c => c.cmd === 'splitStems')), false);
    await split.click(); await page.getByRole('dialog').waitFor();
    assert.match(await page.getByRole('dialog').textContent(), /runs locally/);
    await page.getByRole('button', { name: 'Not now' }).click();
    assert.equal(await page.evaluate(() => fixture.calls.some(c => c.cmd === 'splitStems')), false);
    await split.click(); await page.getByRole('button', { name: 'Download setup & split' }).click();
    await page.waitForFunction(() => fixture.calls.some(c => c.cmd === 'splitStems' && c.id === 'one'));
    await page.evaluate(() => fixture.emit({ evt: 'stems', stems: [{ jobId: 'stem-one', sourceId: 'one', state: 'running', stage: 'downloading_model', progress: null }] }));
    assert.match(await page.locator('.stem-status').textContent(), /Setting up stems/);
    await page.getByRole('button', { name: 'Cancel split' }).click();
    await page.evaluate(() => fixture.emit({ evt: 'stems', stems: [{ jobId: 'stem-one', sourceId: 'one', state: 'error', error: { message: 'Not enough disk space.' } }] }));
    assert.equal(await page.getByRole('button', { name: 'Retry split' }).isEnabled(), true);
    assert.match(await page.locator('.stem-status').textContent(), /disk space/);
    await page.evaluate(() => fixture.emit({ evt: 'stems', stems: [{ jobId: 'stem-one', sourceId: 'one', state: 'done', outputDir: 'C:\\Music\\stems', files: ['vocals', 'drums', 'bass', 'other'] }] }));
    await page.getByRole('button', { name: 'Stems folder' }).click();
    await page.getByRole('button', { name: 'Drag all stems' }).click();
    assert.equal(await page.evaluate(() => fixture.calls.some(c => c.cmd === 'showStems')), true);
    await mkdir(join(root, 'tests/artifacts'), { recursive: true });
    await page.screenshot({ path: join(root, `tests/artifacts/stems-${surface.split('/')[0]}.png`), fullPage: true });
    await page.evaluate(() => { fixture.emit({ evt: 'stems', stems: [] }); fixture.history[0].analysisPending = 1; fixture.emit({ evt: 'history', history: fixture.history }); });
    assert.equal(await split.isDisabled(), true);
    await page.evaluate(() => fixture.emit({ evt: 'host', status: { state: 'ready' } }));
    assert.match(await page.locator('.stem-controls').textContent(), /latest Siphon helper/);
    assert.equal(await split.isDisabled(), true);

    await page.evaluate(() => {
      fixture.emit({ evt: 'host', status: fixture.host });
      fixture.history = [{ id: 'playlist', title: 'Two tracks', state: 'done', file: 'C:\\Music\\last.wav', files: ['C:\\Music\\first.wav', 'C:\\Music\\last.wav'] }];
      fixture.emit({ evt: 'history', history: fixture.history });
    });
    const picker = page.getByRole('combobox', { name: 'Choose track for stems: Two tracks' });
    assert.equal(await split.count(), 0, 'playlist must never silently choose the last file');
    await picker.selectOption({ label: 'first.wav' });
    await split.click();
    await page.waitForFunction(() => fixture.calls.some(c => c.cmd === 'splitStems' && c.id === 'playlist' && c.path === fixture.history[0].files[0]));
    await page.evaluate(() => fixture.emit({ evt: 'stems', stems: [
      { jobId: 'first', sourceId: 'playlist', sourceFile: '\\\\?\\c:\\music\\FIRST.wav', state: 'running', stage: 'separating', progress: .25, createdAt: 1 },
      { jobId: 'last', sourceId: 'playlist', sourceFile: 'C:\\Music\\last.wav', state: 'done', files: ['v','d','b','o'], outputDir: 'C:\\Music\\last-stems', createdAt: 2 }
    ] }));
    assert.match(await page.locator('.stem-status').textContent(), /25%/);
    assert.equal(await page.locator('#active-stems').isVisible(), false, 'visible active row is not duplicated');
    await picker.selectOption({ label: 'last.wav' });
    assert.match(await page.locator('#active-stems').textContent(), /FIRST.wav/);
    await page.getByRole('button', { name: 'Stems folder' }).click();
    assert.equal(await page.evaluate(() => fixture.calls.at(-1).path), 'C:\\Music\\last-stems');
    await picker.selectOption({ label: 'first.wav' });
    assert.equal(await page.locator('#active-stems').isVisible(), false);
    // A source outside the popup's six rows still has cancellation controls.
    await page.evaluate(() => {
      fixture.history = [...Array.from({length: 6}, (_, i) => ({ id: `filler-${i}`, state: 'done', title: `Recent ${i}`, file: `C:\\Music\\${i}.wav` })), ...fixture.history];
      fixture.emit({ evt: 'history', history: fixture.history });
    });
    if (surface.startsWith('popup')) assert.equal(await page.locator('#active-stems').isVisible(), true);
    // Removing the entire history cannot hide an active native operation.
    await page.evaluate(() => fixture.emit({ evt: 'history', history: [] }));
    assert.equal(await page.locator('#active-stems').isVisible(), true);
    await page.getByRole('button', { name: 'Cancel split' }).click();
    assert.equal(await page.evaluate(() => fixture.calls.at(-1).jobId), 'first');
    await page.evaluate(() => sessionStorage.setItem('stemFixture', JSON.stringify({ history: fixture.history, stems: fixture.stems })));
    await page.reload();
    await page.getByRole('button', { name: 'Cancel split' }).waitFor();
    assert.match(await page.locator('#active-stems').textContent(), /25%/);
    await page.getByRole('button', { name: 'Cancel split' }).click();
    await page.evaluate(() => fixture.emit({ evt: 'stems', stems: [{ ...fixture.stems[0], cancelRequested: true }] }));
    assert.match(await page.locator('#active-stems').textContent(), /Cancelling/);
    await page.screenshot({ path: join(root, `tests/artifacts/stems-${surface.split('/')[0]}-orphan.png`), fullPage: true });
    await page.evaluate(() => fixture.emit({ evt: 'stems', stems: [{ ...fixture.stems[0], state: 'cancelled' }] }));
    assert.equal(await page.locator('#active-stems').isVisible(), false);
    assert.doesNotMatch(await page.locator('body').innerText(), /cancellation requested|Cancelling stems/i);
    await page.evaluate(() => sessionStorage.removeItem('stemFixture'));
    console.log(`PASS ${surface}: opt-in, setup, cancel, error/retry, completion, analysis/capability gates, playlist selection, per-track results, canonical paths, hidden/removed history, reopen, terminal cancellation`);

  }
  assert.deepEqual(errors, []);
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
