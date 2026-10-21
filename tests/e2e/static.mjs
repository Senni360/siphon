import { readFile, readdir, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { resolve, dirname, extname, relative } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import vm from 'node:vm';
import { ROOT, Report, check, command } from './lib.mjs';

const base = resolve(ROOT, 'extension'), report = new Report('static');
async function walk(dir) { const out = []; for (const d of await readdir(dir, { withFileTypes: true })) out.push(...(d.isDirectory() ? await walk(resolve(dir, d.name)) : [resolve(dir, d.name)])); return out; }
const all = await walk(base), manifest = JSON.parse(await readFile(resolve(base, 'manifest.json'), 'utf8'));
const sources = new Map(await Promise.all(all.filter(f => relative(base, f).replaceAll('\\', '/').startsWith('src/') && /\.(?:html|js|css)$/.test(f)).map(async f => [f, await readFile(f, 'utf8')])));
function localFile(source, target, baseHref) {
  if (/^(https?:|data:|#|chrome:|mailto:|javascript:)/i.test(target)) return null;
  const file = target.startsWith('/') ? resolve(base, target.slice(1).split(/[?#]/)[0]) : baseHref
    ? fileURLToPath(new URL(target.split(/[?#]/)[0], new URL(baseHref, pathToFileURL(source))))
    : resolve(dirname(source), target.split(/[?#]/)[0]);
  check(file.startsWith(base + '\\') || file.startsWith(base + '/'), `${relative(base, source)} escapes extension: ${target}`);
  return file;
}
await report.test('MV3 manifest and pinned extension ID', () => {
  check(manifest.manifest_version === 3, 'manifest_version must be 3');
  check(manifest.background?.type === 'module', 'Service worker must be ES module');
  const hash = createHash('sha256').update(Buffer.from(manifest.key, 'base64')).digest('hex').slice(0, 32);
  const id = hash.replace(/[0-9a-f]/g, x => String.fromCharCode(97 + parseInt(x, 16)));
  check(id === 'bieofegakabfdjplcnpiaegdifmempmf', `Pinned ID mismatch: ${id}`);
  check(/^\d+(?:\.\d+){0,3}$/.test(manifest.version), 'Invalid manifest version');
  return id;
});
await report.test('all manifest entry points/icons/content resources exist', async () => {
  const resources = [manifest.background.service_worker, manifest.action.default_popup, manifest.options_page, ...Object.values(manifest.icons || {}), ...Object.values(manifest.action.default_icon || {})];
  for (const c of manifest.content_scripts || []) resources.push(...(c.js || []), ...(c.css || []));
  for (const file of resources.filter(Boolean)) check(existsSync(resolve(base, file)), `manifest.json → missing ${file}`);
  for (const resource of manifest.web_accessible_resources || []) for (const file of resource.resources || []) if (!file.includes('*')) check(existsSync(resolve(base, file)), `Missing web resource ${file}`);
  for (const [size, file] of Object.entries(manifest.icons || {})) {
    const bytes = await readFile(resolve(base, file)); check(bytes.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10])), `${file} is not a PNG`);
    check(bytes.readUInt32BE(16) === Number(size) && bytes.readUInt32BE(20) === Number(size), `${file} size mismatch`);
  }
  return `${resources.length} resources`;
});
await report.test('HTML references exist; MV3 prohibits executable inline scripts/handlers', () => {
  for (const [file, text] of sources) if (extname(file) === '.html') {
    const label = relative(base, file);
    check(!/\son[a-z]+\s*=/i.test(text), `${label}: inline event handler`);
    check(!/(?:href|src)\s*=\s*['"]\s*javascript:/i.test(text), `${label}: javascript: URL`);
    for (const match of text.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)) {
      check(/\bsrc\s*=/.test(match[1]) || !match[2].trim() || /type\s*=\s*['"]application\/(?:ld\+)?json['"]/.test(match[1]), `${label}: executable inline script`);
      check(!/\bsrc\s*=\s*['"]https?:/i.test(match[1]), `${label}: remotely hosted script`);
    }
    const baseHref = text.match(/<base\b[^>]*\bhref\s*=\s*['"]([^'"]+)['"]/i)?.[1];
    for (const match of text.matchAll(/\b(?:src|href)\s*=\s*['"]([^'"]+)['"]/gi)) {
      if (baseHref && match[1] === baseHref) continue;
      const path = localFile(file, match[1], baseHref); if (path) check(existsSync(path), `${label} → missing ${match[1]}`);
    }
  }
});
await report.test('permissions cover native host, settings, menus, notifications and site injection', () => {
  for (const permission of ['nativeMessaging', 'storage', 'contextMenus', 'notifications', 'tabs', 'scripting', 'cookies']) check(manifest.permissions.includes(permission), `Missing ${permission}`);
  check(manifest.host_permissions?.includes('<all_urls>'), 'Generic site/cookie access lacks host permission');
  check(!/unsafe-inline|unsafe-eval|https?:/.test(manifest.content_security_policy?.extension_pages || ''), 'Unsafe extension CSP');
});
await report.test('JavaScript syntax and local imports resolve', async () => {
  const js = [...sources.keys()].filter(f => extname(f) === '.js');
  await Promise.all(js.map(file => command(process.execPath, ['--check', file])));
  for (const file of js) for (const match of sources.get(file).matchAll(/(?:\bfrom\s*|\bimport\s*\(\s*|\bimport\s*)['"]([^'"]+)['"]/g)) {
    check(match[1].startsWith('.'), `${relative(base, file)}: nonlocal module ${match[1]}`);
    check(existsSync(resolve(dirname(file), match[1])), `${relative(base, file)} → missing import ${match[1]}`);
  }
  return `${js.length} scripts`;
});
await report.test('ES module linking catches missing named exports (chrome mock)', async () => {
  check(typeof vm.SourceTextModule === 'function', 'Run with node --experimental-vm-modules');
  const event = { addListener() {}, removeListener() {}, hasListener() { return false; } };
  const storageArea = { async get() { return {}; }, async set() {}, async remove() {} };
  const chrome = {
    storage: { sync: storageArea, local: storageArea, session: storageArea, onChanged: event },
    runtime: { getManifest: () => manifest, getURL: p => `chrome-extension://bieofegakabfdjplcnpiaegdifmempmf/${p}`, onMessage: event, onConnect: event, onInstalled: event, onStartup: event, sendMessage: async () => ({}), connectNative() { throw new Error('Native host intentionally unavailable in static import check'); } },
    tabs: { query: async () => [], onUpdated: event, onRemoved: event, onActivated: event },
    action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {}, setTitle: async () => {} },
    contextMenus: { onClicked: event, create(options, callback) { callback?.(); return options.id; }, removeAll: async () => {} },
    commands: { onCommand: event }, notifications: { onClicked: event, onButtonClicked: event, onClosed: event, create: async () => {}, clear: async () => {} },
    alarms: { onAlarm: event, create: async () => {} }, scripting: { registerContentScripts: async () => {}, unregisterContentScripts: async () => {}, getRegisteredContentScripts: async () => [] },
  };
  const mockErrors = [];
  const mockConsole = { ...console, error: (...args) => mockErrors.push(args.map(String).join(' ')) };
  const context = vm.createContext({ chrome, console: mockConsole, URL, crypto: globalThis.crypto, setTimeout, clearTimeout, setInterval, clearInterval, structuredClone, navigator: { platform: 'Win32' } });
  // Link each entry in an independent graph to keep concurrent/cyclic linker state deterministic.
  for (const file of [...sources.keys()].filter(f => extname(f) === '.js' && /\b(?:import|export)\b/.test(sources.get(f)))) {
    const cache = new Map();
    const get = filename => {
      if (!cache.has(filename)) cache.set(filename, new vm.SourceTextModule(sources.get(filename), { context, identifier: filename }));
      return cache.get(filename);
    };
    const entry = get(file);
    await entry.link((specifier, ref) => get(resolve(dirname(ref.identifier), specifier)));
    if (relative(base, file).replaceAll('\\', '/').startsWith('src/shared/')) await entry.evaluate({ timeout: 3000 });
    if (file.endsWith('service-worker.js')) { await entry.evaluate({ timeout: 3000 }); await entry.namespace.background.ready; }
  }
  check(mockErrors.length === 0, mockErrors.join('\n'));
  return 'All module graphs linked; shared modules and service worker evaluated with chrome mock';
});
await report.finish();
