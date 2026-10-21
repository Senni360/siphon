import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMediaUrl, isObviouslyMediaUrl, siteForUrl } from '../../src/shared/urls.js';
import { resolveTab } from '../../src/background/router.js';
import { createChromeMock } from '../helpers/chrome-mock.js';

test('YouTube URLs normalize shorts, shortlinks, music and playlist policy', () => {
  for (const url of ['https://youtu.be/abc?si=tracking&list=PL', 'https://music.youtube.com/watch?v=abc&list=PL', 'https://www.youtube.com/shorts/abc?list=PL', 'https://youtube.com/embed/abc']) {
    assert.equal(normalizeMediaUrl(url), 'https://www.youtube.com/watch?v=abc');
  }
  assert.equal(normalizeMediaUrl('https://youtu.be/abc?list=PL', 'all'), 'https://www.youtube.com/watch?v=abc&list=PL');
  assert.equal(normalizeMediaUrl('https://music.youtube.com/watch?v=abc&list=PL', 'all'), 'https://www.youtube.com/watch?v=abc&list=PL');
});
test('URLs reject browser pages, credentials, malformed and script URLs', () => {
  for (const url of ['chrome://extensions', 'javascript:alert(1)', 'not a url', 'file:///a', 'https://user:pass@site.com']) assert.equal(normalizeMediaUrl(url), null);
  assert.equal(siteForUrl('https://youtube.com.evil.test/watch?v=a'), 'other');
});
test('media recognition supports supported sites without treating homepages as tracks', () => {
  for (const url of ['https://soundcloud.com/artist/track', 'https://artist.bandcamp.com/album/name', 'https://x.com/a/status/123', 'https://cdn.test/a.webm']) assert.equal(isObviouslyMediaUrl(url), true, url);
  for (const url of ['https://youtube.com/', 'https://soundcloud.com/discover', 'https://artist.bandcamp.com', 'https://example.org/article']) assert.equal(isObviouslyMediaUrl(url), false, url);
});
test('current tab queries content responder, tolerates absence and rejects internal pages', async () => {
  const chrome = createChromeMock({ tabs: [{ id: 2, active: true, url: 'https://example.org/article', title: 'Article' }] });
  assert.equal((await resolveTab(chrome)).url, 'https://example.org/article');
  chrome.tabMessageHandler = (id, message) => { assert.equal(id, 2); assert.deepEqual(message, { q: 'siphon:getMedia' }); return { url: 'https://cdn.test/song.mp3', title: 'Song' }; };
  assert.deepEqual(await resolveTab(chrome), { url: 'https://cdn.test/song.mp3', title: 'Song' });
  chrome.tabs.items[0].url = 'chrome://settings';
  chrome.tabMessageHandler = () => null;
  await assert.rejects(resolveTab(chrome), { code: 'UNSUPPORTED_URL' });
});
