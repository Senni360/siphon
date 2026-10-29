import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { createChromeMock } from '../helpers/chrome-mock.js';

const source = name => readFileSync(new URL(`../../src/content/${name}.js`, import.meta.url), 'utf8');
function adapter(name, url, document) {
  const context = vm.createContext({ URL, location: new URL(url), document, innerHeight: 720, chrome: createChromeMock() });
  vm.runInContext(source('media'), context);
  vm.runInContext(source('core'), context);
  let result;
  context.SiphonContent = { ...context.SiphonContent, start: value => { result = value; } };
  vm.runInContext(source(name), context);
  return result;
}
const node = ({ text = '', attributes = {}, one = () => null, all = () => [] } = {}) => ({ textContent: text, getAttribute: key => attributes[key] ?? null, querySelector: one, querySelectorAll: all });
const doc = (one = () => null, all = () => []) => ({ title: 'Example title', querySelector: one, querySelectorAll: all });

test('Music resolves plain-text player title via current artwork on an unrelated browse page', () => {
  const image = node({ attributes: { src: 'https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?x=1' } });
  const title = node({ text: 'Playing track' });
  const player = node({ one: s => s === '.title' ? title : s.includes('img') ? image : null });
  const a = adapter('youtube-music', 'https://music.youtube.com/browse/ALBUM123', doc(s => s === 'ytmusic-player-bar' ? player : null));
  assert.equal(a.current().url, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');
  assert.equal(a.current().title, 'Playing track');
});
test('Music player title link takes precedence over the browsing URL', () => {
  const link = node({ text: 'Current track', attributes: { href: '/watch?v=dQw4w9WgXcQ&list=PL123' } });
  const player = node({ all: () => [link] });
  const a = adapter('youtube-music', 'https://music.youtube.com/watch?v=abcdefghijk', doc(s => s === 'ytmusic-player-bar' ? player : null));
  assert.equal(a.current().url, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');
});
test('Music collection descriptors force playlist downloads', () => {
  const mount = node();
  const a = adapter('youtube-music', 'https://music.youtube.com/playlist?list=PL123', doc(s => s.includes('header-renderer') ? mount : null));
  const descriptor = a.collect()[0];
  assert.equal(descriptor.playlist, true);
  assert.equal(descriptor.resolve().url, 'https://music.youtube.com/playlist?list=PL123');
});
test('SoundCloud media query returns the bottom playbar track while browsing a feed', () => {
  const badge = node({ text: 'Now playing', attributes: { href: '/artist/song?in=feed', title: 'Now playing' } });
  const a = adapter('soundcloud', 'https://soundcloud.com/feed', doc(s => s === '.playbackSoundBadge__titleLink' ? badge : null));
  assert.equal(a.current().url, 'https://soundcloud.com/artist/song');
  assert.equal(a.current().title, 'Now playing');
});
test('SoundCloud without a bottom player safely falls back to the track page', () => {
  const a = adapter('soundcloud', 'https://soundcloud.com/artist/song', doc());
  assert.equal(a.current().url, 'https://soundcloud.com/artist/song');
});
test('SoundCloud feed without a player has no media', () => {
  assert.equal(adapter('soundcloud', 'https://soundcloud.com/feed', doc()).current(), null);
});
test('SoundCloud list descriptors resolve a recycled item permalink at click time', () => {
  let permalink = '/artist/first';
  const link = node({ text: 'Track' }); link.getAttribute = () => permalink;
  const mount = node();
  const item = node({ one: s => s.includes('a.soundTitle') ? link : mount });
  const a = adapter('soundcloud', 'https://soundcloud.com/feed', doc(() => null, s => s === '.soundList__item, .trackList__item' ? [item] : []));
  const descriptor = a.collect()[0];
  assert.equal(descriptor.resolve().url, 'https://soundcloud.com/artist/first');
  permalink = '/artist/second';
  assert.equal(descriptor.resolve().url, 'https://soundcloud.com/artist/second');
});
test('SoundCloud rows join the inner toolbar button group, not the outer actions wrapper', () => {
  // querySelector with a selector list matches in document order, so the outer wrapper used to win.
  const link = node({ text: 'Track', attributes: { href: '/artist/song' } });
  const buttons = node(), toolbar = node({ one: s => s === ':scope > .sc-button-group' ? buttons : null }), wrapper = node();
  const item = node({ one: s => s.includes('a.soundTitle') ? link : s === '.sound__soundActions .soundActions' ? toolbar : s === '.sound__soundActions' ? wrapper : null });
  const a = adapter('soundcloud', 'https://soundcloud.com/feed', doc(() => null, s => s === '.soundList__item, .trackList__item' ? [item] : []));
  const descriptor = a.collect()[0];
  assert.equal(descriptor.mount, buttons);
  assert.equal(descriptor.slot, 'sc-row');
  assert.equal(descriptor.icon, true);
});
test('SoundCloud player control sits beside the now-playing badge, not inside it', () => {
  const elements = node(), title = node({ text: 'Now playing', attributes: { href: '/artist/song', title: 'Now playing' } });
  const badge = { ...node(), closest: s => s === '.playControls__elements' ? elements : null };
  const a = adapter('soundcloud', 'https://soundcloud.com/feed', doc(s => s === '.playControls .playbackSoundBadge' ? badge : s === '.playbackSoundBadge__titleLink' ? title : null));
  const descriptor = a.collect().find(d => d.key === 'player');
  assert.equal(descriptor.mount, elements);
  assert.equal(descriptor.slot, 'sc-bar');
});
test('Bandcamp album descriptors download all audio; track descriptors stay single', () => {
  for (const type of ['album', 'track']) {
    const a = adapter('bandcamp', `https://artist.bandcamp.com/${type}/release`, doc(s => s === '#name-section' ? node() : null));
    assert.equal(Boolean(a.collect()[0].playlist), type === 'album');
  }
});
test('YouTube watch action row uses a canonical single-video URL', () => {
  const mount = node();
  const a = adapter('youtube', 'https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123', doc(s => s === 'ytd-watch-metadata #top-level-buttons-computed' ? mount : null));
  assert.equal(a.collect()[0].mount, mount);
  assert.equal(a.current().url, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');
});

test('YouTube mounts beside Subscribe before the clipped like/share toolbar', () => {
  const owner = node(), toolbar = node();
  const a = adapter('youtube', 'https://www.youtube.com/watch?v=dQw4w9WgXcQ', doc(s =>
    s === 'ytd-watch-metadata #owner' ? owner : s === 'ytd-watch-metadata #top-level-buttons-computed' ? toolbar : null));
  assert.equal(a.collect()[0].mount, owner);
});
test('YouTube 2026 Shorts uses the visible reel action-bar view model without is-active', () => {
  const pivot = node(), mount = node({ one: s => s === ':scope > pivot-button-view-model' ? pivot : null }), title = node({ text: 'Current short' });
  const reel = node({ one: s => s.includes('reel-action-bar-view-model') ? mount : title });
  reel.getBoundingClientRect = () => ({ top: 64 });
  const a = adapter('youtube', 'https://www.youtube.com/shorts/DZpSvIkDH-c', doc(() => null, () => [reel]));
  assert.equal(a.collect()[0].mount, mount);
  assert.equal(a.collect()[0].compact, true);
  assert.equal(a.collect()[0].before, pivot, 'the sound pivot stays last in the column');
  assert.equal(a.current().title, 'Current short');
  assert.equal(a.current().url, 'https://www.youtube.com/watch?v=DZpSvIkDH-c');
});
