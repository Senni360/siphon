import { CMD } from '../shared/protocol.js';

export const MENU_FORMATS = { mp3: 'MP3', m4a: 'M4A', flac: 'FLAC', opus: 'Opus', wav: 'WAV', best: 'Original (no re-encode)' };
export function registerMenus(chrome, route, onError) {
  const contexts = ['page', 'link', 'video', 'audio'];
  let creating;
  function ensure() {
    if (creating) return creating;
    creating = (async () => {
      // Menus persist across worker restarts. Recreate by stable IDs, serialized.
      await chrome.contextMenus.removeAll();
      await new Promise((resolve, reject) => {
        chrome.contextMenus.create({ id: 'siphon:download', title: 'Download audio', contexts }, () => {
          const error = chrome.runtime.lastError;
          if (error) reject(new Error(error.message)); else resolve();
        });
      });
      await new Promise(resolve => chrome.contextMenus.create({ id: 'siphon:as', title: 'Download as', contexts }, resolve));
      for (const [format, label] of Object.entries(MENU_FORMATS)) {
        await new Promise(resolve => chrome.contextMenus.create({ id: `siphon:format:${format}`, parentId: 'siphon:as', title: label, contexts }, resolve));
      }
    })().finally(() => { creating = null; });
    return creating;
  }
  chrome.contextMenus.onClicked.addListener((info, tab) => {
    const id = String(info.menuItemId);
    if (id !== 'siphon:download' && !id.startsWith('siphon:format:')) return;
    const format = id.startsWith('siphon:format:') ? id.slice(14) : undefined;
    void route({ cmd: CMD.DOWNLOAD, url: info.linkUrl || info.srcUrl || info.pageUrl || tab?.url, title: info.linkText || tab?.title, ...(format ? { options: { format } } : {}) }).catch(onError);
  });
  return ensure;
}
