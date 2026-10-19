import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setImmediate } from 'node:timers/promises';
import { readUsage, renderKey, startPlugin } from './com.yoophi.diskmeter.sdPlugin/usage.mjs';

const root = { path: '/', used_percent: 91.08, error: null };
const dashboard = { panes: [{ path: '/external', used_percent: 10 }, root] };

test('selects root, rejects missing or invalid data, and renders the same disk chart', () => {
  assert.equal(readUsage(dashboard), root);
  assert.throws(() => readUsage({ panes: [] }), /NO DATA/);
  assert.throws(() => readUsage({ panes: [{ path: '/', used_percent: null }] }), /BAD DATA/);
  assert.throws(() => readUsage({ panes: [{ path: '/', error: 'failed' }] }), /FETCH ERROR/);
  const svg = renderKey(readUsage(dashboard));
  assert.match(svg, />91%</);
  assert.match(svg, /#f47777/);
  const width = Number(svg.match(/data-kind="usage"[^>]*width="([^"]+)"/)[1]);
  assert.ok(Math.abs(width / 116 * 100 - 91.08) < 0.001);
  for (const match of svg.matchAll(/<text[^>]* y="(\d+)"/g)) assert.ok(Number(match[1]) <= 110);
  assert.match(renderKey({ ...root, used_percent: 75 }), /#edc56b/);
  assert.match(renderKey({ ...root, used_percent: 50 }), /#67deb3/);
});

test('registers, shares fetches between keys, recovers offline, opens DiskMeter and stops hidden fetches', async () => {
  class Socket extends EventTarget {
    readyState = 1;
    sent = [];
    send(data) { this.sent.push(JSON.parse(data)); }
    message(message) { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(message) })); }
  }
  const socket = new Socket();
  let offline = false;
  let calls = 0;
  const plugin = startPlugin(socket, { event: 'registerPlugin', uuid: 'test' }, {
    fetchImpl: async url => {
      assert.equal(url, 'http://127.0.0.1:9998/api/dashboard');
      calls++;
      if (offline) throw new Error('connection refused');
      return { ok: true, json: async () => dashboard };
    },
  });
  const lastSvg = () => Buffer.from(socket.sent.filter(m => m.event === 'setImage').at(-1).payload.image.split(',')[1], 'base64').toString();
  try {
    socket.dispatchEvent(new Event('open'));
    assert.equal(socket.sent[0].event, 'registerPlugin');
    for (const context of ['key1', 'key2']) socket.message({ event: 'willAppear', action: 'com.yoophi.diskmeter.usage', context });
    await setImmediate();
    assert.equal(calls, 1);
    assert.match(lastSvg(), />91%</);
    offline = true;
    await plugin.refresh();
    assert.match(lastSvg(), /OFFLINE/);
    assert.match(lastSvg(), /#a4aab5/);
    assert.match(lastSvg(), />91%</);
    offline = false;
    await plugin.refresh();
    assert.doesNotMatch(lastSvg(), /OFFLINE/);
    socket.message({ event: 'keyUp', context: 'key1' });
    assert.equal(socket.sent.find(m => m.event === 'openUrl').payload.url, 'http://127.0.0.1:9998');
    await setImmediate();
    for (const context of ['key1', 'key2']) socket.message({ event: 'willDisappear', context });
    const before = calls;
    await plugin.refresh();
    assert.equal(calls, before);
  } finally { plugin.stop(); }
});
