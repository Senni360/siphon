/** Dependency-free Chrome mock. Events use addListener/removeListener/emit;
 * nativeHandler(request, port) can reply via port.reply(data) or port.emit(message).
 * Set nativeHandler to null for manual correlation/timeouts, or nativeError to fail.
 */
export function createEvent() {
  const listeners = new Set();
  return {
    addListener: listener => listeners.add(listener),
    removeListener: listener => listeners.delete(listener),
    hasListener: listener => listeners.has(listener),
    emit: (...args) => [...listeners].map(listener => listener(...args)),
    listeners,
  };
}

export function createChromeMock({ settings, jobs = [], history = [], tabs = [{ id: 1, active: true, url: 'https://www.youtube.com/watch?v=abc', title: 'Track' }], cookies = [], nativeHandler } = {}) {
  const clone = value => structuredClone(value);
  const calls = { broadcasts: [], native: [], ports: [], tabs: [], cookies: [], menus: [], notifications: [], badge: [], alarms: [] };
  const chrome = { calls };
  const changed = createEvent();
  function storage(name, initial) {
    const data = clone(initial);
    return {
      data,
      async get(keys = null) {
        if (keys === null) return clone(data);
        const names = typeof keys === 'string' ? [keys] : Array.isArray(keys) ? keys : Object.keys(keys);
        return Object.fromEntries(names.filter(key => key in data || (keys && typeof keys === 'object' && !Array.isArray(keys))).map(key => [key, clone(data[key] ?? keys[key])]));
      },
      async set(values) {
        const changes = {};
        for (const [key, value] of Object.entries(values)) { changes[key] = { oldValue: clone(data[key]), newValue: clone(value) }; data[key] = clone(value); }
        changed.emit(changes, name);
      },
      async remove(keys) {
        const changes = {};
        for (const key of Array.isArray(keys) ? keys : [keys]) { changes[key] = { oldValue: clone(data[key]) }; delete data[key]; }
        changed.emit(changes, name);
      },
      async clear() { return this.remove(Object.keys(data)); },
    };
  }
  chrome.storage = { onChanged: changed, sync: storage('sync', settings ? { settings } : {}), session: storage('session', { jobs }), local: storage('local', { history }) };
  chrome.runtime = {
    id: 'test-extension', lastError: undefined, onMessage: createEvent(), onInstalled: createEvent(), onStartup: createEvent(), onConnect: createEvent(),
    getManifest: () => ({ version: '1.0.0' }), getURL: path => `chrome-extension://test-extension/${path}`,
    async sendMessage(message) {
      calls.broadcasts.push(clone(message));
      if (!message.cmd) { chrome.runtime.onMessage.emit(message, {}, () => {}); return undefined; }
      return new Promise((resolve, reject) => {
        let asynchronous = false;
        for (const listener of chrome.runtime.onMessage.listeners) if (listener(message, {}, resolve) === true) asynchronous = true;
        if (!asynchronous) reject(new Error('Could not establish connection. Receiving end does not exist.'));
      });
    },
    connectNative(name) {
      const port = {
        name, sent: [], onMessage: createEvent(), onDisconnect: createEvent(), disconnected: false,
        postMessage(message) {
          if (port.disconnected) throw new Error('Disconnected port');
          port.sent.push(clone(message)); calls.native.push(clone(message));
          queueMicrotask(() => {
            if (chrome.nativeHandler) chrome.nativeHandler(message, port);
          });
        },
        emit: message => port.onMessage.emit(clone(message)),
        reply(request, data = {}, error) { port.emit({ id: request.id, type: 'result', ok: !error, ...(error ? { error } : { data }) }); },
        disconnect() { if (port.disconnected) return; port.disconnected = true; port.onDisconnect.emit(); },
        fail(message) { chrome.runtime.lastError = { message }; port.disconnect(); chrome.runtime.lastError = undefined; },
      };
      calls.ports.push(port);
      if (chrome.nativeError) queueMicrotask(() => port.fail(chrome.nativeError));
      return port;
    },
    connect({ name = 'siphon:ui' } = {}) {
      const port = { name, onDisconnect: createEvent(), disconnect() { this.onDisconnect.emit(); } };
      chrome.runtime.onConnect.emit(port);
      return port;
    },
  };
  chrome.nativeHandler = nativeHandler === undefined ? (request, port) => {
    port.reply(request, request.type === 'hello' ? { hostVersion: 'test', ffmpeg: { found: true } } : request.type === 'listJobs' ? { jobs: [] } : request.type === 'download' ? { jobId: request.jobId, accepted: true } : request.type === 'cancel' ? { jobId: request.jobId, cancelled: true } : {});
    if (request.type === 'cancel') port.emit({ type: 'job', jobId: request.jobId, state: 'cancelled', error: null });
  } : nativeHandler;
  chrome.contextMenus = { onClicked: createEvent(), async removeAll() { calls.menus.length = 0; }, create(menu, callback) { calls.menus.push(clone(menu)); queueMicrotask(() => callback?.()); return menu.id; } };
  chrome.notifications = { onClicked: createEvent(), onButtonClicked: createEvent(), async create(id, options) { calls.notifications.push({ id, ...clone(options) }); return id; }, async clear() { return true; } };
  chrome.action = { async setBadgeText(value) { calls.badge.push(clone(value)); }, async setBadgeBackgroundColor(value) { calls.badge.push(clone(value)); } };
  chrome.alarms = { onAlarm: createEvent(), async create(name, options) { calls.alarms.push({ name, ...clone(options) }); }, async clear() { return true; } };
  chrome.commands = { onCommand: createEvent() };
  chrome.cookies = { async getAll(filter) {
    calls.cookies.push(clone(filter));
    return clone(cookies.filter(cookie => {
      const domain = cookie.domain.replace(/^\./, '');
      const host = filter.url ? new URL(filter.url).hostname : null;
      return filter.domain ? domain === filter.domain || domain.endsWith(`.${filter.domain}`) : host === domain || (!cookie.hostOnly && host.endsWith(`.${domain}`));
    }));
  } };
  chrome.tabs = {
    items: tabs,
    async get(id) { const tab = tabs.find(tab => tab.id === id); if (!tab) throw new Error('No tab'); return clone(tab); },
    async query() { return clone(tabs.filter(tab => tab.active)); },
    async create(value) { calls.tabs.push(clone(value)); return { id: tabs.length + 1, ...value }; },
    async sendMessage(tabId, message) { if (chrome.tabMessageHandler) return chrome.tabMessageHandler(tabId, message); throw new Error('Receiving end does not exist.'); },
  };
  return chrome;
}
