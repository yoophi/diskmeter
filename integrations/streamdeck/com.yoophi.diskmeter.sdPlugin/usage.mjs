export const BASE_URL = 'http://127.0.0.1:9998';
export const POLL_MS = 30_000;

const escape = text => String(text).replace(/[&<>"']/g, c => ({
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;',
})[c]);

export function readUsage(dashboard) {
  const disk = dashboard.panes?.find(pane => pane.path === '/');
  if (!disk) throw new Error('NO DATA');
  if (!Number.isFinite(disk.used_percent)) throw new Error(disk.error ? 'FETCH ERROR' : 'BAD DATA');
  return disk;
}

export function renderKey(disk, { error = '' } = {}) {
  const status = error || (disk?.error ? 'FETCH ERROR' : '');
  const used = disk ? Math.max(0, Math.min(100, disk.used_percent)) : 0;
  const color = status ? '#a4aab5' : used >= 90 ? '#f47777' : used >= 70 ? '#edc56b' : '#67deb3';
  const text = (x, y, size, value, fill = '#eef3f8', anchor = 'middle') =>
    `<text x="${x}" y="${y}" font-size="${size}" fill="${fill}" text-anchor="${anchor}">${escape(value)}</text>`;
  const content = disk
    ? text(14, 44, 15, 'ROOT /', '#a4b4c7', 'start')
      + text(72, 83, 42, `${Math.round(used)}%`, color)
      + '<rect x="14" y="104" width="116" height="7" rx="4" fill="#2a3545"/>'
      + `<rect data-kind="usage" x="14" y="104" width="${116 * used / 100}" height="7" rx="4" fill="${color}"/>`
    : text(72, 78, 17, status || 'CONNECTING', '#edc56b');
  return `<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><title>${escape(status || 'DISKMETER')}</title><rect width="144" height="144" rx="16" fill="#101923"/><g font-family="Arial,Helvetica,sans-serif" font-weight="600">${text(72, 23, 18, 'DISK')}${content}</g></svg>`;
}

export function startPlugin(socket, registration, { fetchImpl = fetch, pollMs = POLL_MS } = {}) {
  const contexts = new Set();
  let disk;
  let error = '';
  let pending = false;
  const send = message => {
    if (socket.readyState === 1) socket.send(JSON.stringify(message));
  };
  const paint = () => {
    const image = `data:image/svg+xml;base64,${Buffer.from(renderKey(disk, { error })).toString('base64')}`;
    for (const context of contexts) send({ event: 'setImage', context, payload: { image, target: 0 } });
  };
  const refresh = async () => {
    if (pending || contexts.size === 0) return;
    pending = true;
    try {
      const response = await fetchImpl(`${BASE_URL}/api/dashboard`, {
        signal: AbortSignal.timeout(5000), cache: 'no-store',
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      disk = readUsage(await response.json());
      error = '';
    } catch (cause) {
      error = ['NO DATA', 'FETCH ERROR', 'BAD DATA'].includes(cause.message) ? cause.message : 'OFFLINE';
    } finally {
      pending = false;
      paint();
    }
  };
  socket.addEventListener('open', () => send(registration));
  socket.addEventListener('message', event => {
    let message;
    try { message = JSON.parse(event.data); } catch { return; }
    if (message.event === 'willAppear' && message.action === 'com.yoophi.diskmeter.usage') {
      contexts.add(message.context);
      paint();
      void refresh();
    } else if (message.event === 'willDisappear') {
      contexts.delete(message.context);
    } else if (message.event === 'keyUp' && contexts.has(message.context)) {
      send({ event: 'openUrl', payload: { url: BASE_URL } });
      void refresh();
    } else if (message.event === 'systemDidWakeUp') {
      void refresh();
    }
  });
  const timer = setInterval(() => void refresh(), pollMs);
  const stop = () => clearInterval(timer);
  socket.addEventListener('close', stop);
  return { stop, refresh };
}
