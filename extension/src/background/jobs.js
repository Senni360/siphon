import { EVT, HOST, TERMINAL_STATES } from '../shared/protocol.js';
import { toDownloadOptions } from '../shared/settings.js';
import { normalizeMediaUrl, siteForUrl } from '../shared/urls.js';
import { siteCookies } from './cookies.js';
import { broadcast, errorData, failure, timer } from './common.js';

export class Jobs {
  constructor(chrome, { native, getSettings, analysisCache, onTerminal = () => {}, onAnalysis = () => {}, onCount = () => {}, analysisTimeoutMs = 180_000 }) {
    Object.assign(this, { chrome, native, getSettings, analysisCache, onTerminal, onAnalysis, onCount, analysisTimeoutMs });
    this.jobs = new Map();
    this.history = [];
    this.writes = Promise.resolve();
    this.lastBroadcast = 0;
    this.terminalUpdates = new Map();
    // Finished jobs whose BPM/key the host is still computing (jobId -> fallback timer).
    this.awaiting = new Map();
  }
  async init() {
    const [session, local] = await Promise.all([this.chrome.storage.session.get('jobs'), this.chrome.storage.local.get('history')]);
    for (const job of session.jobs || []) if (!TERMINAL_STATES.has(job.state)) this.jobs.set(job.jobId, { ...job, submitted: true });
    this.history = (local.history || []).slice(0, 500);
    this.count();
  }
  list() { return [...this.jobs.values()]; }
  count() { this.onCount(this.jobs.size, this.awaiting.size); }
  awaitAnalysis(jobId) {
    clearTimeout(this.awaiting.get(jobId));
    this.awaiting.set(jobId, timer(() => this.settle(jobId), this.analysisTimeoutMs));
    this.count();
  }
  settle(jobId) {
    if (!this.awaiting.has(jobId)) return;
    clearTimeout(this.awaiting.get(jobId));
    this.awaiting.delete(jobId);
    this.count();
  }
  persist() {
    const jobs = structuredClone(this.list());
    const history = structuredClone(this.history);
    this.writes = this.writes.catch(() => {}).then(() => Promise.all([
      this.chrome.storage.session.set({ jobs }), this.chrome.storage.local.set({ history }),
    ]));
    return this.writes;
  }
  publish(terminal) {
    if (terminal) this.terminalUpdates.set(terminal.jobId, terminal);
    if (this.broadcastTimer) return;
    const send = () => {
      this.broadcastTimer = null;
      this.lastBroadcast = Date.now();
      const jobs = [...this.list(), ...this.terminalUpdates.values()];
      this.terminalUpdates.clear();
      void broadcast(this.chrome, { evt: EVT.JOBS, jobs });
    };
    const delay = Math.max(0, 250 - (Date.now() - this.lastBroadcast));
    if (!delay) send(); else this.broadcastTimer = timer(send, delay);
  }
  async download({ url: value, title, source, options: overrides = {} }) {
    const settings = await this.getSettings();
    const options = toDownloadOptions(settings, overrides);
    const url = normalizeMediaUrl(value, options.playlist);
    if (!url) throw failure('UNSUPPORTED_URL', 'Choose a valid HTTP or HTTPS media URL.');
    const jobId = crypto.randomUUID();
    const now = Date.now();
    const job = { jobId, url, title: title || url, source: source || siteForUrl(value), options, state: 'queued', progress: null, createdAt: now, updatedAt: now, submitted: false };
    this.jobs.set(jobId, job);
    this.count();
    this.publish();
    await this.persist();
    try {
      const cookies = settings.sendSiteCookies ? await siteCookies(this.chrome, url) : undefined;
      await this.native.request(HOST.DOWNLOAD, { jobId, url, options, source: job.source, ...(cookies ? { cookies } : {}) });
      if (this.jobs.has(jobId)) this.jobs.get(jobId).submitted = true;
      await this.persist();
      return { jobId };
    } catch (error) {
      await this.event({ jobId, state: 'error', error: errorData(error) });
      if (error.code === 'HOST_NOT_FOUND') await this.chrome.tabs.create({ url: this.chrome.runtime.getURL('src/options/options.html#setup') });
      throw error;
    }
  }
  async event(event) {
    if (this.history.some(item => item.id === event.jobId)) return;
    const previous = this.jobs.get(event.jobId) || { jobId: event.jobId, createdAt: Date.now() };
    const { type, id, ...fields } = event;
    if (type === 'job') fields.submitted = true;
    // The host starts with an empty title until extraction discovers one.
    if (!fields.title && previous.title) delete fields.title;
    const job = { ...previous, ...fields, updatedAt: Date.now() };
    if (TERMINAL_STATES.has(job.state)) {
      this.jobs.delete(job.jobId);
      const history = {
        ...job, id: job.jobId, format: job.options?.format || 'best', finishedAt: Date.now(),
      };
      // Cookie payloads never enter a persisted job or history record.
      delete history.cookies;
      this.history = [history, ...this.history.filter(item => item.id !== job.jobId)].slice(0, 500);
      if (job.state === 'done' && job.analysisPending > 0) this.awaitAnalysis(job.jobId);
      this.count();
      this.publish(job);
      if (job.state === 'done' && job.analysis) await this.analysisCache?.set(job.url, {analysis:job.analysis,title:job.title});
      await this.persist();
      await broadcast(this.chrome, { evt: EVT.HISTORY, history: this.history });
      await this.onTerminal(history);
    } else {
      this.jobs.set(job.jobId, job);
      this.count();
      this.publish();
      await this.persist();
    }
  }
  async resync(hostJobs) {
    // A fresh host process has dropped any analysis the previous one owed us.
    for (const jobId of [...this.awaiting.keys()]) this.settle(jobId);
    // Restore deferred analysis after worker reload; a new host may have lost it.
    let historyChanged = false;
    this.history = this.history.map(item => {
      if (!(item.analysisPending > 0)) return item;
      const snapshot = hostJobs.find(job => job.jobId === item.id);
      const pending = snapshot?.analysisPending || 0;
      if (pending) this.awaitAnalysis(item.id);
      historyChanged = true;
      return { ...item, analysisPending: pending, file: snapshot?.file || item.file, files: snapshot?.files || item.files };
    });
    if (historyChanged) {
      await this.persist();
      await broadcast(this.chrome, { evt: EVT.HISTORY, history: this.history });
    }
    const known = new Set(hostJobs.map(job => job.jobId));
    for (const job of hostJobs) await this.event({ ...job, submitted: true });
    for (const job of this.list()) if (job.submitted && !known.has(job.jobId)) {
      await this.event({ jobId: job.jobId, state: 'error', error: { code: 'INTERNAL', message: 'The native host stopped before this download finished. Retry the download.' } });
    }
  }
  // Late BPM/key for one file of a job, which may already be in history.
  // The host may have renamed `source` to `file` to add the BPM/key suffix.
  async analysis({ jobId, source, file, files, analysis, analysisError, pending }) {
    if (!pending) this.settle(jobId);
    const entry = { file, analysis, ...(analysisError ? { analysisError } : {}) };
    const rename = path => path === source ? file : path;
    const merge = item => ({
      ...item, analysisPending: pending ?? 0, file: rename(item.file), files: files?.length ? files : item.files?.map(rename),
      analysis: analysis || item.analysis || null, analyses: [...(item.analyses || []), entry],
      analysisError: analysisError || item.analysisError || null,
    });
    if (this.jobs.has(jobId)) {
      this.jobs.set(jobId, merge(this.jobs.get(jobId)));
      this.publish();
      await this.persist();
      return;
    }
    const index = this.history.findIndex(item => item.id === jobId);
    if (index < 0) return;
    const previous = this.history[index];
    const item = merge(previous);
    this.history = this.history.map((value, i) => i === index ? item : value);
    // In-page controls read BPM/key from the jobs broadcast, not history.
    this.publish(item);
    if (analysis) await this.analysisCache?.set(item.url, { analysis, title: item.title });
    await this.persist();
    await broadcast(this.chrome, { evt: EVT.HISTORY, history: this.history });
    await this.onAnalysis(item, previous);
  }
  async retry(jobId) {
    const job = this.jobs.get(jobId) || this.history.find(item => item.id === jobId);
    if (!job) throw failure('BAD_REQUEST', 'Download not found.');
    if (this.jobs.has(jobId)) throw failure('BAD_REQUEST', 'Cancel the active download before retrying.');
    return this.download({ url: job.url, title: job.title, source: job.source, options: job.options || { format: job.format } });
  }
  async cancel(jobId) {
    if (!this.jobs.has(jobId)) throw failure('BAD_REQUEST', 'Active download not found.');
    const result = await this.native.request(HOST.CANCEL, { jobId });
    // cancelled=true acknowledges the request. FFmpeg may still be running;
    // the host's terminal event is authoritative for queue/history/liveness.
    return result;
  }
  async clearHistory(ids) {
    if (ids !== undefined && !Array.isArray(ids)) throw failure('BAD_REQUEST', 'History ids must be an array.');
    this.history = ids ? this.history.filter(item => !ids.includes(item.id)) : [];
    await this.persist();
    await broadcast(this.chrome, { evt: EVT.HISTORY, history: this.history });
    return { history: this.history };
  }
}
