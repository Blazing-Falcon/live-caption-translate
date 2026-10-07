import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const dist = resolve(fileURLToPath(new URL('../dist', import.meta.url)));
const problems = [];

function* walk(directory) {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

for (const page of ['overlay.html', 'control.html']) {
  if (!existsSync(join(dist, page))) problems.push(`missing ${page}`);
}

const TEST_ONLY = ['__LT_BOOT', '__lt', 'FakeBackend', 'mockIPC', 'plugin:event|listen"'];

if (existsSync(dist)) {
  for (const file of walk(dist)) {
    const name = relative(dist, file);
    if (/\.(woff2?|ttf|otf|eot)$/i.test(file)) problems.push(`${name}: font file shipped`);
    if (!/\.(html|css|js)$/i.test(file)) continue;
    const text = readFileSync(file, 'utf8');
    if (/\.(html|css)$/i.test(file)) {
      if (/(?:src|href)\s*=\s*["']https?:/i.test(text)) problems.push(`${name}: external src/href`);
      if (/url\(\s*["']?(?:https?:)?\/\//i.test(text)) problems.push(`${name}: external url()`);
      if (/@import/i.test(text)) problems.push(`${name}: @import`);
    }
    if (/\.js$/i.test(file)) {
      for (const marker of TEST_ONLY.slice(0, 4)) {
        if (text.includes(marker)) problems.push(`${name}: test-only marker ${marker}`);
      }
    }
  }
}

if (problems.length > 0) {
  for (const problem of problems) console.error(`verify:dist: ${problem}`);
  process.exit(1);
}
console.log('verify:dist ok: overlay.html and control.html present, no external resources, fonts or test harness code');
