import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const context = vm.createContext({ URL });
vm.runInContext(readFileSync(new URL('../../src/content/media.js', import.meta.url), 'utf8'), context);
const { youtube, youtubePlaylist, soundcloud, bandcamp } = context.SiphonMedia;

test('YouTube watch, shorts, live, embed and short links resolve to single videos', () => {
  for (const url of ['/watch?v=dQw4w9WgXcQ&list=PL123&t=10', '/shorts/dQw4w9WgXcQ', '/live/dQw4w9WgXcQ', '/embed/dQw4w9WgXcQ', 'https://youtu.be/dQw4w9WgXcQ?t=42', 'https://music.youtube.com/watch?v=dQw4w9WgXcQ']) {
    assert.equal(youtube(url), 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');
  }
});
test('YouTube rejects foreign origins, malformed ids, and collection pages', () => {
  for (const url of ['https://youtube.com.evil.test/watch?v=dQw4w9WgXcQ', '/watch?v=short', '/playlist?list=PL123', 'javascript:alert(1)', undefined, null, '']) assert.equal(youtube(url), null);
});
test('YouTube collection URLs keep list ids and discard the current video', () => {
  assert.equal(youtubePlaylist('/watch?v=dQw4w9WgXcQ&list=OLAK5uy_123'), 'https://music.youtube.com/playlist?list=OLAK5uy_123');
  assert.equal(youtubePlaylist('/playlist?list=PL123'), 'https://music.youtube.com/playlist?list=PL123');
  for (const url of ['/watch?v=dQw4w9WgXcQ', '/playlist?list=bad%20id', 'https://evil.test/playlist?list=PL123']) assert.equal(youtubePlaylist(url), null);
});
test('SoundCloud track permalinks drop stream and collection tracking', () => {
  assert.equal(soundcloud('/artist/song?in=artist/sets/album&utm_source=clipboard'), 'https://soundcloud.com/artist/song');
  assert.equal(soundcloud('https://www.soundcloud.com/artist/sets/album/'), 'https://soundcloud.com/artist/sets/album');
});
test('SoundCloud private track and set URLs preserve access tokens', () => {
  assert.equal(soundcloud('/artist/song/s-private'), 'https://soundcloud.com/artist/song/s-private');
  assert.equal(soundcloud('/artist/sets/album/s-private'), 'https://soundcloud.com/artist/sets/album/s-private');
  assert.equal(soundcloud('/artist/song?secret_token=s-private&utm_source=test'), 'https://soundcloud.com/artist/song?secret_token=s-private');
});
test('SoundCloud rejects navigation routes, profiles and missing player links', () => {
  for (const url of ['/feed', '/stream', '/discover/sets', '/artist', '/artist/likes', '/artist/tracks', '/artist/sets', '/search/sounds', 'https://evil.test/artist/song', undefined, null, '']) assert.equal(soundcloud(url, 'https://soundcloud.com/artist/sets/album'), null);
});
test('Bandcamp supports artist subdomains, track and album URLs', () => {
  assert.equal(bandcamp('https://artist.bandcamp.com/album/release/?from=discover'), 'https://artist.bandcamp.com/album/release');
  assert.equal(bandcamp('/track/song', 'https://artist.bandcamp.com'), 'https://artist.bandcamp.com/track/song');
});
test('Bandcamp excludes custom domains, storefronts and invalid media paths', () => {
  for (const url of ['https://artist.example/album/release', 'https://bandcamp.com/album/release', 'https://artist.bandcamp.com/music', 'https://artist.bandcamp.com/album/', 'https://artist.bandcamp.com.evil.test/track/song', undefined, null, '']) assert.equal(bandcamp(url), null);
});
