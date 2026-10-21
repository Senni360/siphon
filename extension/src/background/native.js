import { HOST, HOST_NAME, EVT } from '../shared/protocol.js';
import { broadcast, errorData, failure, timer } from './common.js';

export class NativeManager {
  constructor(chrome, { getSettings, onJob = () => {}, onResync = () => {}, idleMs = 60_000, retryMs = 1000, timeoutMs = 15_000, probeTimeoutMs = 45_000, analyzeTimeoutMs = 150_000, updateTimeoutMs = 330_000, pickerTimeoutMs = 300_000 } = {}) {
    Object.assign(this, { chrome, getSettings, onJob, onResync, idleMs, retryMs, timeoutMs, probeTimeoutMs, analyzeTimeoutMs, updateTimeoutMs, pickerTimeoutMs });
    this.status = { state: 'connecting' };
    this.pending = new Map();
    this.activeJobs = 0;
    this.uiPorts = 0;
    this.uiUntil = 0;
    this.attempt = 0;
  }
  setStatus(status) { this.status = status; void broadcast(this.chrome, { evt: EVT.HOST, status }); }
  touchUI() { this.uiUntil = Date.now() + this.idleMs; }
  demand() { return this.activeJobs > 0 || this.uiPorts > 0 || Date.now() < this.uiUntil; }
  async hasDemand() {
    if (this.activeJobs > 0 || this.uiPorts > 0) return true;
    if (this.chrome.runtime.getContexts) {
      try {
        const contexts = await this.chrome.runtime.getContexts({ contextTypes: ['POPUP', 'TAB', 'SIDE_PANEL'] });
        return contexts.some(context => ['src/popup/', 'src/options/'].some(path => context.documentUrl?.startsWith(this.chrome.runtime.getURL(path))));
      } catch { /* A transient context lookup can use the recent UI lease. */ }
    }
    return this.demand();
  }
  setActiveJobs(count) {
    this.activeJobs = count;
    if (!count && this.restartRequired && !this.updating) {
      void this.reconnect().catch(() => {});
      return;
    }
    if (!count) this.armIdle();
    else if (!this.port && !this.connecting) this.scheduleReconnect();
  }
  armIdle() {
    clearTimeout(this.idleTimer);
    if (!this.port || this.activeJobs || this.uiPorts) return;
    this.idleTimer = timer(() => {
      if (this.pending.size) return this.armIdle();
      if (!this.activeJobs && !this.uiPorts) this.disconnect();
    }, this.idleMs);
  }
  async connect() {
    if (this.connecting) return this.connecting;
    if (this.port) return this.status;
    clearTimeout(this.reconnectTimer);
    this.setStatus({ state: 'connecting' });
    this.connecting = (async () => {
      try {
        const settings = await this.getSettings();
        const port = this.chrome.runtime.connectNative(HOST_NAME);
        this.port = port;
        port.onMessage.addListener(message => this.receive(message));
        port.onDisconnect.addListener(() => {
          const message = this.chrome.runtime.lastError?.message || 'Native host disconnected.';
          if (this.port !== port) return;
          this.lost(message);
        });
        const hello = await this.raw(HOST.HELLO, { extVersion: this.chrome.runtime.getManifest().version, maxConcurrent: settings.maxConcurrent });
        const snapshot = await this.raw(HOST.LIST_JOBS);
        this.attempt = 0;
        this.restartRequired = false;
        this.setStatus({ ...hello, state: 'ready' });
        await this.onResync(snapshot.jobs || []);
        this.armIdle();
        return this.status;
      } catch (error) {
        if (this.port) { const port = this.port; this.port = null; port.disconnect(); }
        if (this.status.state !== 'missing') this.setStatus({ state: error.code === 'HOST_NOT_FOUND' ? 'missing' : 'error', error: errorData(error) });
        this.scheduleReconnect();
        throw error;
      } finally { this.connecting = null; }
    })();
    return this.connecting;
  }
  lost(message) {
    this.port = null;
    clearTimeout(this.idleTimer);
    const missing = /specified native messaging host not found|native messaging host.*not found/i.test(message);
    const error = failure(missing ? 'HOST_NOT_FOUND' : 'INTERNAL', message);
    this.setStatus({ state: missing ? 'missing' : 'error', error: errorData(error) });
    this.rejectPending(error);
    this.scheduleReconnect();
  }
  scheduleReconnect() {
    if (this.reconnectTimer || this.checkingDemand) return;
    this.checkingDemand = true;
    void this.hasDemand().then(demand => {
      if (!demand || this.port || this.reconnectTimer) return;
      const delay = Math.min(this.retryMs * 2 ** this.attempt++, 30_000);
      this.reconnectTimer = timer(() => {
        this.reconnectTimer = null;
        void this.hasDemand().then(needed => { if (needed) return this.connect(); }).catch(() => {});
      }, delay);
    }).finally(() => { this.checkingDemand = false; });
  }
  rejectPending(error) {
    for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error); }
    this.pending.clear();
  }
  disconnect() {
    clearTimeout(this.idleTimer);
    clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const port = this.port;
    this.port = null;
    port?.disconnect();
    this.rejectPending(failure('INTERNAL', 'Native connection closed.'));
  }
  async reconnect() { this.touchUI(); this.disconnect(); return this.connect(); }
  async configure(settings) {
    // Updating a queue limit must not replace the process running the downloads.
    if (this.connecting) await this.connecting;
    if (!this.port) return;
    const hello = await this.raw(HOST.HELLO, { extVersion: this.chrome.runtime.getManifest().version, maxConcurrent: settings.maxConcurrent });
    this.setStatus({ ...this.status, ...hello });
  }
  async update() {
    if (this.updating) throw failure('BAD_REQUEST', 'An engine update is already running.');
    this.updating = true;
    try {
      const result = await this.request(HOST.UPDATE);
      if (result.restartRequired) {
        this.restartRequired = true;
        this.setStatus({ ...this.status, restartRequired: true });
        // Disconnecting a native port ends its host, including active FFmpeg jobs.
        if (!this.activeJobs) await this.reconnect();
      }
      return result;
    } finally { this.updating = false; }
  }
  async request(type, payload = {}) {
    // A resync callback may reveal a completed file. The handshake has already
    // completed in that case; waiting on connect again would await ourselves.
    if (!this.port || this.status.state !== 'ready') await this.connect();
    return this.raw(type, payload);
  }
  raw(type, payload = {}) {
    if (!this.port) return Promise.reject(failure('INTERNAL', 'Native host is disconnected.'));
    const id = crypto.randomUUID();
    this.armIdle();
    return new Promise((resolve, reject) => {
      const timeout = timer(() => {
        this.pending.delete(id);
        reject(failure('NETWORK', `Native host timed out during ${type}.`));
      }, type === HOST.ANALYZE ? this.analyzeTimeoutMs : type === HOST.PROBE ? this.probeTimeoutMs : type === HOST.UPDATE ? this.updateTimeoutMs : [HOST.PICK_FOLDER, HOST.PICK_FILE].includes(type) ? this.pickerTimeoutMs : this.timeoutMs);
      this.pending.set(id, { resolve, reject, timer: timeout });
      try { this.port.postMessage({ ...payload, id, type }); }
      catch (error) { clearTimeout(timeout); this.pending.delete(id); reject(error); }
    });
  }
  receive(message) {
    if (message.type === 'result' && this.pending.has(message.id)) {
      const request = this.pending.get(message.id);
      this.pending.delete(message.id);
      clearTimeout(request.timer);
      if (message.ok) request.resolve(message.data || {});
      else request.reject(failure(message.error?.code || 'INTERNAL', message.error?.message || 'Native request failed.'));
    } else if (message.type === 'job' && message.jobId) {
      void Promise.resolve(this.onJob(message)).catch(error => console.error('Siphon job event:', error));
    }
  }
}
