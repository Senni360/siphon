/** Pure URL handling shared by the worker and tests. Only HTTP(S) reaches yt-dlp. */
export function normalizeMediaUrl(value, playlist = 'single') {
  let url;
  try { url = new URL(value); } catch { return null; }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return null;
  const host = url.hostname.toLowerCase();
  if (host === 'youtu.be' || host.endsWith('.youtu.be')) {
    const id = url.pathname.split('/').filter(Boolean)[0];
    if (!id) return null;
    const target = new URL('https://www.youtube.com/watch');
    target.searchParams.set('v', id);
    if (playlist === 'all' && url.searchParams.has('list')) target.searchParams.set('list', url.searchParams.get('list'));
    url = target;
  } else if (host === 'youtube.com' || host.endsWith('.youtube.com')) {
    const match = url.pathname.match(/^\/(?:shorts|embed|live)\/([^/]+)/);
    if (match) {
      url.pathname = '/watch';
      url.searchParams.set('v', match[1]);
    }
    if (url.pathname === '/watch') {
      const id = url.searchParams.get('v');
      const list = url.searchParams.get('list');
      url = new URL('https://www.youtube.com/watch');
      if (id) url.searchParams.set('v', id);
      if (playlist === 'all' && list) url.searchParams.set('list', list);
    }
  }
  url.hash = '';
  return url.href;
}

export function siteForUrl(value) {
  try {
    const host = new URL(value).hostname.toLowerCase();
    if (host === 'youtu.be' || host === 'youtube.com' || host.endsWith('.youtube.com')) return host === 'music.youtube.com' ? 'youtubeMusic' : 'youtube';
    for (const site of ['soundcloud', 'bandcamp']) if (host === `${site}.com` || host.endsWith(`.${site}.com`)) return site;
    return 'other';
  } catch { return 'other'; }
}

export function isObviouslyMediaUrl(value) {
  const normalized = normalizeMediaUrl(value);
  if (!normalized) return false;
  const url = new URL(normalized);
  const site = siteForUrl(normalized);
  if (site === 'youtube' || site === 'youtubeMusic') return (url.pathname === '/watch' && url.searchParams.has('v')) || (url.pathname === '/playlist' && url.searchParams.has('list'));
  if (site === 'soundcloud') return url.pathname.split('/').filter(Boolean).length >= 2 && !/^\/(discover|search|you|stream)(\/|$)/.test(url.pathname);
  if (site === 'bandcamp') return /^\/(track|album)\//.test(url.pathname);
  return /\.(mp3|m4a|opus|ogg|wav|flac|mp4|webm|m3u8)(?:$)/i.test(url.pathname) || /(?:vimeo\.com\/\d+|twitch\.tv\/.*(?:videos\/\d+|clip\/)|tiktok\.com\/.*\/video\/|(?:twitter|x)\.com\/.*\/status\/)/i.test(url.host + url.pathname);
}
