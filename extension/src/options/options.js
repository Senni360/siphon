import { stemControls, renderActiveStems } from '../popup/stems-ui.js';
import { CMD, EVT, send } from '../shared/protocol.js';
import { mergeSettings } from '../shared/settings.js';
import { formatBytes } from '../shared/ui/format.js';
import { friendlyError } from '../shared/errors.js';
import { previewFilename, formatSuffix, SAMPLE_ANALYSIS } from '../shared/music.js';
import { analysisChips } from '../shared/ui/music.js';
import { enableGrab } from '../shared/grab.js';
import { serviceMark, serviceOf, SERVICE_NAMES } from '../shared/ui/services.js';

const $ = (selector) => document.querySelector(selector);
const escape = (value) => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const state = { settings: mergeSettings(), host: { state: 'connecting' }, history: [] };
const sections = {
  general: ['General', 'Make Siphon fit the way you collect audio.'],
  audio: ['Audio', 'Choose how your audio sounds, looks, and travels.'],
  analysis: ['Tempo & key', 'Know the pulse. Find the key. Keep both with your audio.'],
  sites: ['Sites & playlists', 'Bring Siphon to the places you listen.'],
  advanced: ['Advanced', 'Fine-tune the engine. See what’s happening locally.'],
  history: ['History', 'A record of the audio you’ve brought home.'],
  setup: ['Connection & setup', 'A few local tools. One seamless download.'],
  about: ['About Siphon', 'From the browser to your music folder.'],
};
const presets = [
  ['Artist - Title', '%(artist,uploader)s - %(title)s.%(ext)s'],
  ['Title', '%(title)s.%(ext)s'],
  ['Uploader - Title', '%(uploader)s - %(title)s.%(ext)s'],
  ['Playlist - NN - Title', '%(playlist_title)s - %(playlist_index)02d - %(title)s.%(ext)s'],
];
const formats = [
  ['mp3', 'MP3', 'Compatible with almost every player.'],
  ['m4a', 'M4A / AAC', 'Efficient files. Great for Apple devices.'],
  ['opus', 'Opus', 'Best quality per bit. Modern players.'],
  ['ogg', 'Ogg Vorbis', 'Open format. Broad desktop support.'],
  ['flac', 'FLAC', 'Lossless container, smaller than WAV.'],
  ['wav', 'WAV', 'Uncompressed. Large files for audio tools.'],
  ['best', 'Original', 'No re-encoding. The fastest route to the best source audio.'],
];
let current = 'general', ready = false, saveTimer, saveChain = Promise.resolve(), pending = {}, failed = {}, localEdits = {}, shortcuts = [], historyQuery = '', historyFilter = 'all', historySort = 'recent', historyAnalysis = 'all', updateResult = '', updating = false, clearArmed = false;
const version = chrome.runtime.getManifest().version;
$('#rail-version').textContent = `v${version}`;

function toggle(key, title, hint) {
  const checked = key.startsWith('siteButtons.') ? state.settings.siteButtons[key.split('.')[1]] : state.settings[key];
  const id = `setting-${key.replace('.', '-')}`;
  return `<div class="setting-row"><div class="setting-copy"><label class="setting-title" for="${id}">${title}</label><p id="${id}-hint">${hint}</p></div><input class="switch" type="checkbox" id="${id}" data-setting="${key}" aria-describedby="${id}-hint" ${checked ? 'checked' : ''}></div>`;
}
function select(key, items, extra = '') {
  return `<select id="setting-${key}" data-setting="${key}" ${extra}>${items.map(([value, label]) => `<option value="${value}" ${String(state.settings[key]) === value ? 'selected' : ''}>${label}</option>`).join('')}</select>`;
}
function row(title, hint, control, key) {
  return `<div class="setting-row"><div class="setting-copy"><label class="setting-title" for="setting-${key}">${title}</label><p>${hint}</p></div>${control}</div>`;
}
function group(title, body) { return `<section class="group"><h2>${title}</h2>${body}</section>`; }
function panel(body) { return `<div class="panel">${body}</div>`; }
function outputDirectory() { return state.settings.outputDir || state.host.defaultOutputDir || '%USERPROFILE%\\Music\\Siphon'; }
function filenamePreview() {
  return previewFilename(state.settings);
}
function general() {
  return group('Files & folders', panel(`<div class="panel-pad"><span class="field-label">Default output folder</span><div class="inline"><div class="folder-path" id="output-path">${escape(outputDirectory())}</div><button data-action="pick-folder">Choose…</button><button class="quiet" data-action="reset-folder">Reset</button></div><p class="hint">${state.settings.outputDir ? 'Custom folder. Reset to use the host’s default.' : 'Using the native host’s default music folder.'}</p></div>`)) +
    group('File naming', panel(`<div class="panel-pad"><label class="field-label" for="setting-filenameTemplate">Filename template</label><input id="setting-filenameTemplate" type="text" data-setting="filenameTemplate" spellcheck="false" value="${escape(state.settings.filenameTemplate)}"><div class="inline presets" aria-label="Filename presets">${presets.map(([label], i) => `<button data-preset="${i}">${label}</button>`).join('')}</div><div class="preview"><span>Example · Nils Frahm / Says</span><code id="filename-preview">${escape(filenamePreview())}</code></div><p class="hint">Uses yt-dlp template fields. Include <code>%(ext)s</code> to keep the correct extension.${state.settings.format === 'best' ? ' Original audio uses the source extension; WebM is an example.' : ''}</p></div>`)) +
    group('Download behaviour', panel(row('Concurrent downloads', 'How many downloads can run at the same time.', `<input id="setting-maxConcurrent" type="number" min="1" max="16" step="1" required data-setting="maxConcurrent" value="${escape(state.settings.maxConcurrent)}">`, 'maxConcurrent') + toggle('autoOpenFolder', 'Open folder after downloading', 'Show the output folder when a download completes.') + toggle('dropOverlay', 'Show the drag tray', 'When a download finishes, a small card appears at the bottom of the screen. Drag it into FL Studio or any DAW. Hover to keep it, click to pin it.'))) +
    group('Notifications', panel(toggle('notifyOnComplete', 'Download complete', 'Let me know when my audio is ready.') + toggle('notifyOnError', 'Download failed', 'Let me know when a download needs attention.'))) +
    group('Appearance', panel(row('Theme', 'Follow your system, or choose your own.', select('theme', [['system','System'],['dark','Dark'],['light','Light']]), 'theme')));
}
function audio() {
  return group('Audio format', `<div class="format-grid" role="radiogroup" aria-label="Audio format">${formats.map(([key,title,hint]) => `<label class="format-choice"><input type="radio" name="format" data-setting="format" value="${key}" ${state.settings.format === key ? 'checked' : ''}><strong>${title}</strong><span>${hint}</span></label>`).join('')}</div><p class="hint" style="margin-top:12px">Most online sources are already lossy. FLAC and WAV avoid further encoding loss; they cannot restore detail missing from the source.</p>`) +
    group('Encoding quality', panel(row('Quality', 'For lossy re-encoding. Higher bitrates create larger files.', select('quality', [['320','320 kbps'],['256','256 kbps'],['192','192 kbps'],['128','128 kbps'],['V0','V0 · variable'],['best','Best available']], ['best','flac','wav'].includes(state.settings.format) ? 'disabled' : ''), 'quality')) + `<p class="hint" id="quality-note">${['best','flac','wav'].includes(state.settings.format) ? 'Bitrate does not apply to this format.' : 'The engine maps this preference to the selected codec.'}</p>`) +
    group('Tags & artwork', panel(toggle('embedMetadata','Embed metadata','Keep title, artist, album, and other available tags.') + toggle('embedThumbnail','Embed cover artwork','Use the source thumbnail when the format supports it.') + toggle('embedChapters','Embed chapters','Preserve available chapter markers in supported formats.'))) +
    group('Audio processing', panel(toggle('trimSilence','Trim silence at the start','Cut leading silence so the file starts on its first sound, ready to line up in a DAW. Use WAV or FLAC for a sample-accurate start; MP3 adds a few milliseconds of encoder padding.') + toggle('normalize','Normalize loudness','Use EBU R128 normalization for more consistent listening levels.') + toggle('sponsorblockRemove','Remove non-music sections','On YouTube, remove music_offtopic segments using SponsorBlock.'))) + `<div class="callout"><strong>Keep the original, or process it.</strong>Original skips re-encoding by default. Loudness normalization and section removal may require audio processing. Metadata support varies by format.</div>`;
}
const suffixPresets = [' [{bpm} BPM {key}]', ' - {bpm}BPM - {camelot}', ' ({key}, {bpm})'];
let toolResult = null, toolBusy = false, toolError = '', toolLink = '';
function analysis() {
  return group('With every download', panel(toggle('analyze','Analyze tempo & key','Measure the saved audio locally. An analysis failure never interrupts your download.') + toggle('writeBpmKeyTags','Write BPM & key tags','Store tempo and musical key in supported audio tags.') + toggle('appendBpmKey','Add BPM & key to filename','Append the analysis before the file extension. Requires analysis to be enabled.'))) +
    group('Key notation', `<div class="notation-segments" role="radiogroup" aria-label="Key notation">${[['standard','Standard','Am'],['camelot','Camelot','8A'],['openkey','Open Key','1m']].map(([value,label,example]) => `<label><input type="radio" name="keyNotation" data-setting="keyNotation" value="${value}" ${state.settings.keyNotation === value ? 'checked' : ''}><span>${label} <strong>${example}</strong></span></label>`).join('')}</div><p class="hint">Standard uses familiar note names. Camelot and Open Key number the circle of fifths for harmonic mixing. Am, 8A, and 1m describe the same key.</p>`) +
    group('Filename suffix', panel(`<div class="panel-pad"><label class="field-label" for="setting-bpmKeySuffix">Suffix template</label><input id="setting-bpmKeySuffix" type="text" data-setting="bpmKeySuffix" value="${escape(state.settings.bpmKeySuffix)}" spellcheck="false" aria-describedby="suffix-help"><div class="inline presets">${suffixPresets.map((v,i) => `<button data-suffix-preset="${i}">${escape(v)}</button>`).join('')}</div><p class="hint" id="suffix-help">Use {bpm} (whole number), {bpmExact}, {key}, {camelot}, {openkey}, {keyShort}, or {keyLong}. {key} follows your notation. Windows-illegal characters become underscores.</p><div class="preview"><span>Example · 128 BPM / A minor</span><code id="filename-preview">${escape(filenamePreview())}</code></div></div>`)) +
    group('Tempo range', panel(`<div class="panel-pad"><div class="bpm-range"><label for="bpm-min">Minimum BPM<input id="bpm-min" data-bpm-index="0" type="number" min="30" max="360" step="1" required value="${state.settings.bpmRange[0]}"></label><span aria-hidden="true">—</span><label for="bpm-max">Maximum BPM<input id="bpm-max" data-bpm-index="1" type="number" min="30" max="360" step="1" required value="${state.settings.bpmRange[1]}"></label></div><p class="hint">Default 70–180 BPM. Half- and double-time estimates are folded into this range: a 64 BPM pulse may be shown as 128 BPM.</p></div>`)) +
    group('Try it without downloading', panel(`<div class="panel-pad analysis-tool"><h3>Analyze an audio file or link</h3><p class="hint">Analysis runs on your computer. Link analysis temporarily fetches a short audio sample.</p><button data-action="analyze-file" ${toolBusy ? 'disabled' : ''}>Analyze a file…</button><label class="field-label" for="analysis-link">Analyze a link</label><div class="inline"><input id="analysis-link" type="url" placeholder="https://…" value="${escape(toolLink)}"><button data-action="analyze-link" ${toolBusy ? 'disabled' : ''}>Analyze link</button></div><div id="analysis-result" role="status" aria-live="polite" aria-busy="${toolBusy}"></div></div>`));
}
function renderToolResult() {
  const target = $('#analysis-result'); if (!target) return;
  target.replaceChildren(); target.setAttribute('aria-busy',String(toolBusy));
  if (toolBusy) { target.textContent = 'Listening… ~10 s'; return; }
  if (toolError) { target.textContent = toolError; return; }
  if (!toolResult?.analysis) return;
  const title = document.createElement('strong'); title.textContent = toolResult.title || toolResult.path?.split(/[\\/]/).pop() || 'Audio analysis';
  target.append(title,analysisChips(toolResult.analysis,state.settings.keyNotation,true));
}
function sites() {
  return group('In-page download buttons', panel(toggle('siteButtons.youtube','YouTube','Show a download button on watch pages.') + toggle('siteButtons.youtubeMusic','YouTube Music','Keep a download button beside the player.') + toggle('siteButtons.soundcloud','SoundCloud','Download tracks, sets, and playlists in place.') + toggle('siteButtons.bandcamp','Bandcamp','Show download buttons on album and track pages.'))) +
    group('Playlists & albums', panel(row('When a link contains a playlist','Choose the default scope of a download.',select('playlist',[['single','Single track'],['all','All tracks']]),'playlist') + toggle('playlistSubfolder','Create a playlist subfolder','Keep each playlist or album together in its own folder.'))) +
    group('Logged-in content',panel(toggle('sendSiteCookies','Send site cookies for logged-in content','Send cookies for the current site when downloading private or age-restricted content.') + toggle('cookiesFromBrowser','Use Helium cookie store','Read Helium’s local cookie store instead. Turn off “Send site cookies” to use this option.'))) +
    `<div class="callout"><strong>Your cookies stay local.</strong>Cookies are passed only to the local Siphon host process, never to a Siphon server or another service. The host uses them to authenticate requests to the source site. Helium’s cookie encryption may prevent direct cookie-store access on Windows.</div>`;
}
function diagnostics() { return JSON.stringify(state.host, null, 2); }
function advanced() {
  return group('Download limits', panel(`<div class="panel-pad"><label class="field-label" for="setting-rateLimit">Maximum download rate</label><input id="setting-rateLimit" data-setting="rateLimit" type="text" value="${escape(state.settings.rateLimit || '')}" placeholder="Unlimited" spellcheck="false" aria-describedby="rate-hint"><p class="hint" id="rate-hint">Leave blank for unlimited. Use a value like <code>2M</code> (2 MiB/s) or <code>500K</code> (500 KiB/s).</p></div>` + toggle('overwrite','Overwrite existing files','Replace a file if the same output filename already exists.'))) +
    group('Download engine', panel(`<div class="setting-row"><div class="setting-copy"><span class="setting-title">yt-dlp <span class="muted" id="engine-version">${escape(state.host.ytdlpVersion || 'Version unavailable')}</span></span><p>Keep site support current with the latest engine.</p></div><button data-action="update-host" ${updating || state.host.state !== 'ready' ? 'disabled' : ''}>${updating ? 'Updating…' : 'Update engine'}</button></div><div class="panel-pad" id="update-result" role="status" ${updateResult ? '' : 'hidden'}>${escape(updateResult)}</div>`)) +
    group('Diagnostics', panel(`<div class="panel-pad"><div class="section-head"><h2>Native host status</h2><button data-action="copy-diagnostics">Copy JSON</button></div><pre id="diagnostics">${escape(diagnostics())}</pre><p class="hint">Local log file</p><div class="code-block"><code>%LOCALAPPDATA%\\Siphon\\logs\\host.log</code><button data-action="copy-log">Copy path</button></div><button data-action="reconnect">Reconnect host</button></div>`));
}
function dateLabel(item) {
  const date = new Date(item.finishedAt || item.createdAt);
  return Number.isNaN(date.valueOf()) ? 'Date unavailable' : date.toLocaleDateString(undefined, {year:'numeric',month:'short',day:'numeric'});
}
function siteLabel(item) {
  try { return new URL(item.url).hostname.replace(/^www\./,''); } catch { return item.source || 'Unknown site'; }
}
function thumbnail(url) { if (!url) return ''; try { const u = new URL(url, location.href); return ['https:','http:'].includes(u.protocol) ? escape(u.href) : ''; } catch { return ''; } }
function historyRows() {
  const focusedKey = document.activeElement?.dataset.key;
  const items = state.history.filter(item => (historyFilter === 'all' || item.state === historyFilter) && `${item.title} ${siteLabel(item)} ${SERVICE_NAMES[serviceOf(item)]} ${item.format} ${item.analysis?.bpm || ''} ${item.analysis?.key || ''} ${item.analysis?.keyShort || ''} ${item.analysis?.camelot || ''} ${item.analysis?.openKey || ''}`.toLowerCase().includes(historyQuery.toLowerCase()) && (historyAnalysis === 'all' || (historyAnalysis === 'analyzed' ? !!item.analysis : !item.analysis)));
  if (historySort !== 'recent') items.sort((a,b) => a.analysis?.bpm == null ? (b.analysis?.bpm == null ? 0 : 1) : b.analysis?.bpm == null ? -1 : (a.analysis.bpm - b.analysis.bpm) * (historySort === 'bpm-desc' ? -1 : 1));
  $('#history-count').textContent = `${items.length} of ${state.history.length} downloads`;
  $('#history-list').innerHTML = items.length ? items.map(item => {
    const path = item.file || item.files?.[0], thumb = thumbnail(item.thumbnail), id = escape(item.id || item.jobId), label = {done:'Complete',error:'Failed',cancelled:'Cancelled'}[item.state] || item.state;
    return `<article class="history-item" data-state="${escape(item.state)}" data-service="${serviceOf(item)}" ${path && item.state === 'done' && state.host.dropzone ? `data-grab-id="${id}" title="Drag into FL Studio or any app"` : ''}><div class="artwork" aria-hidden="true">${thumb ? `<img src="${thumb}" alt="" loading="lazy" referrerpolicy="no-referrer">` : '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 18V6l10-2v12M9 18a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 9 18Zm10-2a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 19 16Z"/></svg>'}</div><div><p class="history-title">${escape(item.title || 'Untitled download')}</p><div class="history-meta"><span data-service-mark title="${escape(siteLabel(item))}"></span><span class="mono">${escape(item.format === 'best' ? 'Original' : (item.format || 'Audio').toUpperCase())}</span><span>${escape(dateLabel(item))}</span>${item.size != null ? `<span class="mono">${formatBytes(item.size)}</span>` : ''}<span class="state ${escape(item.state)}">${escape(label)}</span></div><div class="history-analysis" data-analysis-id="${id}"></div>${item.error ? `<p class="history-error">${escape(friendlyError(item.error))}</p>` : ''}</div><div class="history-actions" data-id="${id}"><button data-action="drag" ${path && state.host.dropzone ? '' : 'disabled'} title="${state.host.dropzone ? 'Open in the drag tray' : 'The drag tray needs the latest Siphon installer'}" aria-label="Drag ${escape(item.title)} into a DAW">Drag</button><button data-action="reveal" ${path ? '' : 'disabled'} aria-label="Reveal ${escape(item.title)} in folder">Reveal</button><button data-action="open" ${path ? '' : 'disabled'} aria-label="Open ${escape(item.title)}">Open</button><button data-action="retry" ${item.url ? '' : 'disabled'} aria-label="Retry ${escape(item.title)}">Retry</button><button data-action="remove" class="quiet" aria-label="Remove ${escape(item.title)} from history">Remove</button></div></article>`;
  }).join('') : `<div class="empty"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 18V6l10-2v12M9 18a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 9 18Zm10-2a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 19 16Z"/></svg><h2>${state.history.length ? 'No matching downloads' : 'Your listening library starts here'}</h2><p>${state.history.length ? 'Try another search or change the status filter.' : 'Download audio from the toolbar. Your history will appear here.'}</p></div>`;
  document.querySelectorAll('[data-service-mark]').forEach(node => {
    const mark = serviceMark(node.closest('[data-service]').dataset.service);
    mark.title = node.title;
    if (mark.dataset.service === 'other') mark.lastChild.textContent = node.title;
    node.replaceWith(mark);
  });
  document.querySelectorAll('[data-analysis-id]').forEach(node => { const item = state.history.find(i => (i.id || i.jobId) === node.dataset.analysisId); if (item?.analysis) node.append(analysisChips(item.analysis,state.settings.keyNotation)); });
  document.querySelectorAll('[data-analysis-id]').forEach(node => { const item = state.history.find(i => i.id === node.dataset.analysisId); if (item?.state === 'done' && (item.file || item.files?.length)) node.after(stemControls(item, state, text => showError(new Error(text)), historyRows)); });
  renderActiveStems($('#active-stems'), state, text => showError(new Error(text)), $('#history-list'));
  $('#history-export-json').disabled = $('#history-export-csv').disabled = $('#clear-history').disabled = !state.history.length;
  if (focusedKey?.startsWith('stem-')) [...document.querySelectorAll('[data-key]')].find(button => button.dataset.key === focusedKey)?.focus({ preventScroll: true });
}
function history() {
  return `<section id="active-stems" aria-label="Active stems" hidden></section><div class="toolbar"><input type="search" id="history-search" aria-label="Search history" placeholder="Search title, site, BPM, or key" value="${escape(historyQuery)}"><select id="history-filter" aria-label="Filter history by status">${[['all','All states'],['done','Complete'],['error','Failed'],['cancelled','Cancelled']].map(([value,label]) => `<option value="${value}" ${historyFilter === value ? 'selected' : ''}>${label}</option>`).join('')}</select><select id="history-analysis-filter" aria-label="Filter history by analysis">${[['all','All analyses'],['analyzed','With BPM & key'],['missing','Without analysis']].map(([v,l]) => `<option value="${v}" ${historyAnalysis === v ? 'selected' : ''}>${l}</option>`).join('')}</select><select id="history-sort" aria-label="Sort history">${[['recent','Most recent'],['bpm-asc','BPM · low to high'],['bpm-desc','BPM · high to low']].map(([v,l]) => `<option value="${v}" ${historySort === v ? 'selected' : ''}>${l}</option>`).join('')}</select></div><p class="history-count" id="history-count"></p><div id="history-list" class="history-list"></div><div class="history-footer"><div class="inline"><button id="history-export-json" data-action="export-json">Export JSON</button><button id="history-export-csv" data-action="export-csv">Export CSV</button></div><button id="clear-history" class="danger quiet" data-action="clear-history">Clear history</button></div><p class="hint">Removing history keeps your downloaded files.</p>`;
}
const installCommand = 'powershell -NoProfile -ExecutionPolicy Bypass -File .\\installer\\install.ps1';
function step(n,title,body,complete) { return `<div class="step"><span class="step-number ${complete ? 'complete' : ''}" aria-label="${complete ? 'Complete' : `Step ${n}`}">${complete ? '✓' : n}</span><div><h3>${title}</h3>${body}</div></div>`; }
function setup() {
  const host = state.host, connected = host.state === 'ready', shortcut = shortcuts.find(c => c.name === 'download-current')?.shortcut;
  return panel(step(1,'Native host', connected ? `<p>Connected · Siphon ${escape(host.hostVersion || '')}</p><p class="hint">${escape(host.defaultOutputDir || '')}</p>` : `<p>${host.state === 'missing' ? 'Install the local host to save audio to your computer.' : host.state === 'connecting' ? 'Connecting to the local host…' : escape(friendlyError(host.error))}</p><div class="code-block"><code>${escape(installCommand)}</code><button data-action="copy-install">Copy</button></div><p>Run this in PowerShell from the extracted Siphon folder. You can also double-click <code>install.cmd</code> in the Siphon folder. Then reload the extension.</p><button class="primary" data-action="reconnect">Check again</button>`,connected) +
    step(2,'ffmpeg',`<p>${host.ffmpeg?.found ? `Found · ${escape(host.ffmpeg.version || 'Ready for audio conversion')}` : connected ? 'Not found. Install ffmpeg, then check again to enable conversion and tagging.' : 'Waiting for the native host to report audio-tool availability.'}</p>${connected && !host.ffmpeg?.found ? '<div class="code-block"><code>winget install Gyan.FFmpeg</code><button data-action="copy-ffmpeg">Copy</button></div><button data-action="reconnect">Check again</button>' : ''}`,!!host.ffmpeg?.found) +
    step(3,'JavaScript runtime',`<p>${host.jsRuntime ? `${escape(host.jsRuntime.name || 'Runtime')} ${escape(host.jsRuntime.version || '')} is available.` : connected ? 'Not found. Install Node.js or Deno and reconnect. A JavaScript runtime is needed for YouTube challenges.' : 'Waiting for the native host. YouTube needs a JavaScript runtime such as Node.js or Deno.'}</p>${connected && !host.jsRuntime ? '<div class="code-block"><code>winget install OpenJS.NodeJS.LTS</code><button data-action="copy-node">Copy</button></div><button data-action="reconnect">Check again</button>' : ''}`,!!host.jsRuntime) +
    step(4,'Set your shortcut',`<p>Download the current tab without opening the popup.</p><div class="inline">${shortcut ? `<kbd>${escape(shortcut)}</kbd>` : '<span class="muted">No shortcut assigned</span>'}<button data-action="shortcuts">Manage shortcuts ↗</button></div>`,!!shortcut) +
    step(5,'Keep Siphon within reach','<p>Open Helium’s extensions menu in the toolbar, then pin Siphon. One click opens your audio controls.</p>',false)) + `<div class="group" style="margin-top:24px"><label class="field-label">Extension ID</label><div class="code-block"><code>${escape(chrome.runtime.id)}</code><button data-action="copy-id">Copy</button></div></div>`;
}
function about() {
  return `<div class="about-brand"><svg class="about-mark" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 4v8a6 6 0 0 0 12 0V4M10 4v8a2 2 0 0 0 4 0V4"/></svg><div><h2>Siphon</h2><p class="muted">Version ${escape(version)} · Made for Helium</p></div></div><p class="prose">Save the audio you want to keep. Siphon connects your browser to a local download engine, with thoughtful defaults for files, tags, and artwork.</p>${group('A wide-open listening world', '<p class="prose">YouTube, YouTube Music, SoundCloud, Bandcamp, Mixcloud, Vimeo, and 1000+ sites supported by yt-dlp. Availability depends on the source site and may change as sites update.</p>')}${group('Local by design','<p class="prose">Downloads and audio conversion run on your computer through the Siphon native host, yt-dlp, and ffmpeg. Siphon does not run a download server.</p>')}${group('Download responsibly','<p class="prose">For personal use and content you own, have permission to download, or otherwise have rights to use. Respect creators, copyright, and each site’s terms. Siphon does not bypass DRM.</p>')}`;
}
function applyTheme() { document.documentElement.dataset.theme = state.settings.theme; }
function syncControls() {
  document.querySelectorAll('[data-setting]').forEach(control => {
    if (control === document.activeElement) return;
    const key = control.dataset.setting;
    const value = key.startsWith('siteButtons.') ? state.settings.siteButtons[key.split('.')[1]] : state.settings[key];
    if (control.type === 'checkbox') control.checked = !!value;
    else if (control.type === 'radio') control.checked = control.value === value;
    else { control.value = value ?? ''; control.setCustomValidity?.(''); }
  });
  if ($('#filename-preview')) $('#filename-preview').textContent = filenamePreview();
  document.querySelectorAll('[data-bpm-index]').forEach(input => { if (input !== document.activeElement) input.value = state.settings.bpmRange[Number(input.dataset.bpmIndex)]; });
  renderToolResult();
  if ($('#output-path')) {
    $('#output-path').textContent = outputDirectory();
    $('#output-path').closest('.panel-pad').querySelector('.hint').textContent = state.settings.outputDir ? 'Custom folder. Reset to use the host’s default.' : 'Using the native host’s default music folder.';
  }
  if ($('#setting-quality')) {
    const disabled = ['best','flac','wav'].includes(state.settings.format);
    $('#setting-quality').disabled = disabled;
    $('#quality-note').textContent = disabled ? 'Bitrate does not apply to this format.' : 'The engine maps this preference to the selected codec.';
  }
}
function renderHost() {
  $('#host-dot').className = `status-dot ${state.host.state}`;
  $('#host-label').textContent = {ready:'Local host connected',missing:'Host installation needed',error:'Host needs attention',connecting:'Connecting to host…'}[state.host.state] || 'Host status unavailable';
  if (!ready) return;
  if (current === 'setup') render();
  if (current === 'advanced') render();
  if (current === 'history') historyRows();
  if (current === 'general') $('#output-path').textContent = outputDirectory();
}
function render() {
  if (!ready) return;
  const hash = location.hash.slice(1);
  current = sections[hash] ? hash : 'general';
  if (!sections[hash]) historyReplace('#general');
  const [title, description] = sections[current];
  $('#page-title').textContent = title;
  $('#page-description').textContent = description;
  document.title = `${title} · Siphon`;
  document.querySelectorAll('nav a').forEach(a => a.hash === `#${current}` ? a.setAttribute('aria-current','page') : a.removeAttribute('aria-current'));
  $('#content').innerHTML = ({general,audio,analysis,sites,advanced,history,setup,about})[current]();
  $('#content').setAttribute('aria-busy','false');
  if (current === 'history') historyRows();
  if (current === 'analysis') renderToolResult();
}
function historyReplace(hash) { window.history.replaceState(null,'',hash); }
function showError(error) {
  $('#notice').hidden = false;
  $('#notice').textContent = friendlyError(error);
  if (Object.keys(failed).length) { const b = document.createElement('button'); b.textContent = 'Retry saving'; b.dataset.action = 'retry-save'; $('#notice').append(b); }
}
function clearError() { $('#notice').hidden = true; }
function saveStatus(text, isError = false) { $('#save-status').textContent = text; $('#save-status').classList.toggle('failed',isError); }
function queueSave(patch, delayed = false) {
  state.settings = mergeSettings({...state.settings,...patch});
  pending = {...pending,...patch};
  localEdits = {...localEdits,...patch};
  applyTheme();
  saveStatus('Saving…');
  clearTimeout(saveTimer);
  if (delayed) saveTimer = setTimeout(flushSave,400); else flushSave();
}
function flushSave() {
  clearTimeout(saveTimer);
  if (!Object.keys(pending).length) return saveChain;
  const patch = pending; pending = {};
  saveChain = saveChain.then(async () => {
    try {
      await send(CMD.SET_SETTINGS,{patch});
      for (const key of Object.keys(patch)) {
        delete failed[key];
        if (JSON.stringify(localEdits[key]) === JSON.stringify(patch[key])) delete localEdits[key];
      }
      if (!Object.keys(localEdits).length) saveStatus('✓ Saved');
      if (!Object.keys(failed).length) clearError();
    } catch (e) { failed = {...failed,...patch}; saveStatus('Changes not saved',true); showError(e); }
  });
  return saveChain;
}
function updateControl(target) {
  if (!target.checkValidity()) { target.reportValidity(); return; }
  let key = target.dataset.setting, value = target.type === 'checkbox' ? target.checked : target.value;
  if (key === 'maxConcurrent') {
    value = Number(value);
    if (!Number.isInteger(value) || value < 1 || value > 16) return;
  }
  if (key === 'filenameTemplate' && (!value.trim() || value.length > 2048 || /[\/\\\0\r\n]/.test(value) || ['.','..'].includes(value))) { target.setCustomValidity('Enter a filename without directory separators. Use the playlist subfolder setting for folders.'); target.reportValidity(); return; }
  if (key === 'bpmKeySuffix') {
    try { formatSuffix(SAMPLE_ANALYSIS,value,state.settings.keyNotation); }
    catch (error) { target.setCustomValidity(error.message); target.reportValidity(); return; }
  }
  if (key === 'rateLimit') {
    value = value.trim().replace(/^\./,'0.');
    const match = /^(\d+(?:\.\d+)?)\s*([KMGT]?)(?:i?B)?$/i.exec(value);
    const bytes = match ? Number(match[1]) * 1024 ** (match[2] ? 'KMGT'.indexOf(match[2].toUpperCase()) + 1 : 0) : NaN;
    if (value && (!Number.isFinite(bytes) || bytes < 1 || bytes > 2 ** 63 - 1)) { target.setCustomValidity('Use a positive rate like 2M or 500K, or leave blank.'); target.reportValidity(); return; }
    value ||= null;
  }
  const patch = key.startsWith('siteButtons.') ? {siteButtons:{...state.settings.siteButtons,[key.split('.')[1]]:value}} : {[key]:value};
  queueSave(patch,['text','search','number'].includes(target.type));
  if ($('#filename-preview')) $('#filename-preview').textContent = filenamePreview();
  if (key === 'keyNotation') renderToolResult();
  if (key === 'format') {
    $('#setting-quality').disabled = ['best','flac','wav'].includes(value);
    $('#quality-note').textContent = ['best','flac','wav'].includes(value) ? 'Bitrate does not apply to this format.' : 'The engine maps this preference to the selected codec.';
  }
}
document.addEventListener('input', e => {
  if (e.target.id === 'analysis-link') toolLink = e.target.value;
  else if (e.target.id === 'history-search') { historyQuery = e.target.value; historyRows(); }
  else if (e.target.dataset.setting && ['text','number'].includes(e.target.type)) { e.target.setCustomValidity(''); updateControl(e.target); }
});
document.addEventListener('change', e => {
  if (e.target.id === 'bpm-min' || e.target.id === 'bpm-max') {
    const min = $('#bpm-min'), max = $('#bpm-max');
    min.setCustomValidity(''); max.setCustomValidity('');
    if (!min.checkValidity() || !max.checkValidity()) { (!min.checkValidity() ? min : max).reportValidity(); return; }
    if (+min.value >= +max.value) { e.target.setCustomValidity('Minimum BPM must be lower than maximum BPM.'); e.target.reportValidity(); return; }
    queueSave({bpmRange:[+min.value,+max.value]});
  }
  else if (e.target.id === 'history-sort') { historySort = e.target.value; historyRows(); }
  else if (e.target.id === 'history-analysis-filter') { historyAnalysis = e.target.value; historyRows(); }
  else if (e.target.id === 'history-filter') { historyFilter = e.target.value; historyRows(); }
  else if (e.target.dataset.setting && !['text','number'].includes(e.target.type)) updateControl(e.target);
});
document.addEventListener('focusout', e => { if (e.target.dataset.setting) requestAnimationFrame(syncControls); });
document.addEventListener('error', e => {
  if (e.target.matches?.('.artwork img')) {
    e.target.parentElement.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 18V6l10-2v12M9 18a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 9 18Zm10-2a2.5 2.5 0 1 1-2.5-2.5A2.5 2.5 0 0 1 19 16Z"/></svg>';
  }
}, true);
async function copy(value, button) {
  await navigator.clipboard.writeText(value);
  const previous = button.textContent; button.textContent = 'Copied'; setTimeout(() => { if (button.isConnected) button.textContent = previous; },1600);
}
function exportHistory(type) {
  const columns = ['id','title','url','format','state','file','createdAt','finishedAt','bpm','key','keyShort','camelot','openKey','bpmConfidence','keyConfidence'];
  const csvValue = value => { let text = String(value ?? ''); if (/^[=+@\-\t\r]/.test(text)) text = `'${text}`; return `"${text.replace(/"/g,'""')}"`; };
  const text = type === 'json' ? JSON.stringify(state.history,null,2) : '\uFEFF' + [columns.map(csvValue).join(','),...state.history.map(item => columns.map(k => csvValue(item[k] ?? item.analysis?.[k])).join(','))].join('\r\n');
  const url = URL.createObjectURL(new Blob([text],{type:type === 'json' ? 'application/json' : 'text/csv;charset=utf-8'}));
  const link = document.createElement('a'); link.href = url; link.download = `siphon-history-${new Date().toISOString().slice(0,10)}.${type}`; link.click(); setTimeout(() => URL.revokeObjectURL(url),5000);
}
document.addEventListener('click', async e => {
  if (e.target.closest('.skip')) { e.preventDefault(); $('#main').focus(); return; }
  const button = e.target.closest('button'); if (!button) return;
  if (button.dataset.suffixPreset != null) { queueSave({bpmKeySuffix:suffixPresets[Number(button.dataset.suffixPreset)]}); syncControls(); return; }
  if (button.dataset.preset != null) { queueSave({filenameTemplate:presets[Number(button.dataset.preset)][1]}); $('#setting-filenameTemplate').value = state.settings.filenameTemplate; $('#filename-preview').textContent = filenamePreview(); return; }
  const action = button.dataset.action; if (!action) return;
  const item = state.history.find(i => (i.id || i.jobId) === button.closest('[data-id]')?.dataset.id);
  button.disabled = true;
  try {
    if (action !== 'retry-save') clearError();
    if (action === 'analyze-file' || action === 'analyze-link') {
      if (toolBusy) return;
      const input = $('#analysis-link');
      if (action === 'analyze-link' && (!input.value.trim() || !input.checkValidity() || !/^https?:\/\//i.test(input.value))) { input.setCustomValidity('Paste a full HTTP or HTTPS media link.'); input.reportValidity(); input.addEventListener('input', () => input.setCustomValidity(''),{once:true}); return; }
      toolBusy = true; toolError = ''; renderToolResult();
      document.querySelectorAll('[data-action^=analyze-]').forEach(b => b.disabled = true);
      try {
        const result = await send(action === 'analyze-file' ? CMD.ANALYZE_FILE : CMD.ANALYZE, action === 'analyze-link' ? {url:input.value.trim()} : {});
        if (result) { toolResult = result; if (!result.analysis) toolError = 'No tempo or key found. Try another audio file.'; }
      } catch (error) { toolError = friendlyError(error); toolResult = null; }
      finally { toolBusy = false; renderToolResult(); document.querySelectorAll('[data-action^=analyze-]').forEach(b => b.disabled = false); }
    }
    if (action === 'pick-folder') { const result = await send(CMD.PICK_FOLDER); if (result?.path) { queueSave({outputDir:result.path}); render(); } }
    if (action === 'reset-folder') { queueSave({outputDir:null}); render(); }
    if (action === 'copy-diagnostics') await copy(diagnostics(),button);
    if (action === 'copy-install') await copy(installCommand,button);
    if (action === 'copy-log') await copy('%LOCALAPPDATA%\\Siphon\\logs\\host.log',button);
    if (action === 'copy-ffmpeg') await copy('winget install Gyan.FFmpeg',button);
    if (action === 'copy-node') await copy('winget install OpenJS.NodeJS.LTS',button);
    if (action === 'copy-id') await copy(chrome.runtime.id,button);
    if (action === 'shortcuts') await chrome.tabs.create({url:'chrome://extensions/shortcuts'});
    if (action === 'reconnect') { button.textContent = 'Checking…'; const status = await send(CMD.RECONNECT_HOST); if (status?.state) state.host = status; renderHost(); }
    if (action === 'update-host') {
      updating = true; button.textContent = 'Updating…'; updateResult = 'Updating yt-dlp. This may take a minute.'; $('#update-result').hidden = false; $('#update-result').textContent = updateResult;
      try { const result = await send(CMD.UPDATE_HOST); updateResult = `${result?.before || state.host.ytdlpVersion || 'Previous version'} → ${result?.after || 'Updated'}${state.host.restartRequired ? ' · Engine restart will finish after the queue completes.' : ''}`; }
      catch (error) { updateResult += ` · ${friendlyError(error)}`; throw error; }
      finally { updating = false; renderHost(); }
    }
    if (action === 'open' || action === 'reveal') { if (item) await send(action === 'open' ? CMD.OPEN : CMD.REVEAL,{path:item.file || item.files?.[0]}); }
    if (action === 'drag' && item) await send(CMD.SHOW_DROP,{id:item.id || item.jobId});
    if (action === 'retry' && item) { await send(CMD.RETRY,{jobId:item.id || item.jobId}); button.textContent = 'Queued'; }
    if (action === 'remove' && item) { await send(CMD.CLEAR_HISTORY,{ids:[item.id || item.jobId]}); state.history = state.history.filter(i => i !== item); historyRows(); }
    if (action === 'clear-history') {
      if (!clearArmed) { clearArmed = true; button.textContent = 'Confirm clear history'; setTimeout(() => { clearArmed = false; if (button.isConnected) button.textContent = 'Clear history'; },5000); }
      else { await send(CMD.CLEAR_HISTORY); clearArmed = false; state.history = []; historyRows(); }
    }
    if (action.startsWith('export-')) exportHistory(action.split('-')[1]);
    if (action === 'retry-save') { queueSave({...failed,...localEdits,...pending}); await saveChain; }
  } catch (error) { showError(error); }
  finally { if (button.isConnected) button.disabled = button.id === 'clear-history' && !state.history.length; }
});
window.addEventListener('hashchange', () => { flushSave(); render(); });
window.addEventListener('pagehide', flushSave);
document.addEventListener('visibilitychange', () => { if (document.hidden) flushSave(); });
chrome.runtime.onMessage.addListener(message => {
  if (message.evt === EVT.STEMS) { state.stems = message.stems; if (current === 'history') historyRows(); }
  if (message.evt === EVT.HOST) { state.host = message.status; renderHost(); }
  if (message.evt === EVT.HISTORY) { state.history = message.history || []; if (current === 'history') historyRows(); }
  if (message.evt === EVT.SETTINGS) {
    // Pending local edits win over a broadcast from an earlier serialized save.
    state.settings = mergeSettings({...state.settings,...message.settings,...localEdits}); applyTheme();
    if (ready) { syncControls(); if (current === 'history') historyRows(); }
  }
});
async function init() {
  try {
    const [initial, commands] = await Promise.all([send(CMD.GET_STATE), chrome.commands.getAll().catch(() => [])]);
    if (!initial) throw new Error('The background service did not respond. Reload Siphon and try again.');
    Object.assign(state,initial); state.settings = mergeSettings(initial.settings); state.host ||= {state:'missing'}; state.history ||= []; shortcuts = commands;
    ready = true; applyTheme(); saveStatus('All changes saved'); render(); renderHost();
  } catch (error) { ready = true; state.host = {state:'error',error:{message:error.message}}; render(); renderHost(); saveStatus('Unable to load',true); showError(error); }
}
enableGrab($('#content'));
init();
