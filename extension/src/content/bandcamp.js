(() => {
  'use strict';
  if (!location.hostname.endsWith('.bandcamp.com')) return;
  const { start, text } = SiphonContent;
  function current() {
    const url = SiphonMedia.bandcamp(location.href);
    return url ? { url, title: text(document, '#name-section .trackTitle, h2.trackTitle') || document.title } : null;
  }
  start({ site: 'bandcamp', current,
    collect() {
      if (!current()) return [];
      const mount = document.querySelector('#name-section') || document.querySelector('#trackInfoInner .inline_player') || document.querySelector('#trackInfoInner');
      const album = location.pathname.startsWith('/album/');
      return mount ? [{ mount, key: 'release', resolve: current, ...(album ? { label: 'Download album audio', playlist: true } : {}) }] : [];
    },
  });
})();
