import { siteForUrl } from '../shared/urls.js';

export function toNetscapeCookies(cookies) {
  const safe = value => String(value ?? '').replace(/[\t\r\n]/g, '');
  return '# Netscape HTTP Cookie File\n' + cookies.map(cookie => [
    `${cookie.httpOnly ? '#HttpOnly_' : ''}${safe(cookie.domain)}`,
    (cookie.hostOnly ?? !cookie.domain.startsWith('.')) ? 'FALSE' : 'TRUE',
    safe(cookie.path || '/'), cookie.secure ? 'TRUE' : 'FALSE',
    cookie.session ? 0 : Math.floor(cookie.expirationDate || 0), safe(cookie.name), safe(cookie.value),
  ].join('\t')).join('\n') + '\n';
}

export async function siteCookies(chrome, value) {
  const url = new URL(value);
  const domains = new Set();
  const labels = url.hostname.split('.');
  // Never query a bare top-level domain. Domain filters also return subdomains.
  for (let index = 0; index < labels.length - 1; index++) domains.add(labels.slice(index).join('.'));
  const site = siteForUrl(value);
  if (site.startsWith('youtube')) { domains.add('youtube.com'); domains.add('google.com'); }
  if (site === 'soundcloud') domains.add('soundcloud.com');
  const results = await Promise.all([
    chrome.cookies.getAll({ url: url.href }),
    ...[...domains].map(domain => chrome.cookies.getAll({ domain })),
  ]);
  const unique = new Map();
  for (const cookie of results.flat()) {
    const domain = cookie.domain.replace(/^\./, '').toLowerCase();
    const related = url.hostname === domain || url.hostname.endsWith(`.${domain}`);
    const youtubeLogin = site.startsWith('youtube') && (domain === 'google.com' || domain.endsWith('.google.com') || domain === 'youtube.com' || domain.endsWith('.youtube.com'));
    if (related || youtubeLogin) unique.set(JSON.stringify([cookie.domain, cookie.path, cookie.name, cookie.storeId, cookie.partitionKey]), cookie);
  }
  return toNetscapeCookies([...unique.values()]);
}
