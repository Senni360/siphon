// Shared contract between popup / options / content scripts and the service worker.
// Keep protocol changes backward compatible.

export const HOST_NAME = 'com.siphon.host';

// Native host request types (browser -> host)
export const HOST = Object.freeze({
  HELLO: 'hello',
  PROBE: 'probe',
  ANALYZE: 'analyze',
  PICK_FILE: 'pickFile',
  DOWNLOAD: 'download',
  CANCEL: 'cancel',
  REVEAL: 'reveal',
  OPEN: 'open',
  PICK_FOLDER: 'pickFolder',
  UPDATE: 'update',
  LIST_JOBS: 'listJobs',
  SHOW_DROP: 'showDrop',
  GRAB: 'grab',
});

// Internal commands (UI/content -> service worker), sent as { cmd, ...args }
export const CMD = Object.freeze({
  GET_STATE: 'getState',            // -> { host, jobs, history, settings }
  PROBE: 'probe',                   // { url } -> MediaInfo
  ANALYZE: 'analyze',               // { url } -> { analysis, title? }
  ANALYZE_TAB: 'analyzeTab',         // { tabId? } -> { analysis, title? }
  ANALYZE_FILE: 'analyzeFile',       // { path? } -> { analysis, title?, path? } | null
  PROBE_TAB: 'probeTab',            // { tabId? } -> { url, info|null, error|null }
  DOWNLOAD: 'download',             // { url, options?: Partial<DownloadOptions>, title?, source? } -> { jobId }
  DOWNLOAD_TAB: 'downloadTab',      // { tabId?, options? } -> { jobId }
  CANCEL: 'cancel',                 // { jobId }
  RETRY: 'retry',                   // { jobId }  (history item or failed job)
  REVEAL: 'reveal',                 // { path }
  OPEN: 'open',                     // { path }
  PICK_FOLDER: 'pickFolder',        // {} -> { path|null }
  UPDATE_HOST: 'updateHost',        // {} -> { before, after, restartRequired? }; worker restarts safely
  RECONNECT_HOST: 'reconnectHost',  // {} -> host status
  CLEAR_HISTORY: 'clearHistory',    // { ids? } (omit = all)
  SHOW_DROP: 'showDrop',            // { id } -> { shown }; pins a history item in the drag tray
  GRAB: 'grab',                     // { id } -> { started }; continue a pointer drag as a native file drag
  GET_SETTINGS: 'getSettings',
  SET_SETTINGS: 'setSettings',      // { patch }
});

// Broadcasts (service worker -> everyone), sent as { evt, ... }
export const EVT = Object.freeze({
  JOBS: 'jobs',         // { jobs: JobState[] }; active snapshot + terminal transitions once
  HOST: 'host',         // { status: HostStatus }
  HISTORY: 'history',   // { history: HistoryItem[] }
  SETTINGS: 'settings', // { settings }
});

export const JOB_STATE = Object.freeze({
  QUEUED: 'queued', PROBING: 'probing', DOWNLOADING: 'downloading', CONVERTING: 'converting',
  TAGGING: 'tagging', ANALYZING: 'analyzing', DONE: 'done', ERROR: 'error', CANCELLED: 'cancelled',
});

export const TERMINAL_STATES = new Set([JOB_STATE.DONE, JOB_STATE.ERROR, JOB_STATE.CANCELLED]);

export const ERR = Object.freeze({
  HOST_NOT_FOUND: 'HOST_NOT_FOUND', UNSUPPORTED_URL: 'UNSUPPORTED_URL', NETWORK: 'NETWORK',
  GEO_BLOCKED: 'GEO_BLOCKED', LOGIN_REQUIRED: 'LOGIN_REQUIRED', AGE_RESTRICTED: 'AGE_RESTRICTED',
  DRM: 'DRM', LIVE_NOT_SUPPORTED: 'LIVE_NOT_SUPPORTED', FFMPEG_MISSING: 'FFMPEG_MISSING',
  CANCELLED: 'CANCELLED', DISK: 'DISK', BAD_REQUEST: 'BAD_REQUEST', INTERNAL: 'INTERNAL',
});

/**
 * HostStatus = {
 *   state: 'connecting' | 'ready' | 'missing' | 'error',
 *   hostVersion?, ytdlpVersion?, ffmpeg?: {found, version, path}, jsRuntime?, defaultOutputDir?,
 *   error?: { code, message }, restartRequired?: bool  // restart deferred until jobs finish
 * }
 * JobState = host `job` event fields + { url, source, options, createdAt, updatedAt }
 * HistoryItem = { id (=jobId), url, title, thumbnail, file, files, format, state, error, createdAt, finishedAt, source, analysis? }
 */

/** Promise wrapper for sending a command to the service worker. Throws on { error }. */
export async function send(cmd, args = {}) {
  const res = await chrome.runtime.sendMessage({ cmd, ...args });
  if (res && res.error) {
    const e = new Error(res.error.message || String(res.error));
    e.code = res.error.code;
    throw e;
  }
  return res;
}
