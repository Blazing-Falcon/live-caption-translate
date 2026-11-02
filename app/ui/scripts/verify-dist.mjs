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

for (const page of ['overlay.html', 'control.html', 'screen.html', 'tray.html']) {
  if (!existsSync(join(dist, page))) problems.push(`missing ${page}`);
}

/** The OFL fonts in src/assets/fonts, credited in notices/. Vite appends a content hash. */
const BUNDLED_FONTS = ['nunito-latin-wght', 'fredoka-latin-wght', 'atkinson-hyperlegible-latin-700'];
const isBundledFont = (file, stem) => new RegExp(`[\\\\/]${stem}-[\\w-]+\\.woff2$`).test(file);

const TEST_ONLY = ['__LT_BOOT', '__lt', 'FakeBackend', 'mockIPC', 'plugin:event|listen"'];

if (existsSync(dist)) {
  for (const file of walk(dist)) {
    const name = relative(dist, file);
    if (/\.(woff2?|ttf|otf|eot)$/i.test(file) && !BUNDLED_FONTS.some((stem) => isBundledFont(file, stem))) {
      problems.push(`${name}: font file shipped that is not one of the bundled OFL fonts`);
    }
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
console.log('verify:dist ok: overlay.html, control.html, screen.html and tray.html present, no external resources, unlisted fonts or test harness code');
