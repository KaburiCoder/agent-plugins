#!/usr/bin/env node
// Starts the bundled windows-use.exe with this process's stdio, so the MCP client talks to
// the server directly.
'use strict';
const { spawn } = require('node:child_process');
const path = require('node:path');

if (process.platform !== 'win32') {
  console.error('windows-use-mcp runs only on Windows.');
  process.exit(1);
}

const exe = path.join(__dirname, 'windows-use.exe');
const child = spawn(exe, process.argv.slice(2), { stdio: 'inherit' });
child.on('error', (e) => {
  console.error(`windows-use-mcp: cannot start ${exe}: ${e.message}`);
  process.exit(1);
});
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => child.kill(signal));
}
child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
