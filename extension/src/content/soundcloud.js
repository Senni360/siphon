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
  // querySelector('a, b') returns the first match in document order, not the first selector;
  // the outer .sound__soundActions wrapper would win and push the control under the toolbar.
  const first = (root, selectors) => selectors.map(s => root.querySelector(s)).find(Boolean) || null;
  // "More" is never toggled (Like turns orange), so it gives the neutral button fill.
  const sample = toolbar => toolbar.querySelector('.sc-button-more, .sc-button-share');
  // Joining the native button group as its last child lets SoundCloud's own spacing
  // rule give "More" its 16px gap, whatever padding or buy link the toolbar carries.
  const group = toolbar => toolbar.querySelector(':scope > .sc-button-group') || toolbar;
  // Feed rows pin the play and comment counts to the right edge. Below ~1240px SoundCloud's
  // own buttons nearly fill the row, so the control then takes its own line under the toolbar
  // instead of running into the counts. Nothing measured moves with the control, so the
  // choice cannot flip back and forth between scans.
  function row(toolbar, card) {
    const buttons = group(toolbar), more = buttons.querySelector(':scope > .sc-button-more');
    const wrapper = toolbar.parentElement?.matches?.('.sound__soundActions') ? toolbar.parentElement : null;
    const counts = card.querySelector('.soundStats .sc-ministats');
    if (!more || !wrapper || !counts?.getClientRects().length) return { mount: buttons, slot: 'sc-row' };
    const width = card.querySelector('[data-siphon="track"]')?.getBoundingClientRect().width || 104;
    const end = more.getBoundingClientRect().right + 16 + width + parseFloat(getComputedStyle(buttons).paddingRight || 0) + toolbar.getBoundingClientRect().right - buttons.getBoundingClientRect().right;
    return counts.getBoundingClientRect().left - end >= 8 ? { mount: buttons, slot: 'sc-row' } : { mount: wrapper, slot: 'sc-wrap' };
  }
  start({ site: 'soundcloud', current,
    collect() {
      const result = [];
      if (pageMedia()) {
        // The main action row is outside list items; do not attach a page URL to a feed item.
        const mount = [...document.querySelectorAll('.listenEngagement .soundActions, .listenEngagement__actions, .fullHero .soundActions, .soundActions')].find(e => !e.closest('.soundList__item, .sound, .trackList__item'));
        const isSet = /\/sets\//.test(location.pathname);
        if (mount) result.push({ mount: group(mount), key: 'page', resolve: pageMedia, slot: 'sc', sample: sample(mount), ...(isSet ? { label: 'Download set', playlist: true } : {}) });
      }
      // List rows have little room before the stats column: icon-only, like Share and Copy link.
      for (const item of document.querySelectorAll('.soundList__item, .trackList__item')) {
        const toolbar = first(item, ['.sound__soundActions .soundActions', '.soundActions', '.sound__soundActions', '.trackItem__additional']);
        if (toolbar && itemMedia(item)) result.push({ ...row(toolbar, item), key: 'track', resolve: () => itemMedia(item), icon: true, sample: sample(toolbar) });
      }
      // Some feeds mount .sound directly without .soundList__item wrappers.
      for (const wrapper of document.querySelectorAll('.sound__soundActions')) {
        if (wrapper.closest('.soundList__item, .trackList__item')) continue;
        const item = wrapper.closest('.sound'), toolbar = wrapper.querySelector('.soundActions') || wrapper;
        if (item && itemMedia(item)) result.push({ ...row(toolbar, item), key: 'track', resolve: () => itemMedia(item), icon: true, sample: sample(toolbar) });
      }
      // Beside the fixed-width now-playing badge rather than inside it: inside, the control
      // squeezed the track title to nothing. Here the flexible timeline gives up the width.
      const badge = document.querySelector('.playControls .playbackSoundBadge');
      if (badge && href(document, '.playbackSoundBadge__titleLink')) {
        const mount = badge.closest('.playControls__elements') || badge.querySelector('.playbackSoundBadge__actions') || badge;
        result.push({ mount, key: 'player', resolve: current, slot: 'sc-bar', icon: true, sample: badge.querySelector('.playbackSoundBadge__showQueue, .playbackSoundBadge__like') });
      }
      return result;
    },
  });
})();
