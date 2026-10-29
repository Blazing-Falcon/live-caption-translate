// Renders testdata/screen/source/*.html to testdata/screen/<name>@<scale>.png and a .truth.json
// beside each: the text, rect and rendered lines of every [data-block] element, in image pixels.
// Fonts come from Windows, so another PC can render differently; the PNGs are the record.
import { mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from '@playwright/test';

const SCALES = [1, 1.5];
const root = fileURLToPath(new URL('../../../testdata/screen/', import.meta.url));
const sourceDir = join(root, 'source');

// Runs in the page. Rects are CSS pixels; the caller scales them.
function readBlocks() {
  const rectOf = (r) => ({ left: r.left + scrollX, top: r.top + scrollY, right: r.right + scrollX, bottom: r.bottom + scrollY });
  const union = (a, b) => ({ left: Math.min(a.left, b.left), top: Math.min(a.top, b.top), right: Math.max(a.right, b.right), bottom: Math.max(a.bottom, b.bottom) });

  // One entry per rendered line: the characters whose boxes share a vertical band.
  const linesOf = (element) => {
    const lines = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    const range = document.createRange();
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      for (let i = 0; i < node.length; i++) {
        range.setStart(node, i);
        range.setEnd(node, i + 1);
        const box = range.getClientRects()[0];
        if (!box || box.width === 0 || /\s/.test(node.data[i])) continue;
        const rect = rectOf(box);
        const last = lines[lines.length - 1];
        const sameLine = last && Math.abs(rect.top - last.rect.top) < (rect.bottom - rect.top) / 2;
        if (sameLine) {
          last.text += node.data[i];
          last.rect = union(last.rect, rect);
        } else {
          lines.push({ text: node.data[i], rect });
        }
      }
    }
    return lines;
  };

  return [...document.querySelectorAll('[data-block]')]
    .sort((a, b) => Number(a.dataset.block) - Number(b.dataset.block))
    .map((element) => ({
      id: Number(element.dataset.block),
      text: element.innerText,
      rect: rectOf(element.getBoundingClientRect()),
      lines: linesOf(element),
    }));
}

// Image pixels, rounded outward so the rect always covers the box.
function toPixels(rect, scale) {
  const x = Math.floor(rect.left * scale);
  const y = Math.floor(rect.top * scale);
  return { x, y, w: Math.ceil(rect.right * scale) - x, h: Math.ceil(rect.bottom * scale) - y };
}

const squash = (text) => text.replace(/\s+/g, '');

const browser = await chromium.launch();
try {
  mkdirSync(root, { recursive: true });
  const pages = readdirSync(sourceDir).filter((file) => file.endsWith('.html')).sort();
  for (const file of pages) {
    const name = basename(file, '.html');
    for (const scale of SCALES) {
      const context = await browser.newContext({ deviceScaleFactor: scale, locale: 'zh-CN', colorScheme: 'light' });
      const page = await context.newPage();
      await page.goto(pathToFileURL(join(sourceDir, file)).href);
      await page.evaluate(() => document.fonts.ready);
      const [width, height] = await page.evaluate(() => document.body.dataset.size.split('x').map(Number));
      await page.setViewportSize({ width, height });

      const blocks = (await page.evaluate(readBlocks)).map((block) => {
        const lines = block.lines.map((line) => ({ text: line.text, rect: toPixels(line.rect, scale) }));
        if (squash(lines.map((line) => line.text).join('')) !== squash(block.text)) {
          throw new Error(`${name} block ${block.id}: rendered lines do not add up to its text`);
        }
        return { id: block.id, text: block.text, rect: toPixels(block.rect, scale), lines };
      });

      const stem = join(root, `${name}@${scale}`);
      await page.screenshot({ path: `${stem}.png` });
      const image = { width: Math.round(width * scale), height: Math.round(height * scale) };
      writeFileSync(`${stem}.truth.json`, `${JSON.stringify({ page: name, scale, image, blocks }, null, 2)}\n`);
      console.log(`${name}@${scale}: ${image.width}x${image.height}, ${blocks.length} blocks, ${blocks.reduce((sum, b) => sum + b.lines.length, 0)} lines`);
      await context.close();
    }
  }
} finally {
  await browser.close();
}
