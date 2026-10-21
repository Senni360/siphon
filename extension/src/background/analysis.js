import { normalizeMediaUrl } from '../shared/urls.js';

/** Session-scoped URL results; no cookies, paths or audio are retained. */
export class AnalysisCache {
  constructor(chrome) { this.chrome = chrome; this.entries = {}; this.writes = Promise.resolve(); }
  init() {
    return this.loading ||= this.chrome.storage.session.get('analyses').then(({analyses}) => { this.entries = analyses || {}; });
  }
  get(url) { return this.entries[normalizeMediaUrl(url)] || null; }
  async set(url, result) {
    const key = normalizeMediaUrl(url);
    if (!key || !result?.analysis) return;
    delete this.entries[key];
    this.entries[key] = {analysis:result.analysis, ...(result.title ? {title:result.title} : {})};
    this.entries = Object.fromEntries(Object.entries(this.entries).slice(-200));
    const analyses = structuredClone(this.entries);
    this.writes = this.writes.catch(() => {}).then(() => this.chrome.storage.session.set({analyses}));
    await this.writes;
  }
}
