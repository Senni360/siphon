import { CMD, HOST } from '../shared/protocol.js';
import { normalizeMediaUrl, siteForUrl } from '../shared/urls.js';
import { siteCookies } from './cookies.js';
import { errorData, failure } from './common.js';
import { AnalysisCache } from './analysis.js';
import { dropItem } from '../shared/music.js';

export async function resolveTab(chrome, { tabId, playlist = 'single' } = {}) {
  const tab = tabId !== undefined ? await chrome.tabs.get(tabId) : (await chrome.tabs.query({ active: true, currentWindow: true }))[0];
  if (!tab) throw failure('UNSUPPORTED_URL', 'Open a media page first.');
  let url = normalizeMediaUrl(tab.url, playlist);
  let title = tab.title;
  const page = url ? new URL(url) : null, site = siteForUrl(tab.url);
  const collection = page && (site.startsWith('youtube') && page.searchParams.has('list') || site === 'soundcloud' && page.pathname.includes('/sets/') || site === 'bandcamp' && page.pathname.startsWith('/album/'));
  if ((playlist === 'single' || !collection) && tab.id !== undefined) {
    try {
      const media = await chrome.tabs.sendMessage(tab.id, { q: 'siphon:getMedia' });
      if (media?.url) { url = normalizeMediaUrl(media.url, playlist) || url; title = media.title || title; }
    } catch { /* A site without a content script can still be supported by yt-dlp. */ }
  }
  if (!url) throw failure('UNSUPPORTED_URL', 'Open an HTTP or HTTPS media page first.');
  return { url, title };
}

export function createRouter({ chrome, ready, native, jobs, stems, getSettings, setSettings, analysisCache = new AnalysisCache(chrome) }) {
  const cacheReady = analysisCache.init();
  const analyzing = new Map();
  async function analyzeUrl(value) {
    await cacheReady;
    const url = normalizeMediaUrl(value);
    if (!url) throw failure('UNSUPPORTED_URL', 'Choose a valid HTTP or HTTPS media URL.');
    const cached = analysisCache.get(url);
    if (cached) return cached;
    if (analyzing.has(url)) return analyzing.get(url);
    const request = (async () => {
      const settings = await getSettings();
      const cookies = settings.sendSiteCookies ? await siteCookies(chrome, url) : undefined;
      const result = await native.request(HOST.ANALYZE, {url, ...(cookies ? {cookies} : {})});
      await analysisCache.set(url,result);
      return result;
    })();
    analyzing.set(url,request);
    try { return await request; } finally { analyzing.delete(url); }
  }
  async function probe(url) {
    const settings = await getSettings();
    // Probe the collection context independently of the default download scope.
    url = normalizeMediaUrl(url, 'all');
    if (!url) throw failure('UNSUPPORTED_URL', 'Choose a valid media URL.');
    const cookies = settings.sendSiteCookies ? await siteCookies(chrome, url) : undefined;
    return native.request(HOST.PROBE, { url, cookiesFromBrowser: settings.cookiesFromBrowser, flatPlaylist: true, ...(cookies ? { cookies } : {}) });
  }
  async function route(message) {
    await ready; await cacheReady;
    const { cmd } = message;
    if ([CMD.GET_STATE, CMD.PROBE, CMD.PROBE_TAB, CMD.ANALYZE, CMD.ANALYZE_TAB, CMD.ANALYZE_FILE, CMD.GET_SETTINGS, CMD.RECONNECT_HOST].includes(cmd)) native.touchUI();
    switch (cmd) {
      case CMD.GET_STATE:
        await native.connect().catch(() => {});
        return { host: native.status, jobs: jobs.list(), history: jobs.history, stems: stems?.list() || [], settings: await getSettings() };
      case CMD.SPLIT_STEMS: return stems.split(message.id, message.path);
      case CMD.CANCEL_STEMS: return stems.cancel(message.jobId);
      case CMD.SHOW_STEMS: return stems.show(message.jobId);
      case CMD.GET_SETTINGS: return getSettings();
      case CMD.ANALYZE: return analyzeUrl(message.url);
      case CMD.ANALYZE_TAB: return analyzeUrl((await resolveTab(chrome, {tabId:message.tabId})).url);
      case CMD.ANALYZE_FILE: {
        let path = message.path;
        if (path === undefined) path = (await native.request(HOST.PICK_FILE)).path;
        if (path == null) return null;
        if (typeof path !== 'string' || !path.trim()) throw failure('BAD_REQUEST', 'Choose an audio file.');
        return {...await native.request(HOST.ANALYZE, {path}), path};
      }
      case CMD.SET_SETTINGS: return setSettings(message.patch || {});
      case CMD.PROBE: return probe(message.url);
      case CMD.PROBE_TAB: {
        let url = null;
        try {
          const media = await resolveTab(chrome, { tabId: message.tabId, playlist: 'all' });
          url = media.url;
          return { url, info: await probe(url), error: null, ...(analysisCache.get(url) || {}) };
        } catch (error) { return { url, info: null, error: errorData(error) }; }
      }
      case CMD.DOWNLOAD: return jobs.download(message);
      case CMD.DOWNLOAD_TAB: {
        const settings = await getSettings();
        const media = await resolveTab(chrome, { tabId: message.tabId, playlist: message.options?.playlist || settings.playlist });
        return jobs.download({ ...media, options: message.options });
      }
      case CMD.CANCEL: return jobs.cancel(message.jobId);
      case CMD.RETRY: return jobs.retry(message.jobId);
      case CMD.REVEAL:
      case CMD.OPEN:
        if (typeof message.path !== 'string' || !message.path) throw failure('BAD_REQUEST', 'Choose a downloaded file.');
        return native.request(cmd === CMD.REVEAL ? HOST.REVEAL : HOST.OPEN, { path: message.path });
      case CMD.PICK_FOLDER: return native.request(HOST.PICK_FOLDER, { ...(message.initial ? { initial: message.initial } : {}) });
      case CMD.UPDATE_HOST: return native.update();
      case CMD.RECONNECT_HOST: return native.reconnect();
      case CMD.CLEAR_HISTORY: return jobs.clearHistory(message.ids);
      case CMD.SHOW_DROP:
      case CMD.GRAB: {
        const item = jobs.history.find(entry => entry.id === message.id);
        if (!item?.file) throw failure('BAD_REQUEST', 'Choose a downloaded file.');
        const settings = await getSettings();
        if (cmd === CMD.GRAB) return native.request(HOST.GRAB, { item: dropItem(item, settings.keyNotation) });
        return native.request(HOST.SHOW_DROP, { items: [dropItem(item, settings.keyNotation)], pinned: true });
      }
      default: throw failure('BAD_REQUEST', `Unknown command: ${cmd}`);
    }
  }
  const listener = (message, sender, respond) => {
    if (!message?.cmd) return false;
    void route(message).then(respond, error => respond({ error: errorData(error) }));
    return true;
  };
  chrome.runtime.onMessage.addListener(listener);
  return { route, listener };
}
