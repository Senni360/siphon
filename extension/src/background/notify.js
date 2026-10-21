import { HOST } from '../shared/protocol.js';
import { friendlyError } from '../shared/errors.js';
import { dropItem, formatBpm } from '../shared/music.js';
export { friendlyError } from '../shared/errors.js';

export class Notifications {
  constructor(chrome, { native, getSettings, findJob, badge }) {
    Object.assign(this, { chrome, native, getSettings, findJob, badge });
    chrome.notifications.onClicked.addListener(id => { void this.activate(id).catch(() => {}); });
    chrome.notifications.onButtonClicked.addListener((id, button) => { void this.activate(id, button).catch(() => {}); });
  }
  async activate(id, button = 0) {
    if (!id.startsWith('siphon:')) return;
    const job = this.findJob(id.slice(7));
    if (job?.file) await this.native.request(button === 1 ? HOST.OPEN : HOST.REVEAL, { path: job.file });
  }
  async terminal(job) {
    if (job.state === 'cancelled') return;
    const settings = await this.getSettings();
    const success = job.state === 'done';
    await this.badge.flash(success);
    if (success && settings.autoOpenFolder && job.file) await this.native.request(HOST.REVEAL, { path: job.file }).catch(() => {});
    // The drag tray already says "done", so it replaces the completion toast.
    if (success && settings.dropOverlay && this.native.status?.dropzone && job.file) {
      try { await this.native.request(HOST.SHOW_DROP, { items: [dropItem(job, settings.keyNotation)] }); return; } catch { /* fall back to a notification */ }
    }
    if (success ? !settings.notifyOnComplete : !settings.notifyOnError) return;
    await this.chrome.notifications.create(`siphon:${job.id}`, {
      type: 'basic', iconUrl: this.chrome.runtime.getURL('icons/icon-128.png'),
      title: job.title || (success ? 'Download complete' : 'Download failed'),
      message: success ? [job.file?.split(/[\\/]/).pop() || 'Audio downloaded.', job.analysis && `${formatBpm(job.analysis.bpm)} BPM · ${job.analysis.key}`].filter(Boolean).join('\n') : friendlyError(job.error),
      ...(success && job.file ? { buttons: [{ title: 'Show in folder' }, { title: 'Play' }] } : {}),
    });
  }
}
