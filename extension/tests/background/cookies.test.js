import test from 'node:test';
import assert from 'node:assert/strict';
import { siteCookies, toNetscapeCookies } from '../../src/background/cookies.js';
import { createChromeMock } from '../helpers/chrome-mock.js';

test('Netscape serialization handles httpOnly, sessions, expiry and unsafe newlines', () => {
  const text = toNetscapeCookies([{ domain: '.site.com', path: '/', name: 'SID', value: 'one\ntwo', httpOnly: true, secure: true, expirationDate: 123.9 }]);
  assert.ok(text.includes('#HttpOnly_.site.com\tTRUE\t/\tTRUE\t123\tSID\tonetwo\n'));
  assert.ok(toNetscapeCookies([{ domain: 'site.com', hostOnly: true, session: true, name: 'a', value: 'b' }]).includes('site.com\tFALSE\t/\tFALSE\t0\ta\tb'));
});
test('YouTube exports deduplicated youtube/google auth cookies, excludes unrelated sites', async () => {
  const chrome = createChromeMock({ cookies: ['.youtube.com', '.google.com', '.soundcloud.com'].map(domain => ({ domain, name: 'SID', value: domain, path: '/' })) });
  const text = await siteCookies(chrome, 'https://music.youtube.com/watch?v=a');
  assert.equal(text.split('\n').filter(line => line.includes('\tSID\t')).length, 2);
  assert.ok(text.includes('.google.com')); assert.ok(!text.includes('.soundcloud.com'));
});
test('parent-domain queries never export sibling domains under a public suffix', async () => {
  const chrome = createChromeMock({ cookies: ['.artist.co.uk', '.co.uk', '.unrelated.co.uk'].map(domain => ({ domain, name: 'a', value: 'b' })) });
  const text = await siteCookies(chrome, 'https://artist.co.uk/track');
  assert.ok(text.includes('.artist.co.uk')); assert.ok(!text.includes('.unrelated.co.uk'));
});
