(() => {
  'use strict';
  if (location.hostname !== 'www.youtube.com') return;
  const { start, text } = SiphonContent;
  function activeReel() {
    return document.querySelector('ytd-reel-video-renderer[is-active], yt-shorts-video-renderer[is-active]') || [...document.querySelectorAll('ytd-reel-video-renderer, yt-shorts-video-renderer')].find(e => e.getBoundingClientRect().top >= -100 && e.getBoundingClientRect().top < innerHeight);
  }
  function current() {
    const url = SiphonMedia.youtube(location.href);
    const title = location.pathname.startsWith('/shorts/') ? text(activeReel(), 'yt-shorts-video-title-view-model h1, h1, h2') : text(document, 'ytd-watch-metadata h1');
    return url ? { url, title: title || document.title.replace(/ - YouTube$/, '') } : null;
  }
  start({
    site: 'youtube', current, dark: () => document.documentElement.hasAttribute('dark'),
    collect() {
      if (location.pathname === '/watch') {
        // Beside Subscribe: the like/share row is a clipped scroller that hides appended buttons.
        const mount = document.querySelector('ytd-watch-metadata #owner') || document.querySelector('ytd-watch-metadata #top-level-buttons-computed') || document.querySelector('ytd-watch-metadata #actions-inner') || document.querySelector('ytd-watch-metadata #actions') || document.querySelector('ytd-watch-flexy #menu #top-level-buttons-computed');
        return mount ? [{ mount, key: 'watch', resolve: current, slot: 'yt' }] : [];
      }
      if (location.pathname.startsWith('/shorts/')) {
        const reel = activeReel();
        const mount = reel?.querySelector('reel-action-bar-view-model') || reel?.querySelector('#actions, .actions, .ytReelPlayerOverlayViewModelActionsContainer') || document.querySelector('ytd-shorts #actions');
        // Native order ends with the sound pivot; keep it last, as it is in every other Short.
        const before = mount?.querySelector(':scope > pivot-button-view-model');
        return mount ? [{ mount, key: 'short', resolve: current, compact: true, before }] : [];
      }
      return [];
    },
  });
})();
