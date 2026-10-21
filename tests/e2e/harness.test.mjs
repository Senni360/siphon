import { test } from 'node:test';
import assert from 'node:assert/strict';
import { frame, FrameDecoder } from './lib.mjs';

test('Chrome framing handles every split, Unicode, and coalesced messages', () => {
  const values = [{ id: 'a', title: 'Siphon 🎵 日本語' }, { type: 'job', progress: 0.5 }];
  const bytes = Buffer.concat(values.map(frame));
  for (let split = 0; split <= bytes.length; split++) {
    const decoder = new FrameDecoder();
    assert.deepEqual([...decoder.push(bytes.subarray(0, split)), ...decoder.push(bytes.subarray(split))], values);
    assert.equal(decoder.buffer.length, 0);
  }
});
test('Chrome framing rejects unbounded lengths and polluted stdout', () => {
  const decoder = new FrameDecoder();
  assert.throws(() => decoder.push(Buffer.from([1, 0, 16, 0])), /Invalid response frame/);
  assert.throws(() => new FrameDecoder().push(Buffer.from('debug log\n')), /Invalid response frame/);
});
