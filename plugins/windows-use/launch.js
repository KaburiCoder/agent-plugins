// Starts the windows-use MCP server for the plugin, choosing the server in this order:
//   1. WINDOWS_USE_EXE, if set: any exe you point it at
//   2. a local build in this checkout (target/release, then target/debug): plugin installed
//      from the repository folder (`/plugin marketplace add D:/path/to/agent-plugins`)
//   3. the npm package of this plugin's version (`npx -y windows-use-mcp@<version>`)
// A local build is run from a copy in %TEMP%, so `cargo build` can replace it while the
// server runs; reconnect the server (/mcp) to pick up a new build.
'use strict';
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const root = __dirname;
const version = JSON.parse(fs.readFileSync(path.join(root, '.claude-plugin', 'plugin.json'), 'utf8')).version;

function localBuild() {
  if (process.env.WINDOWS_USE_EXE) return process.env.WINDOWS_USE_EXE;
  for (const profile of ['release', 'debug']) {
    const exe = path.join(root, '..', '..', 'target', profile, 'windows-use.exe');
    if (fs.existsSync(exe)) return exe;
  }
  return null;
}

// Copies the exe to %TEMP% under a name that changes with every build, and removes copies of
// earlier builds that are no longer running.
function runnableCopy(exe) {
  const stat = fs.statSync(exe);
  const dir = path.join(os.tmpdir(), 'windows-use-dev');
  fs.mkdirSync(dir, { recursive: true });
  const name = `windows-use-${stat.size}-${Math.trunc(stat.mtimeMs)}.exe`;
  const copy = path.join(dir, name);
  if (!fs.existsSync(copy)) fs.copyFileSync(exe, copy);
  for (const old of fs.readdirSync(dir)) {
    if (old !== name) fs.rm(path.join(dir, old), { force: true }, () => {});
  }
  return copy;
}

function start(command, args) {
  const child = spawn(command, args, { stdio: 'inherit' });
  child.on('error', (e) => {
    console.error(`windows-use: cannot start ${command}: ${e.message}`);
    process.exit(1);
  });
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill(signal));
  child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
}

if (process.platform !== 'win32') {
  console.error('windows-use runs only on Windows.');
  process.exit(1);
}
const exe = localBuild();
if (exe) {
  console.error(`windows-use: local build ${exe}`);
  start(runnableCopy(exe), []);
} else {
  start('cmd', ['/c', 'npx', '-y', `windows-use-mcp@${version}`]);
}
