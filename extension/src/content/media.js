/* Pure URL helpers, shared by ordered classic content scripts and Node tests. */
(() => {
  'use strict';
  const parse = (value, base) => {
    if (typeof value !== 'string' || !value.trim()) return null;
    try { const u = new URL(value, base); return ['http:', 'https:'].includes(u.protocol) ? u : null; } catch { return null; }
  };
  function youtube(value, base = 'https://www.youtube.com') {
    const u = parse(value, base);
    if (!u || !['www.youtube.com', 'youtube.com', 'music.youtube.com', 'm.youtube.com', 'youtu.be'].includes(u.hostname)) return null;
    const id = u.hostname === 'youtu.be' ? u.pathname.slice(1).split('/')[0]
      : /^\/(?:shorts|embed|live)\//.test(u.pathname) ? u.pathname.split('/')[2]
        : u.pathname === '/watch' ? u.searchParams.get('v') : null;
    return /^[\w-]{11}$/.test(id || '') ? `https://www.youtube.com/watch?v=${id}` : null;
  }
  function youtubePlaylist(value, base = 'https://music.youtube.com') {
    const u = parse(value, base);
    if (!u || !['youtube.com', 'www.youtube.com', 'music.youtube.com'].includes(u.hostname)) return null;
    const id = u.searchParams.get('list');
    return /^[\w-]+$/.test(id || '') ? `https://music.youtube.com/playlist?list=${id}` : null;
  }
  function soundcloud(value, base = 'https://soundcloud.com') {
    const u = parse(value, base);
    if (!u || !['soundcloud.com', 'www.soundcloud.com'].includes(u.hostname)) return null;
    const parts = u.pathname.split('/').filter(Boolean);
    if (['discover', 'stream', 'feed', 'search', 'you', 'upload', 'stations', 'charts', 'settings', 'pages', 'terms-of-use'].includes(parts[0])) return null;
    // Preserve secret tokens in private permalinks; drop tracking query parameters.
    const valid = (parts.length === 2 && !['sets', 'tracks', 'albums', 'reposts', 'likes', 'followers', 'following', 'popular-tracks'].includes(parts[1]))
      || (parts.length === 3 && (parts[1] === 'sets' || /^s-[\w-]+$/.test(parts[2])))
      || (parts.length === 4 && parts[1] === 'sets' && /^s-[\w-]+$/.test(parts[3]));
    if (!valid) return null;
    const secret = u.searchParams.get('secret_token');
    return `https://soundcloud.com/${parts.join('/')}${secret ? `?secret_token=${encodeURIComponent(secret)}` : ''}`;
  }
  function bandcamp(value, base) {
    const u = parse(value, base);
    if (!u || !u.hostname.endsWith('.bandcamp.com') || !/^\/(track|album)\/[^/]+\/?$/.test(u.pathname)) return null;
    return `https://${u.hostname}${u.pathname.replace(/\/$/, '')}`;
  }
  globalThis.SiphonMedia = Object.freeze({ youtube, youtubePlaylist, soundcloud, bandcamp });
})();
