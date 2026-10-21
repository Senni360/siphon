import { siteForUrl } from '../urls.js';

// Simple filled marks, drawn on a 16-unit grid so they sit with 13px text.
const MARKS = {
  youtube: 'M15.4 4.6a2 2 0 0 0-1.4-1.4C12.8 3 8 3 8 3s-4.8 0-6 .2A2 2 0 0 0 .6 4.6C.4 5.8.4 8 .4 8s0 2.2.2 3.4A2 2 0 0 0 2 12.8c1.2.2 6 .2 6 .2s4.8 0 6-.2a2 2 0 0 0 1.4-1.4c.2-1.2.2-3.4.2-3.4s0-2.2-.2-3.4ZM6.5 10.3V5.7L10.5 8Z',
  youtubeMusic: 'M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1Zm0 3.2A3.8 3.8 0 1 1 8 11.8 3.8 3.8 0 0 1 8 4.2ZM6.8 6.2v3.6L9.9 8Z',
  soundcloud: 'M7.2 5.2c.4-.2.9-.3 1.4-.3a3.7 3.7 0 0 1 3.7 3.3 2.1 2.1 0 1 1 .6 4.1H7.2ZM6 6v6.3h-.8V6.4c.3-.2.5-.3.8-.4Zm-1.8 1v5.3h-.8V7.3c.3-.1.5-.2.8-.3ZM2.4 8v4.3h-.8V8.2l.8-.2ZM.6 9.1v2.9a1.6 1.6 0 0 1 0-2.9Z',
  bandcamp: 'M5 3.5h10.4L11 12.5H.6Z',
  other: 'M8 1.2a6.8 6.8 0 1 0 0 13.6A6.8 6.8 0 0 0 8 1.2Zm0 1.5c.7 0 1.7 1.6 2 4.5H6c.3-2.9 1.3-4.5 2-4.5Zm-2 6h4c-.3 2.9-1.3 4.5-2 4.5s-1.7-1.6-2-4.5ZM2.8 7.2a5.3 5.3 0 0 1 2.6-3.8 9.6 9.6 0 0 0-.9 3.8Zm0 1.5h1.7a9.6 9.6 0 0 0 .9 3.8 5.3 5.3 0 0 1-2.6-3.8Zm7.8 3.8a9.6 9.6 0 0 0 .9-3.8h1.7a5.3 5.3 0 0 1-2.6 3.8Zm.9-5.3a9.6 9.6 0 0 0-.9-3.8 5.3 5.3 0 0 1 2.6 3.8Z',
};
export const SERVICE_NAMES = { youtube: 'YouTube', youtubeMusic: 'YouTube Music', soundcloud: 'SoundCloud', bandcamp: 'Bandcamp', other: 'Web' };

/** Which service a URL or history item came from: youtube | youtubeMusic | soundcloud | bandcamp | other. */
export function serviceOf(value) {
  const url = typeof value === 'string' ? value : value?.webpageUrl || value?.url;
  const site = url ? siteForUrl(url) : 'other';
  return MARKS[site] ? site : 'other';
}

/** <span class="service-mark" data-service> with its mark and name; tinted by tokens.css. */
export function serviceMark(service, { label = true } = {}) {
  const span = document.createElement('span');
  span.className = 'service-mark'; span.dataset.service = service;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 16 16'); svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', MARKS[service] || MARKS.other); svg.append(path);
  span.append(svg);
  if (label) span.append(SERVICE_NAMES[service] || SERVICE_NAMES.other);
  else span.setAttribute('aria-label', SERVICE_NAMES[service] || SERVICE_NAMES.other);
  return span;
}
