// Isolated UI checks with the real content script in Helium. No audio downloads.
import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { ROOT } from './lib.mjs';

const browser = await chromium.launch({ executablePath: join(process.env.LOCALAPPDATA, 'imput/Helium/Application/chrome.exe'), headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 900, height: 440 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  for (const [site, compact, light] of [['youtube', false, false], ['youtube', false, true], ['soundcloud', false, true], ['bandcamp', false, true], ['youtube', true, false]]) {
    await page.goto('about:blank');
    await page.setContent(`<body style="margin:64px;background:${light ? '#fff' : '#0f0f0f'};color:${light ? '#111' : '#fff'};font:16px system-ui"><h2>Siphon · ${site}${compact ? ' · Shorts' : ''}</h2><div id="mount" style="display:flex;align-items:center;width:fit-content"></div></body>`);
    await page.evaluate(({ site, compact, light }) => {
      window.fixture = { url: 'https://www.youtube.com/watch?v=abcdefghijk', requests: [], pending: [], site, compact, light, playlist: false };
      window.chrome = {
        runtime: { onMessage: { addListener() {} }, getURL: () => '', sendMessage(message) {
          fixture.requests.push(message);
          if (message.cmd === 'download') return Promise.resolve({ jobId: 'download-1' });
          return new Promise(resolve => fixture.pending.push(resolve));
        } },
        storage: { onChanged: { addListener() {} }, sync: { get: async () => ({ settings: {} }) } },
      };
      window.__siphonFonts = true;
    }, { site, compact, light });
    await page.addScriptTag({ path: join(ROOT, 'extension/src/content/core.js') });
    await page.evaluate(() => SiphonContent.start({
      site: fixture.site, dark: () => !fixture.light, current: () => ({ url: fixture.url }),
      collect: () => [{ mount: document.querySelector('#mount'), key: 'track', compact: fixture.compact, playlist: fixture.playlist, resolve: () => ({ url: fixture.url, title: 'Example track' }) }],
    }));
    const control = page.locator('[data-siphon]');
    const analyzer = control.locator('button.analyze');
    await analyzer.waitFor();
    const box = await analyzer.boundingBox();
    assert.equal(box.width, box.height, `${site} square button`);
    const moreBox = await control.locator('.more').boundingBox();
    assert.equal(moreBox.width, moreBox.height, `${site} square format arrow`);
    assert.equal(moreBox.width, box.width, `${site} matching side buttons`);
    await analyzer.click();
    await control.getByRole('status').waitFor({ state: 'visible' });
    assert.match(await control.getByRole('status').textContent(), /Analyzing/);
    assert.equal(await control.locator('.main').isDisabled(), false);
    await analyzer.click(); // Hide and reopen while pending: no second request.
    await analyzer.click();
    assert.equal(await page.evaluate(() => fixture.requests.length), 1);
    await page.evaluate(() => fixture.pending.shift()({ analysis: { bpm: 128, key: 'A minor', keyShort: 'Am', bpmConfidence: .9, keyConfidence: .8 } }));
    await page.waitForFunction(() => document.querySelector('[data-siphon]').shadowRoot.querySelector('.analysis-result')?.textContent.includes('128 BPM'));
    assert.match(await control.getByRole('status').textContent(), /128 BPM · A minor/);
    assert.deepEqual(await page.evaluate(() => fixture.requests.map(r => r.cmd)), ['analyze']);
    await page.keyboard.press('Escape');
    assert.equal(await control.getByRole('status').isVisible(), false);
    assert.equal(await control.locator('.analysis-summary').isVisible(), true);
    assert.match(await control.locator('.analysis-summary').textContent(), compact ? /128 BPMAm/ : /128 BPMA minor/);
    if (site === 'youtube' && !compact) {
      await mkdir(join(ROOT, 'tests/artifacts'), { recursive: true });
      await page.screenshot({ path: join(ROOT, `tests/artifacts/analyzer-inline-${light ? 'light' : 'dark'}.png`), animations: 'disabled' });
    }
    await analyzer.click(); // Cached result reopens without a request.
    assert.equal(await page.evaluate(() => fixture.requests.length), 1);
    if (site === 'youtube' && !compact) {
      await mkdir(join(ROOT, 'tests/artifacts'), { recursive: true });
      await page.screenshot({ path: join(ROOT, `tests/artifacts/analyzer-${light ? 'light' : 'dark'}.png`), animations: 'disabled' });
    }
    await control.locator('.more').click();
    assert.equal(await control.getByRole('status').isVisible(), false);
    assert.equal(await control.getByRole('menu').isVisible(), true);
    await page.keyboard.press('Escape');
    // Recycled player track: old result and late asynchronous responses must not leak.
    await page.evaluate(() => { fixture.url = 'https://www.youtube.com/watch?v=bbbbbbbbbbb'; dispatchEvent(new Event('yt-navigate-finish')); });
    await page.waitForFunction(() => document.querySelector('[data-siphon]').shadowRoot.querySelector('.analyze').dataset.ready === 'false');
    await analyzer.click();
    await page.evaluate(() => fixture.pending.shift()({ error: { message: 'Could not analyze this track' } }));
    await page.waitForFunction(() => document.querySelector('[data-siphon]').shadowRoot.querySelector('[role=status]').textContent.includes('Could not analyze'));
    await analyzer.click();
    assert.equal(await analyzer.getAttribute('aria-busy'), 'true');
    await page.evaluate(() => { fixture.url = 'https://www.youtube.com/watch?v=ccccccccccc'; dispatchEvent(new Event('yt-navigate-finish')); });
    await page.waitForFunction(() => document.querySelector('[data-siphon]').shadowRoot.querySelector('.analyze').getAttribute('aria-busy') === 'false');
    await page.evaluate(() => fixture.pending.shift()({ analysis: { bpm: 99, key: 'C major' } }));
    assert.equal(await analyzer.getAttribute('data-ready'), 'false');
    await control.locator('.main').click();
    assert.equal(await page.evaluate(() => fixture.requests.at(-1).cmd), 'download');
    await page.evaluate(() => { fixture.playlist = true; dispatchEvent(new Event('yt-navigate-finish')); });
    await page.waitForFunction(() => document.querySelector('[data-siphon]').shadowRoot.querySelector('.analyze').disabled);
    console.log(`PASS ${site} compact=${compact} light=${light}: square, analysis only, pending, cache, error/retry, stale response, download, collection guard`);
  }
  assert.deepEqual(errors, []);
} finally {
  await browser.close();
}
