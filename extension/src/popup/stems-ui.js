import { stemPaths, latestStem, activeStem } from '../shared/stems.js';
import { CMD, send } from '../shared/protocol.js';

const node = (tag, text) => { const el = document.createElement(tag); if (text) el.textContent = text; return el; };
export function stemStatus(job) {
  if (job.state === 'done') return 'Four stems ready · original kept';
  if (job.state === 'error') return job.error?.message || 'Separation failed. Try again.';
  if (job.state === 'cancelled') return 'Stem separation cancelled';
  if (job.cancelRequested) return 'Cancelling stems · waiting for the engine to stop';
  if (job.state === 'queued') return 'Stems queued';
  const stage = job.stage || '';
  const readable = stage.replace(/[_-]/g, ' ');
  const phases = { 'checking-gpu': 'Checking GPU support', 'separating-gpu': 'Separating stems · GPU', 'cpu-fallback': 'Switching to CPU · this may take longer', 'separating-cpu': 'Separating stems · CPU', 'downloading-engine': 'Downloading stem engine', 'downloading-model': 'Downloading stem model', decoding: 'Preparing audio' };
  const label = phases[stage] || (/download|setup|install|model|engine/i.test(stage) ? `Setting up stems · ${readable}` : /separat/i.test(stage) ? 'Separating stems' : stage ? readable : 'Preparing stems');
  return label + (typeof job.progress === 'number' ? ` · ${Math.round(Math.max(0, Math.min(1, job.progress)) * 100)}%` : '');
}
async function explain() {
  if ((await chrome.storage.local.get('stemsExplained')).stemsExplained) return true;
  return new Promise(resolve => {
    const dialog = node('dialog'); dialog.className = 'stems-explanation';
    dialog.setAttribute('aria-labelledby', 'stems-heading');
    const heading = node('h2', 'Split into four stems'); heading.id = 'stems-heading';
    dialog.append(heading, node('p', 'The first split downloads the separation engine and model. This needs an internet connection and disk space, and may take a while.'), node('p', 'Separation runs locally on your computer. Vocals, drums, bass and other are saved beside your audio. Your original file is kept.'));
    const finish = value => { dialog.close(); dialog.remove(); resolve(value); };
    const cancel = node('button', 'Not now'), start = node('button', 'Download setup & split');
    cancel.type = start.type = 'button';
    cancel.onclick = () => finish(false);
    start.onclick = async () => { try { await chrome.storage.local.set({ stemsExplained: true }); } catch { /* Explanation can be shown again next time. */ } finish(true); };
    dialog.addEventListener('cancel', event => { event.preventDefault(); finish(false); });
    dialog.append(cancel, start); document.body.append(dialog); dialog.showModal(); cancel.focus();
  });
}
const selections = new Map();
export function stemControls(item, state, report, onSelection = () => {}, explicitJob) {
  const paths = stemPaths(item);
  const path = paths.length === 1 ? paths[0] : paths.find(path => path === selections.get(item.id));
  const box = node('div'); box.className = 'stem-controls';
  // Do not let the history row's native pointer-drag swallow these controls.
  box.addEventListener('pointerdown', event => event.stopPropagation());
  if (!explicitJob && paths.length > 1) {
    const picker = node('select'); picker.setAttribute('aria-label', `Choose track for stems: ${item.title || 'download'}`);
    picker.dataset.key = `stem-track-${item.id}`;
    const placeholder = node('option', 'Choose a track to split…'); placeholder.value = ''; picker.append(placeholder);
    for (const file of paths) { const option = node('option', file.split(/[\\/]/).pop()); option.value = file; picker.append(option); }
    picker.value = path || '';
    picker.onchange = () => { selections.set(item.id, picker.value); onSelection(); };
    box.append(picker);
    if (!path) return box;
  }
  const job = explicitJob || latestStem(item, path, state.stems);
  if (job) box.dataset.stemJob = job.jobId;
  const active = job && ['queued', 'running'].includes(job.state);
  const button = (text, key, handler, disabled = false) => {
    const b = node('button', text); b.type = 'button'; b.dataset.key = `stem-${key}-${explicitJob?.jobId || item.id}`; b.disabled = disabled;
    b.onclick = async () => { b.disabled = true; try { await handler(); } catch (error) { report(error.message || 'Stem separation failed. Try again.'); } finally { if (b.isConnected) b.disabled = disabled; } };
    box.append(b); return b;
  };
  const connected = state.host?.state === 'ready';
  if (job) { const status = node('p', stemStatus(job)); status.className = 'stem-status'; status.setAttribute('role', 'status'); box.append(status); }
  if (active) {
    button('Cancel split', 'cancel', () => send(CMD.CANCEL_STEMS, { jobId: job.jobId }), !connected || job.cancelRequested);
  } else if (job?.state === 'done') {
    button('Stems folder', 'folder', () => send(CMD.OPEN, { path: job.outputDir }), !connected || !job.outputDir);
    button('Drag all stems', 'drag', () => send(CMD.SHOW_STEMS, { jobId: job.jobId }), !connected || !state.host.dropzone || job.files?.length !== 4);
  } else {
    button(job ? 'Retry split' : 'Split stems', 'split', async () => { if (await explain()) await send(CMD.SPLIT_STEMS, { id: item.id, ...(paths.length > 1 ? { path } : {}) }); }, !connected || state.host.stems !== true || item.analysisPending > 0);
  }
  if (!connected) box.append(node('p', 'Reconnect the audio engine to use stems.'));
  else if (state.host.stems !== true) box.append(node('p', 'Stems need the latest Siphon helper. Update using the latest installer, then reconnect in Settings → Connection & setup.'));
  else if (item.analysisPending > 0) box.append(node('p', 'Available after tempo & key analysis finishes.'));
  return box;
}

// Show only active jobs without controls in the visible history (including filtered/orphan jobs).
export function renderActiveStems(container, state, report, historyRoot) {
  const represented = new Set([...historyRoot.querySelectorAll('[data-stem-job]')].map(el => el.dataset.stemJob));
  const jobs = (state.stems || []).filter(job => activeStem(job) && !represented.has(job.jobId));
  container.replaceChildren(); container.hidden = !jobs.length;
  if (!jobs.length) return;
  container.append(node('h2', 'Active stems'));
  for (const job of jobs) {
    const row = node('article'); row.className = 'active-stem';
    row.append(node('p', job.sourceFile?.split(/[\\/]/).pop() || job.title || 'Audio'));
    row.append(stemControls({ id: job.sourceId, file: job.sourceFile }, state, report, undefined, job));
    container.append(row);
  }
}
