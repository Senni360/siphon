(() => {
  'use strict';
  if (location.hostname !== 'music.youtube.com') return;
  const { start, text } = SiphonContent;
  function current() {
    const player = document.querySelector('ytmusic-player-bar');
    const links = player?.querySelectorAll('a.yt-simple-endpoint[href], .title a[href], a[href*="watch?"]') || [];
    for (const link of links) {
      const url = SiphonMedia.youtube(link.getAttribute('href'), location.href);
      if (url) return { url, title: text(player, '.title') || link.textContent.trim() || document.title };
    }
    // Current Music builds render a plain-text title. Its player-bar artwork is
    // still tied to the playing video, even while an unrelated browse page is open.
    const image = player?.querySelector('.thumbnail-image-wrapper img, img.image');
    const imageURL = image?.getAttribute('src') || '';
    const imageID = /^https:\/\/(?:i\d?\.ytimg\.com|img\.youtube\.com)\/(?:vi|vi_webp)\/([\w-]{11})\//.exec(imageURL)?.[1];
    if (imageID) return { url: SiphonMedia.youtube(`/watch?v=${imageID}`), title: text(player, '.title') || document.title };
    // Video ids are public DOM attributes; no isolated-world access to the player API.
    const video = document.querySelector('ytmusic-player[video-id], ytmusic-player-page[video-id], ytmusic-player-bar[video-id]');
    const id = video?.getAttribute('video-id');
    const url = (id && SiphonMedia.youtube(`/watch?v=${id}`)) || SiphonMedia.youtube(location.href);
    return url ? { url, title: text(player, '.title') || document.title.replace(/ - YouTube Music$/, '') } : null;
  }
  function playlist() {
    const url = SiphonMedia.youtubePlaylist(location.href) || [...document.querySelectorAll('ytmusic-detail-header-renderer a[href*="list="], ytmusic-responsive-header-renderer a[href*="list="]')].map(a => SiphonMedia.youtubePlaylist(a.href)).find(Boolean);
    return url ? { url, title: text(document, 'ytmusic-detail-header-renderer .title, ytmusic-responsive-header-renderer .title') || document.title } : null;
  }
  start({ site: 'youtubeMusic', current, dark: () => true,
    collect() {
      const result = [], player = document.querySelector('ytmusic-player-bar');
      const mount = player?.querySelector('.middle-controls-buttons') || player?.querySelector('.middle-controls, .right-controls');
      if (mount) result.push({ mount, key: 'player', resolve: current, slot: 'ytm' });
      if (/^\/(?:playlist|browse)/.test(location.pathname) && playlist()) {
        const buttons = document.querySelector('ytmusic-responsive-header-renderer .buttons, ytmusic-detail-header-renderer #buttons, ytmusic-detail-header-renderer .buttons, ytmusic-responsive-header-renderer #buttons');
        // The current header centres a row too narrow to share; sit centred beneath it instead.
        const header = buttons || document.querySelector('ytmusic-detail-header-renderer, ytmusic-responsive-header-renderer');
        if (header) result.push({ mount: header, key: 'playlist', resolve: playlist, label: 'Download all', playlist: true, ...(buttons ? {} : { slot: 'ytm-header' }) });
      }
      return result;
    },
  });
})();
