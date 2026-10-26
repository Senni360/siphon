import { stemControls, renderActiveStems } from './stems-ui.js';
import { CMD, EVT, TERMINAL_STATES, send } from '../shared/protocol.js';
import { getSettings, mergeSettings, toDownloadOptions } from '../shared/settings.js';
import { formatDuration, formatSpeed, formatEta, formatPercent, parseTime } from '../shared/ui/format.js';
import { friendlyError as errorText } from '../shared/errors.js';

import { analysisChips } from '../shared/ui/music.js';
import { formatBpm, formatKey } from '../shared/music.js';
import { normalizeMediaUrl } from '../shared/urls.js';
import { enableGrab } from '../shared/grab.js';
import { serviceMark, serviceOf } from '../shared/ui/services.js';

const $ = id => document.getElementById(id);
const state = { settings: mergeSettings(), host: { state: 'connecting' }, info: null, probing: true, submitting: false, playlist: 'single' };
const paths = {
  music: 'M9 18V5l11-2v13M9 8l11-2M9 18a3 3 0 1 1-3-3c2 0 3 1 3 3Zm11-2a3 3 0 1 1-3-3c2 0 3 1 3 3Z',
  check: 'm5 12 4 4L19 6', close: 'm6 6 12 12M6 18 18 6',
  folder: 'M3 6h6l2 2h10v12H3ZM3 6V4h6l2 2',
  play: 'm8 5.5 10 6.5-10 6.5Z', drag: 'M9 5h.01M15 5h.01M9 12h.01M15 12h.01M9 19h.01M15 19h.01', retry: 'M4 9a8 8 0 1 1 0 7M4 3v6h6', alert: 'M12 8v5m0 3h.01M10.3 3.9 2.6 17.3A2 2 0 0 0 4.3 20h15.4a2 2 0 0 0 1.7-2.7L13.7 3.9a2 2 0 0 0-3.4 0Z',
};
function icon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', paths[name] || paths.music); svg.append(path); return svg;
}
function el(tag, className, text) {
  const node = document.createElement(tag); if (className) node.className = className; if (text != null) node.textContent = text; return node;
}
function message(text, success = false) {
  $('message').textContent = text; $('message').hidden = !text; $('message').classList.toggle('success', success);
  if (text && !success) $('message').scrollIntoView({ block: 'nearest' });
}
function updateButton() {
  const pasted = $('url').value.trim();
  const service = pasted ? serviceOf(pasted) : state.sourceService;
  if (service && service !== 'other') document.documentElement.dataset.service = service;
  else delete document.documentElement.dataset.service;
  $('clear-url').hidden = !pasted;
  $('analyze').disabled = state.analyzing || state.host.state !== 'ready' || state.info?.isLive || (!state.info && !state.analysis);
  $('download').disabled = state.submitting || state.host.state !== 'ready' || (!pasted && (!state.info || state.probing || state.info.isLive));
  $('download').querySelector('span').textContent = state.submitting ? 'Adding to queue…' : state.info?.isPlaylist && state.playlist === 'all' && !pasted ? 'Download playlist' : 'Download audio';
}
function renderSettings(settings) {
  state.settings = mergeSettings(settings);
  const theme = state.settings.theme;
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
  document.querySelectorAll('[data-format]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.format === state.settings.format)));
  $('quality').value = state.settings.quality;
  $('append-bpm-key').checked = state.settings.appendBpmKey;
  renderAnalysis();
  if (state.history) renderHistory(state.history);
  $('quality-label').hidden = ['flac', 'wav', 'best'].includes(state.settings.format);
}
let settingWrite = Promise.resolve();
function saveSetting(patch) {
  renderSettings({ ...state.settings, ...patch });
  settingWrite = settingWrite.catch(() => {}).then(() => send(CMD.SET_SETTINGS, { patch })).catch(error => message(`Could not remember this choice. ${errorText(error)}`));
}
function renderHost(host) {
  const wasReady = state.host.state === 'ready', hadTray = !!state.host.dropzone;
  state.host = host || { state: 'connecting' };
  if (state.history) renderHistory(state.history);
  const names = { ready: 'Engine ready', connecting: 'Connecting', missing: 'Setup needed', error: 'Engine error' };
  const node = $('host-status'); node.dataset.state = state.host.state;
  node.querySelector('span').textContent = names[state.host.state] || 'Connecting';
  node.title = [names[state.host.state], state.host.hostVersion && `Host ${state.host.hostVersion}`, state.host.ytdlpVersion && `yt-dlp ${state.host.ytdlpVersion}`, state.host.ffmpeg && `FFmpeg ${state.host.ffmpeg.found ? state.host.ffmpeg.version || 'available' : 'missing'}`, state.host.error?.message].filter(Boolean).join('\n');
  node.setAttribute('aria-label', node.title);
  $('setup').hidden = !['missing', 'error'].includes(state.host.state);
  $('setup').querySelector('strong').textContent = state.host.state === 'error' ? 'Your audio engine needs attention' : 'Connect your audio engine';
  $('setup').querySelector('p').textContent = state.host.state === 'error' ? errorText(state.host.error) : 'Install the Siphon host to start downloading.';
  updateButton();
  if (!wasReady && state.host.state === 'ready' && !state.info && !state.probing) probeTab();
}
function safeImage(value) {
  try { const url = new URL(value); return ['https:', 'http:', 'data:'].includes(url.protocol) ? url.href : null; } catch { return null; }
}
function setService(service, art) {
  // Everything tinted (button, meter, switch, focus) follows the tab's service.
  state.sourceService = service;
  if (service && service !== 'other') document.documentElement.dataset.service = service;
  else delete document.documentElement.dataset.service;
  const ambient = $('ambient');
  if (art) { ambient.style.setProperty('--art', `url("${art.replace(/["\\]/g, '')}")`); ambient.classList.add('on'); }
  else ambient.classList.remove('on');
}
function renderSource(info, error, url) {
  state.probing = false; state.info = info || null;
  const card = $('source'); card.replaceChildren(); card.setAttribute('aria-busy', 'false');
  card.className = 'source-card';
  const service = serviceOf(info?.webpageUrl || info?.url || url);
  setService(info ? service : url ? serviceOf(url) : null, info && safeImage(info.thumbnail));
  $('playlist-control').hidden = !info?.isPlaylist;
  if (!info) {
    card.classList.add('source-empty'); card.append(icon('music'));
    const copy = el('div'); copy.append(el('strong', '', 'Find something worth keeping'), el('p', '', 'Open a track or video, or paste a link below.')); card.append(copy);
    if (error && !['UNSUPPORTED_URL', 'HOST_NOT_FOUND'].includes(error.code)) message(errorText(error));
    updateButton(); return;
  }
  const fallback = el('div', 'cover cover-fallback'); fallback.append(icon('music'));
  const thumb = safeImage(info.thumbnail);
  if (thumb) {
    const image = el('img', 'cover'); image.src = thumb; image.alt = ''; image.addEventListener('error', () => image.replaceWith(fallback), { once: true }); card.append(image);
  } else card.append(fallback);
  const copy = el('div', 'source-copy'); const title = el('h3', 'source-title', info.title || 'Untitled audio'); title.title = info.title || 'Untitled audio';
  const meta = el('div', 'source-meta'); meta.append(serviceMark(service), el('span', 'uploader', info.uploader || 'Audio source'));
  if (info.isLive || info.duration) meta.append(el('span', 'duration', info.isLive ? 'Live' : formatDuration(info.duration)));
  copy.append(title, meta); card.append(copy);
  if (info.isPlaylist) $('playlist-count').textContent = `Playlist · ${info.entryCount ?? info.entries?.length ?? 'Multiple'} tracks`;
  if (info.isLive) message(errorText({ code: 'LIVE_NOT_SUPPORTED' }));
  updateButton();
}
async function probeTab() {
  state.probing = true; updateButton();
  try { const result = await send(CMD.PROBE_TAB); renderSource(result?.info, result?.error, result?.url); if (result?.analysis) { state.analysis = result.analysis; renderAnalysis(); } }
  catch (error) { renderSource(null, error); }
}
function actionButton(label, name, handler, key) {
  const button = el('button', 'icon-button'); button.type = 'button'; button.title = label; button.setAttribute('aria-label', label); button.dataset.key = key; button.append(icon(name));
  button.addEventListener('click', async () => {
    button.disabled = true;
    try { await handler(); } catch (error) { message(errorText(error)); }
    finally { button.disabled = false; }
  }); return button;
}
function stageText(job) {
  const labels = { queued: 'Queued', probing: 'Finding audio', downloading: 'Downloading', converting: 'Converting', tagging: 'Tagging', analyzing: 'Analyzing tempo & key' };
  const parts = [labels[job.state] || labels[job.stage] || 'Working'];
  if (job.state === 'downloading') {
    if (job.progress != null) parts[0] += ` ${formatPercent(job.progress)}`;
    if (job.speed != null) parts.push(formatSpeed(job.speed));
    if (job.eta != null) parts.push(`${formatEta(job.eta)} left`);
  }
  if (job.item?.count) parts.push(`${job.item.index}/${job.item.count}`);
  return parts.join(' · ');
}
let lastAnnouncement = 0;
const jobRows = new Map();
function renderJobs(jobs = []) {
  const active = jobs.filter(job => !TERMINAL_STATES.has(job.state));
  const focusedKey = document.activeElement?.dataset.key;
  $('queue-section').hidden = !active.length; $('queue-count').textContent = active.length;
  const ids = new Set(active.map(job => job.jobId || job.id));
  for (const [id, row] of jobRows) if (!ids.has(id)) { row.node.remove(); jobRows.delete(id); }
  for (const [index, job] of active.entries()) {
    const id = job.jobId || job.id;
    const title = job.item?.title || job.title || 'Preparing audio';
    if (!jobRows.has(id)) {
      const node = el('article', 'job'), top = el('div', 'job-top'), copy = el('div', 'job-copy');
      node.dataset.service = serviceOf(job.url);
      const heading = el('h3', 'job-title'), stage = el('div', 'job-stage');
      const cancel = actionButton(`Cancel ${title}`, 'close', () => send(CMD.CANCEL, { jobId: id }), `cancel-${id}`);
      const meter = el('div', 'meter'), fill = el('div', 'meter-fill');
      meter.setAttribute('role', 'progressbar'); meter.setAttribute('aria-valuemin', '0'); meter.setAttribute('aria-valuemax', '100');
      copy.append(heading, stage); top.append(copy, cancel); meter.append(fill); node.append(top, meter);
      jobRows.set(id, { node, heading, stage, cancel, meter, fill });
    }
    const { node, heading, stage, cancel, meter, fill } = jobRows.get(id);
    heading.textContent = title; heading.title = title; stage.textContent = stageText(job);
    cancel.title = `Cancel ${title}`; cancel.setAttribute('aria-label', cancel.title);
    const determinate = job.state === 'downloading' && job.progress != null;
    meter.classList.toggle('indeterminate', !determinate); meter.setAttribute('aria-label', title); meter.setAttribute('aria-valuetext', stageText(job));
    if (determinate) { const percent = Math.max(0, Math.min(100, Number(job.progress) * 100 || 0)); meter.setAttribute('aria-valuenow', String(Math.round(percent))); fill.style.transform = `scaleX(${percent / 100})`; }
    else { meter.removeAttribute('aria-valuenow'); fill.style.removeProperty('transform'); }
    if ($('queue').children[index] !== node) $('queue').insertBefore(node, $('queue').children[index] || null);
  }
  if (focusedKey?.startsWith('cancel-') && !document.activeElement?.dataset.key) ($('queue').querySelector('button') || $('download')).focus({ preventScroll: true });
  if (Date.now() - lastAnnouncement > 5000) { $('progress-live').textContent = active.map(job => `${job.title || 'Audio'}: ${stageText(job)}`).join('. '); lastAnnouncement = Date.now(); }
}
function renderHistory(history = []) {
  state.history = history;
  const focusedKey = document.activeElement?.dataset.key;
  const refreshActive = () => {
    renderActiveStems($('active-stems'), state, text => message(text), $('recent'));
    if (focusedKey) ([...document.querySelectorAll('[data-key]')].find(control => control.dataset.key === focusedKey) || document.querySelector('[data-options="history"]'))?.focus({ preventScroll: true });
  };
  if (!history.length) { $('recent').replaceChildren(el('p', 'empty-recent', 'Downloads land here, ready to drag into your DAW.')); refreshActive(); return; }
  $('recent').replaceChildren();
  for (const item of history.slice(0, 6)) {
    const failed = item.state === 'error', cancelled = item.state === 'cancelled';
    const service = serviceOf(item.url);
    const row = el('article', `history-item${failed ? ' failed' : ''}`); row.dataset.service = service;
    if (!failed && !cancelled && item.file && state.host.dropzone) { row.dataset.grabId = item.id; row.title = 'Drag into FL Studio or any app'; }
    let thumb;
    const art = !failed && !cancelled && safeImage(item.thumbnail);
    if (art) { thumb = el('img', 'history-thumb'); thumb.src = art; thumb.alt = ''; thumb.loading = 'lazy'; thumb.referrerPolicy = 'no-referrer'; thumb.addEventListener('error', () => { const fallback = el('span', 'history-thumb'); fallback.append(icon('music')); thumb.replaceWith(fallback); }, { once: true }); }
    else { thumb = el('span', `history-thumb${failed ? ' failed' : ''}`); thumb.append(icon(failed ? 'alert' : cancelled ? 'close' : 'music')); }
    const copy = el('div', 'history-copy'); const title = el('h3', 'history-title', item.title || 'Untitled audio'); title.title = item.title || 'Untitled audio';
    const meta = el('div', 'history-meta'); meta.append(serviceMark(service));
    if (failed || cancelled) meta.append(el('span', '', failed ? 'Failed' : 'Cancelled'));
    else {
      meta.append(el('span', 'format', item.format === 'best' ? 'Original' : (item.format || 'Audio').toUpperCase()));
      const bpm = formatBpm(item.analysis?.bpm), key = formatKey(item.analysis, state.settings.keyNotation);
      if (bpm || key) meta.append(el('span', 'reading', [bpm && `${bpm} BPM`, key].filter(Boolean).join(' · ')));
    }
    copy.append(title, meta);
    if (item.state === 'done' && (item.file || item.files?.length)) copy.append(stemControls(item, state, text => message(text), () => renderHistory(state.history)));
    if (failed) copy.append(el('p', 'history-error', errorText(item.error)));
    const actions = el('div', 'history-actions');
    if (failed || cancelled) actions.append(actionButton(`Retry ${item.title || 'download'}`, 'retry', () => send(CMD.RETRY, { jobId: item.id || item.jobId }), `retry-${item.id}`));
    else {
      const path = item.file || item.files?.[0];
      if (path) actions.append(actionButton(`Show ${item.title || 'audio'} in folder`, 'folder', () => send(CMD.REVEAL, { path }), `reveal-${item.id}`), actionButton(`Play ${item.title || 'audio'}`, 'play', () => send(CMD.OPEN, { path }), `open-${item.id}`));
    }
    row.append(thumb, copy, actions); $('recent').append(row);
  }
  refreshActive();
}
function renderAnalysis() {
  const target = $('tab-analysis');
  target.setAttribute('aria-busy',String(!!state.analyzing));
  if (state.analyzing) target.textContent = 'Listening… ~10 s';
  else if (state.analysis) target.replaceChildren(analysisChips(state.analysis,state.settings.keyNotation,true));
  else target.textContent = state.analysisError || '';
}
$('analyze').addEventListener('click', async () => {
  state.analyzing = true; state.analysisError = ''; renderAnalysis(); updateButton();
  try { const result = await send(CMD.ANALYZE_TAB); state.analysis = result?.analysis; if (!state.analysis) state.analysisError = 'No tempo or key found. Try another track.'; }
  catch (error) { state.analysisError = errorText(error); state.analysis = null; }
  finally { state.analyzing = false; renderAnalysis(); updateButton(); }
});
$('append-bpm-key').addEventListener('change', () => saveSetting({appendBpmKey:$('append-bpm-key').checked}));
async function loadCachedAnalysis() {
  try {
    const [tab] = await chrome.tabs.query({active:true,currentWindow:true});
    let url = tab?.url;
    if (tab?.id !== undefined) { try { url = (await chrome.tabs.sendMessage(tab.id,{q:'siphon:getMedia'}))?.url || url; } catch { /* use page URL */ } }
    const {analyses = {}} = await chrome.storage.session.get('analyses');
    const result = analyses[normalizeMediaUrl(url)];
    if (result?.analysis && !state.analyzing && !state.analysis) { state.analysis = result.analysis; renderAnalysis(); updateButton(); }
  } catch { /* No stored analysis or no current tab. */ }
}
async function openOptions(hash) {
  try {
    if (!hash) await chrome.runtime.openOptionsPage();
    else await chrome.tabs.create({ url: chrome.runtime.getURL(`src/options/options.html#${hash}`) });
  } catch (error) { message(errorText(error)); }
}
$('settings').addEventListener('click', () => openOptions());
document.querySelectorAll('[data-options]').forEach(link => link.addEventListener('click', event => { event.preventDefault(); openOptions(link.dataset.options); }));
$('formats').addEventListener('click', event => { const format = event.target.closest('[data-format]')?.dataset.format; if (format) saveSetting({ format }); });
$('quality').addEventListener('change', () => saveSetting({ quality: $('quality').value }));
document.querySelectorAll('[data-playlist]').forEach(button => button.addEventListener('click', () => {
  state.playlist = button.dataset.playlist;
  document.querySelectorAll('[data-playlist]').forEach(b => b.setAttribute('aria-pressed', String(b === button))); updateButton();
}));
$('url').addEventListener('input', () => { message(''); updateButton(); });
$('clear-url').addEventListener('click', () => { $('url').value = ''; message(''); updateButton(); $('url').focus(); });
document.addEventListener('paste', event => {
  if (event.target === $('url')) return;
  const text = event.clipboardData?.getData('text').trim();
  if (/^https?:\/\//i.test(text || '')) { event.preventDefault(); $('url').value = text; $('url').focus(); message(''); updateButton(); }
});
async function download() {
  if ($('download').disabled) return;
  message('');
  const url = $('url').value.trim();
  if (url) { try { if (!['http:', 'https:'].includes(new URL(url).protocol)) throw new Error(); } catch { message('Paste a full http:// or https:// media link.'); $('url').focus(); return; } }
  let trim = null;
  if ($('trim').open) {
    const start = parseTime($('trim-start').value), end = parseTime($('trim-end').value);
    if (Number.isNaN(start) || Number.isNaN(end) || (end != null && end <= (start ?? 0))) { message('Check the trim range. Use mm:ss, with the end after the start.'); return; }
    if (start != null || end != null) trim = { start, end };
  }
  state.submitting = true; updateButton();
  try {
    const options = toDownloadOptions(state.settings, { playlist: state.playlist, trim });
    await send(url ? CMD.DOWNLOAD : CMD.DOWNLOAD_TAB, url ? { url, options, source: 'popup' } : { options });
    message('Added to your queue.', true); $('progress-live').textContent = 'Audio added to the download queue.';
  } catch (error) { message(errorText(error)); }
  finally { state.submitting = false; updateButton(); }
}
$('download-form').addEventListener('submit', event => { event.preventDefault(); download(); });
$('url').addEventListener('keydown', event => { if (event.key === 'Enter') { event.preventDefault(); download(); } });
document.addEventListener('keydown', event => {
  if (event.key === 'Enter' && !event.ctrlKey && !event.metaKey && !event.altKey && (event.target === document.body || event.target === document.documentElement)) { event.preventDefault(); download(); }
});
chrome.runtime.onMessage.addListener(event => {
  if (event.evt === EVT.STEMS) { state.stems = event.stems; renderHistory(state.history); }
  if (event.evt === EVT.JOBS) renderJobs(event.jobs);
  if (event.evt === EVT.HISTORY) renderHistory(event.history);
  if (event.evt === EVT.HOST) renderHost(event.status);
  if (event.evt === EVT.SETTINGS) renderSettings(event.settings);
});
async function init() {
  // Both start immediately; an unavailable host must not hold the whole interface hostage.
  // Local preferences can paint while GET_STATE waits for the native handshake.
  getSettings().then(renderSettings).catch(() => {});
  void loadCachedAnalysis();
  const probing = probeTab();
  try {
    const initial = await send(CMD.GET_STATE);
    state.stems = initial?.stems || [];
    renderSettings(initial?.settings); renderHost(initial?.host); renderJobs(initial?.jobs); renderHistory(initial?.history);
    if (state.settings.playlist === 'all') document.querySelector('[data-playlist="all"]').click();
  } catch (error) { renderHost({ state: 'error', error }); message(errorText(error)); }
  await probing;
}
enableGrab($('recent'));
init();
