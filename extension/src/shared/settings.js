// Settings schema + defaults. Stored in chrome.storage.sync under key "settings".

export const FORMATS = ['mp3', 'm4a', 'opus', 'flac', 'wav', 'ogg', 'best'];
export const QUALITIES = ['320', '256', '192', '128', 'V0', 'best'];

export const SITE_INTEGRATIONS = ['youtube', 'youtubeMusic', 'soundcloud', 'bandcamp'];

export const DEFAULT_SETTINGS = Object.freeze({
  // DownloadOptions defaults
  format: 'mp3',
  quality: '320',
  outputDir: null,
  filenameTemplate: '%(artist,uploader)s - %(title)s.%(ext)s',
  embedMetadata: true,
  embedThumbnail: true,
  embedChapters: false,
  playlist: 'single',
  playlistSubfolder: true,
  cookiesFromBrowser: false,
  sponsorblockRemove: true,
  normalize: false,
  trimSilence: true,
  overwrite: false,
  rateLimit: null,
  analyze: true,
  appendBpmKey: true,
  keyNotation: 'standard',
  bpmKeySuffix: ' [{bpm} BPM {key}]',
  writeBpmKeyTags: true,
  bpmRange: [70, 180],

  // Extension behaviour
  maxConcurrent: 3,
  notifyOnComplete: true,
  notifyOnError: true,
  autoOpenFolder: false,
  dropOverlay: true,       // show the floating drag tray when a download finishes
  sendSiteCookies: true,   // export current-site cookies to host for login-gated content
  siteButtons: { youtube: true, youtubeMusic: true, soundcloud: true, bandcamp: true },
  theme: 'system',         // 'system' | 'light' | 'dark'
});

const DOWNLOAD_OPTION_KEYS = [
  'format', 'quality', 'outputDir', 'filenameTemplate', 'embedMetadata', 'embedThumbnail',
  'embedChapters', 'playlist', 'playlistSubfolder', 'cookiesFromBrowser', 'sponsorblockRemove',
  'normalize', 'trimSilence', 'overwrite', 'rateLimit',
  'analyze', 'appendBpmKey', 'keyNotation', 'bpmKeySuffix', 'writeBpmKeyTags', 'bpmRange',
];

export async function getSettings() {
  const { settings } = await chrome.storage.sync.get('settings');
  return mergeSettings(settings);
}

export function mergeSettings(stored) {
  const s = { ...DEFAULT_SETTINGS, ...(stored || {}) };
  s.siteButtons = { ...DEFAULT_SETTINGS.siteButtons, ...((stored && stored.siteButtons) || {}) };
  return s;
}

let settingsWrites = Promise.resolve();
export function setSettings(patch) {
  settingsWrites = settingsWrites.catch(() => {}).then(async () => {
    const previous = await getSettings();
    const next = mergeSettings({ ...previous, ...patch, siteButtons: { ...previous.siteButtons, ...patch.siteButtons } });
    await chrome.storage.sync.set({ settings: next });
    return next;
  });
  return settingsWrites;
}

/** Extract DownloadOptions from settings, then apply per-download overrides. */
export function toDownloadOptions(settings, overrides = {}) {
  overrides ??= {};
  const o = {};
  for (const k of DOWNLOAD_OPTION_KEYS) o[k] = settings[k];
  o.trim = null;
  // Only protocol DownloadOptions may enter persisted jobs/history.
  for (const k of Object.keys(o)) if (Object.hasOwn(overrides, k)) o[k] = overrides[k];
  return o;
}
