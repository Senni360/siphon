import { stemPathKey, stemPaths } from '../shared/stems.js';
import { EVT, HOST } from '../shared/protocol.js';
import { broadcast, failure } from './common.js';

const active = job => ['queued', 'running'].includes(job.state);
const time = value => typeof value === 'number' ? value : Date.parse(value) || 0;
export class Stems {
  constructor(chrome, { native, jobs, onCount = () => {} }) {
    Object.assign(this, { chrome, native, jobs, onCount });
    this.jobsById = new Map();
    this.writes = Promise.resolve();
  }
  async init() {
    const { stems = [] } = await this.chrome.storage.local.get('stems');
    for (const job of stems) this.jobsById.set(job.jobId, job);
    this.count();
  }
  list() { return [...this.jobsById.values()]; }
  count() { this.onCount(this.list().filter(active).length); }
  async save() {
    this.count();
    const stems = structuredClone(this.list());
    this.writes = this.writes.catch(() => {}).then(() => this.chrome.storage.local.set({ stems }));
    await this.writes;
    await broadcast(this.chrome, { evt: EVT.STEMS, stems });
  }
  async event(event) {
    if (!event.jobId || !['queued', 'running', 'done', 'error', 'cancelled'].includes(event.state)) return;
    const previous = this.jobsById.get(event.jobId);
    // Late progress must not resurrect a terminal job or regress a newer snapshot.
    if (previous && (!active(previous) && active(event) || time(event.updatedAt) && time(event.updatedAt) < time(previous.updatedAt))) return;
    const source = this.jobs.history.find(item => event.sourceFile && stemPaths(item).some(path => stemPathKey(path) === stemPathKey(event.sourceFile)));
    const { type, id, ...fields } = event;
    if (fields.createdAt != null) fields.createdAt = time(fields.createdAt);
    if (fields.updatedAt != null) fields.updatedAt = time(fields.updatedAt);
    this.jobsById.set(event.jobId, { sourceId: source?.id, title: source?.title, ...previous, ...fields });
    await this.save();
  }
  async resync(snapshots) {
    const known = new Set(snapshots.map(job => job.jobId));
    for (const snapshot of snapshots) await this.event(snapshot);
    for (const job of this.list()) if (active(job) && !known.has(job.jobId)) {
      await this.event({ ...job, state: 'error', stage: 'interrupted', progress: null, updatedAt: Date.now(),
        error: { code: 'STEMS_INTERRUPTED', message: 'The engine stopped before splitting finished. Retry Split stems.' } });
    }
  }
  async split(id, selectedPath) {
    await this.native.connect();
    if (this.native.status.stems !== true) throw failure('BAD_REQUEST', 'Split stems needs a newer Siphon helper. Update the helper using the latest installer, then reconnect in Settings → Connection & setup.');
    // Resolve from current history after the handshake, never trust a UI-supplied path.
    const item = this.jobs.history.find(item => item.id === id);
    const paths = item ? stemPaths(item) : [];
    if (selectedPath == null && paths.length > 1) throw failure('BAD_REQUEST', 'Choose a track from this download before splitting stems.');
    const path = selectedPath == null ? paths[0] : paths.find(path => stemPathKey(path) === stemPathKey(selectedPath));
    if (selectedPath != null && !path) throw failure('BAD_REQUEST', 'That track is no longer in this download. Choose its latest filename and try again.');
    if (item?.state !== 'done' || !path || !/^(?:[a-z]:[\\/]|\\\\|\/)/i.test(path)) throw failure('BAD_REQUEST', 'Choose a completed downloaded audio file.');
    if (item.analysisPending > 0 || this.jobs.awaiting?.has(id)) throw failure('BAD_REQUEST', 'Wait for tempo and key analysis to finish before splitting stems.');
    // No await between this lookup and reservation: concurrent requests share one job.
    const existing = this.list().find(job => active(job) && stemPathKey(job.sourceFile) === stemPathKey(path));
    if (existing) return { jobId: existing.jobId, accepted: true };
    const jobId = crypto.randomUUID(), now = Date.now();
    this.jobsById.set(jobId, { jobId, sourceId: id, sourceFile: path, title: item.title, state: 'queued', stage: 'queued', progress: null, files: [], outputDir: null, error: null, createdAt: now, updatedAt: now });
    await this.save();
    try { return await this.native.request(HOST.SPLIT_STEMS, { jobId, path }); }
    catch (error) {
      // A lost reply may still have started work. A fresh list is authoritative.
      if (this.native.status.state !== 'ready') {
        this.native.scheduleReconnect();
      } else if (error.code === 'NETWORK') {
        // Do not replace a live host merely because its acknowledgement timed out:
        // that would also kill unrelated downloads. Query its authoritative state.
        const snapshot = await this.native.request(HOST.LIST_STEMS);
        await this.resync(snapshot.jobs || []);
      } else await this.event({ jobId, state: 'error', error: { code: error.code, message: error.message }, updatedAt: Date.now() });
      throw error;
    }
  }
  async cancel(jobId) {
    if (!active(this.jobsById.get(jobId) || {})) throw failure('BAD_REQUEST', 'No active stem separation found.');
    // Acknowledgement is not termination; keep the native process alive until its event.
    const result = await this.native.request(HOST.CANCEL_STEMS, { jobId });
    const job = this.jobsById.get(jobId);
    if (result.cancelled && active(job)) {
      job.cancelRequested = true;
      await this.save();
    }
    return result;
  }
  async show(jobId) {
    const job = this.jobsById.get(jobId);
    if (job?.state !== 'done' || job.files?.length !== 4) throw failure('BAD_REQUEST', 'All four stems must finish before opening the drag tray.');
    return this.native.request(HOST.SHOW_DROP, { pinned: true, items: [{ files: job.files, title: job.title || 'Stems', subtitle: 'Vocals · Drums · Bass · Other' }] });
  }
}
