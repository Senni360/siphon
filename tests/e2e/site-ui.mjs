import { chromium } from 'playwright';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';

import { ROOT as root } from './lib.mjs';
const profile = await mkdtemp(join(tmpdir(), 'siphon-site-ui-'));
const extension = join(root, 'extension');
const artifacts = join(root, 'tests/artifacts');
let context;
try {
  await mkdir(artifacts, { recursive: true });
  context = await chromium.launchPersistentContext(profile, {
    executablePath: join(process.env.LOCALAPPDATA, 'imput/Helium/Application/chrome.exe'),
    headless: true, viewport: { width: 1440, height: 900 },
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--no-first-run'],
  });
  const errors = [];
  context.on('console', m => { if(m.type() === 'error' && m.location().url?.startsWith('chrome-extension:')) errors.push(m.text()); });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  const page = await context.newPage();
  await page.goto('https://www.youtube.com/watch?v=eRsGyueVLvQ', {waitUntil:'domcontentloaded'});
  const consent = page.locator('ytd-consent-bump-v2-lightbox');
  const reject = consent.getByRole('button', {name:/Reject|afwijzen|weigeren|refuser|ablehnen/i});
  await consent.waitFor({state:'visible',timeout:12000}).catch(()=>{});
  if(await consent.isVisible()) {
    await reject.first().click({timeout:5000});
    await consent.waitFor({state:'hidden',timeout:10000});
  }
  const control = page.locator('ytd-watch-metadata #owner [data-siphon="watch"]');
  await control.waitFor({state:'visible',timeout:45000});
  if(await reject.first().isVisible()) await reject.first().click();
  console.log('YouTube control located beside Subscribe.');
  assert.equal(await page.locator('[data-siphon="watch"]').count(),1);
  await control.locator('button.more').click();
  assert.equal(await control.getByRole('menu').isVisible(),true);
  await page.keyboard.press('End');
  assert.equal(await control.getByRole('menuitem').last().evaluate(e=>e===e.getRootNode().activeElement),true);
  await page.keyboard.press('Escape');
  assert.equal(await control.getByRole('menu').isVisible(),false);
  await page.screenshot({path:join(artifacts,'youtube-live-light.png'),animations:'disabled'});
  // Simulate YouTube reconciling an unknown injected child away.
  await control.evaluate(e=>e.remove());
  await control.waitFor({state:'visible',timeout:3000});
  assert.equal(await page.locator('[data-siphon="watch"]').count(),1);
  // Exercise the same existing document's SPA transition and the event the adapter uses.
  await page.evaluate(()=>{history.pushState({},'', '/watch?v=aqz-KE-bpKQ');dispatchEvent(new Event('yt-navigate-finish'));});
  await page.waitForFunction(()=>document.querySelector('[data-siphon="watch"]')?.shadowRoot.querySelector('button.main')?.disabled===false);
  await page.waitForTimeout(350);
  const media = await worker.evaluate(()=>chrome.tabs.query({url:'https://www.youtube.com/*'}).then(t=>chrome.tabs.sendMessage(t[0].id,{q:'siphon:getMedia'})));
  assert.equal(media.url,'https://www.youtube.com/watch?v=aqz-KE-bpKQ');
  assert.equal(await page.locator('[data-siphon="watch"]').count(),1);
  await page.evaluate(()=>document.documentElement.setAttribute('dark',''));
  await page.waitForFunction(()=>document.querySelector('[data-siphon="watch"]')?.dataset.light==='false');
  await control.locator('button.more').click();
  await control.getByRole('menu').waitFor({state:'visible'});
  await page.waitForTimeout(300);
  await page.screenshot({path:join(artifacts,'youtube-live-dark-menu.png'),animations:'disabled'});
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({visibleBesideSubscribe:true,keyboardMenu:true,reinsertAfterRemoval:true,spaUrlRefresh:true,singleControl:true,extensionConsoleErrors:errors}));
} finally {
  await context?.close();
  // Only the fresh temporary profile created above is removed.
  if (!resolve(profile).startsWith(resolve(tmpdir()) + '\\') || !profile.includes('siphon-site-ui-')) throw new Error('Unexpected profile path');
  await rm(profile,{recursive:true,force:true,maxRetries:10,retryDelay:300});
}
