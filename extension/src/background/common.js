export function failure(code, message) { return Object.assign(new Error(message), { code }); }
export function errorData(error) { return { code: error?.code || 'INTERNAL', message: error?.message || String(error) }; }
export function timer(callback, delay) {
  const id = setTimeout(callback, delay);
  id?.unref?.();
  return id;
}
export async function broadcast(chrome, message) {
  const sends = [Promise.resolve().then(() => chrome.runtime.sendMessage(message))];
  // runtime.sendMessage reaches extension pages, but not content scripts.
  if (message.evt === 'jobs') sends.push((async () => {
    const matches = chrome.runtime.getManifest().content_scripts?.flatMap(script => script.matches);
    const tabs = await chrome.tabs.query(matches?.length ? { url: [...new Set(matches)] } : {});
    await Promise.allSettled(tabs.filter(tab => tab.id !== undefined).map(tab => chrome.tabs.sendMessage(tab.id, message)));
  })());
  await Promise.allSettled(sends);
}
