(() => {
  'use strict';
  if (!['soundcloud.com', 'www.soundcloud.com'].includes(location.hostname)) return;
  const { start, text, href } = SiphonContent;
  function itemMedia(item) {
    const link = item.querySelector('a.soundTitle__title, a.trackItem__trackTitle, a.playableTile__mainHeading, a[itemprop="url"]');
    const url = SiphonMedia.soundcloud(link?.getAttribute('href'), location.href);
    return url ? { url, title: link.textContent.trim() || document.title } : null;
  }
  function current() {
    const badge = document.querySelector('.playbackSoundBadge__titleLink');
    const playingURL = SiphonMedia.soundcloud(badge?.getAttribute('href'), location.href);
    if (playingURL) return { url: playingURL, title: badge.getAttribute('title') || badge.textContent.trim() || document.title };
    return pageMedia();
  }
  function pageMedia() {
    const url = SiphonMedia.soundcloud(location.href);
    return url ? { url, title: text(document, '.soundTitle__title, .soundTitle__titleHero, h1') || document.title.replace(/ \|.*$/, '') } : null;
  }
  start({ site: 'soundcloud', current,
    collect() {
      const result = [];
      if (pageMedia()) {
        // The main action row is outside list items; do not attach a page URL to a feed item.
        const mount = [...document.querySelectorAll('.listenEngagement .soundActions, .listenEngagement__actions, .fullHero .soundActions, .soundActions')].find(e => !e.closest('.soundList__item, .sound, .trackList__item'));
        const isSet = /\/sets\//.test(location.pathname);
        if (mount) result.push({ mount, key: 'page', resolve: pageMedia, ...(isSet ? { label: 'Download set', playlist: true } : {}) });
      }
      for (const item of document.querySelectorAll('.soundList__item, .trackList__item')) {
        const mount = item.querySelector('.sound__soundActions .soundActions, .sound__soundActions, .soundActions, .trackItem__additional');
        if (mount && itemMedia(item)) result.push({ mount, key: 'track', resolve: () => itemMedia(item) });
      }
      // Some feeds mount .sound directly without .soundList__item wrappers.
      for (const mount of document.querySelectorAll('.sound__soundActions')) {
        if (mount.closest('.soundList__item, .trackList__item')) continue;
        const item = mount.closest('.sound');
        if (item && itemMedia(item)) result.push({ mount, key: 'track', resolve: () => itemMedia(item) });
      }
      const playbar = document.querySelector('.playControls .playbackSoundBadge, .playControls .playbackSoundBadge__actions');
      if (playbar && href(document, '.playbackSoundBadge__titleLink')) result.push({ mount: playbar.querySelector('.playbackSoundBadge__actions') || playbar, key: 'player', resolve: current });
      return result;
    },
  });
})();
