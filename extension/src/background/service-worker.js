import { CMD, EVT } from '../shared/protocol.js';
import { getSettings, setSettings } from '../shared/settings.js';
import { NativeManager } from './native.js';
import { Jobs } from './jobs.js';
import { Badge } from './badge.js';
import { Notifications, friendlyError } from './notify.js';
import { createRouter } from './router.js';
import { registerMenus } from './menus.js';
import { broadcast } from './common.js';
import { AnalysisCache } from './analysis.js';

export function startBackground(api = chrome) {
  // Shared settings helpers refer to the extension's global chrome API.
  const badge = new Badge(api);
  let jobs;
  let notifications;
  const analysisCache = new AnalysisCache(api);
  const native = new NativeManager(api, {
    getSettings,
    onJob: event => jobs.event(event),
    onResync: snapshot => jobs.resync(snapshot),
  });
  jobs = new Jobs(api, {
    native, getSettings, analysisCache,
    onCount: count => { native.setActiveJobs(count); void badge.setCount(count).catch(console.error); },
    onTerminal: job => notifications.terminal(job).catch(console.error),
  });
  notifications = new Notifications(api, { native, getSettings, badge, findJob: id => jobs.history.find(job => job.id === id) });
  const ready = Promise.all([jobs.init(), analysisCache.init()]);
  const { route } = createRouter({ chrome: api, ready, native, jobs, getSettings, setSettings, analysisCache });
  const entryError = async error => {
    await badge.flash(false);
    // Download failures already have job notifications; this also covers URL resolution.
    console.warn('Siphon:', friendlyError(error));
  };
  const ensureMenus = registerMenus(api, route, entryError);
  void ensureMenus().catch(console.error);
  api.runtime.onInstalled.addListener(details => {
    void ensureMenus().catch(console.error);
    if (details.reason === 'install') void api.tabs.create({ url: api.runtime.getURL('src/options/options.html#setup') });
  });
  api.runtime.onStartup.addListener(() => {
    void ensureMenus().catch(console.error);
    void ready.then(() => { if (jobs.list().length) return native.connect(); }).catch(console.error);
  });
  api.runtime.onConnect.addListener(port => {
    if (!['siphon:ui', 'siphon-ui', 'popup', 'options'].includes(port.name)) return;
    native.uiPorts++;
    native.touchUI();
    void native.connect().catch(() => {});
    port.onDisconnect.addListener(() => { native.uiPorts = Math.max(0, native.uiPorts - 1); native.armIdle(); });
  });
  api.commands.onCommand.addListener(command => {
    if (command === 'download-current') void route({ cmd: CMD.DOWNLOAD_TAB }).then(() => badge.flash(true, '↓')).catch(entryError);
  });
  api.storage.onChanged.addListener((changes, area) => {
    if (area === 'sync' && changes.settings) void getSettings().then(async settings => {
      await broadcast(api, { evt: EVT.SETTINGS, settings });
      if (changes.settings.oldValue?.maxConcurrent !== settings.maxConcurrent) await native.configure(settings);
    }).catch(console.error);
  });
  void ready.then(() => { if (jobs.list().length) return native.connect(); }).catch(console.error);
  return { native, jobs, badge, notifications, route, ready };
}

export const background = startBackground();
