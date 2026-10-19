import { startPlugin } from './usage.mjs';

const args = process.argv.slice(2);
const arg = name => args[args.indexOf(name) + 1];
const port = arg('-port');
if (!/^\d+$/.test(port ?? '') || !args.includes('-pluginUUID') || !args.includes('-registerEvent')) {
  throw new Error('Launch this plugin through Stream Deck.');
}
const socket = new WebSocket(`ws://127.0.0.1:${port}`);
startPlugin(socket, { event: arg('-registerEvent'), uuid: arg('-pluginUUID') });
socket.addEventListener('error', () => {
  console.error('DiskMeter: Stream Deck WebSocket connection failed.');
  process.exitCode = 1;
});
