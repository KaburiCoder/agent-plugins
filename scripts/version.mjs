// Keeps one plugin's version the same everywhere it is written.
//   node scripts/version.mjs <plugin>               check that they agree
//   node scripts/version.mjs <plugin> --set 0.2.0   write a new version to every file
//   node scripts/version.mjs <plugin> --expect 0.2.0  check, and that they equal 0.2.0 (CI)
import { readFileSync, writeFileSync } from 'node:fs';

const [plugin, flag, wanted] = process.argv.slice(2);
if (!plugin || (flag && !['--set', '--expect'].includes(flag)) || (flag && !wanted)) {
  console.error('usage: node scripts/version.mjs <plugin> [--set|--expect <version>]');
  process.exit(2);
}
const pkg = `${plugin}-mcp`;
const file = (p) => new URL(`../${p}`, import.meta.url);
const read = (p) => readFileSync(file(p), 'utf8');
const write = (p, s) => writeFileSync(file(p), s);

const files = {
  'Cargo.toml': {
    path: `servers/${plugin}/Cargo.toml`,
    get: (s) => s.match(/^version\s*=\s*"([^"]+)"/m)?.[1],
    set: (s, v) => s.replace(/^version\s*=\s*"[^"]+"/m, `version = "${v}"`),
  },
  'plugin.json': {
    path: `plugins/${plugin}/.claude-plugin/plugin.json`,
    get: (s) => JSON.parse(s).version,
    set: (s, v) => s.replace(/"version":\s*"[^"]+"/, `"version": "${v}"`),
  },
  '.mcp.json': {
    path: `plugins/${plugin}/.mcp.json`,
    get: (s) => s.match(new RegExp(`"${pkg}@([^"]+)"`))?.[1],
    set: (s, v) => s.replace(new RegExp(`"${pkg}@[^"]+"`), `"${pkg}@${v}"`),
  },
  'package.json': {
    path: `npm/${pkg}/package.json`,
    get: (s) => JSON.parse(s).version,
    set: (s, v) => s.replace(/"version":\s*"[^"]+"/, `"version": "${v}"`),
  },
};

if (flag === '--set') {
  for (const f of Object.values(files)) write(f.path, f.set(read(f.path), wanted));
}
const found = Object.fromEntries(Object.entries(files).map(([k, f]) => [k, f.get(read(f.path))]));
const versions = new Set(Object.values(found));
if (versions.size !== 1 || versions.has(undefined) || (flag === '--expect' && !versions.has(wanted))) {
  console.error(`${plugin}: versions differ`, flag === '--expect' ? { tag: wanted, ...found } : found);
  process.exit(1);
}
console.log(`${plugin} ${[...versions][0]}`);
