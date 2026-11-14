import assert from 'node:assert/strict';
import { join } from 'node:path';
import { expect } from '@playwright/test';

// Call only after the native runner has verified its owned fixture profile.
// Use real disclosure/scroll interactions; never alter the generated values.
export async function captureDocumentBlocks(page, expected, directory, prefix) {
  assert(expected.length > 0, 'A body screenshot needs generated blocks');
  const options = page.locator('.doc-options');
  if ((await options.getAttribute('open')) !== null)
    await options.locator(':scope > summary').click();
  await expect(options).not.toHaveAttribute('open');

  const editors = page.locator('.doc-block textarea');
  await expect(editors).toHaveCount(expected.length);
  const images = [];
  for (const [index, text] of expected.entries()) {
    const editor = editors.nth(index);
    await expect(editor).toHaveValue(text);
    await editor.scrollIntoViewIfNeeded();
    // Long Markdown inputs may exceed the inner pane. Show their beginning
    // without resizing them or pretending the whole value fits in one image.
    await editor.evaluate((element) => {
      const pane = element.closest('.doc-edit-scroll');
      if (!pane) throw new Error('Document editor scroll pane not found');
      pane.scrollTop += element.getBoundingClientRect().top - pane.getBoundingClientRect().top - 12;
      element.scrollTop = 0;
    });
    await expect(editor).toBeInViewport();
    const visibility = await editor.evaluate((element) => {
      const box = element.getBoundingClientRect();
      const pane = element.closest('.doc-edit-scroll').getBoundingClientRect();
      const left = Math.max(box.left, pane.left, 0);
      const right = Math.min(box.right, pane.right, innerWidth);
      const top = Math.max(box.top, pane.top, 0);
      const bottom = Math.min(box.bottom, pane.bottom, innerHeight);
      const width = Math.max(0, right - left);
      const height = Math.max(0, bottom - top);
      return {
        x: box.x,
        y: box.y,
        width: box.width,
        height: box.height,
        visibleWidth: width,
        visibleHeight: height,
        requiredWidth: Math.min(box.width, pane.width, innerWidth) * 0.95,
        requiredHeight: Math.min(box.height, pane.height - 24, innerHeight) * 0.95,
        entireEditorVisible: width >= box.width * 0.95 && height >= box.height * 0.95,
        centerHitsEditor: document.elementFromPoint(left + width / 2, top + height / 2) === element,
      };
    });
    assert(visibility.visibleWidth >= visibility.requiredWidth, 'The block width must be visible');
    assert(visibility.visibleHeight >= visibility.requiredHeight, 'The block body must be visible');
    assert(visibility.centerHitsEditor, 'The generated block must not be covered or clipped');
    const name = index === 0 ? `${prefix}.png` : `${prefix}-block-${index + 1}.png`;
    await page.screenshot({ path: join(directory, name) });
    images.push({ index, name, visibility });
  }
  assert.deepEqual(
    await editors.evaluateAll((inputs) => inputs.map((input) => input.value)),
    expected,
    'Preparing screenshot evidence must preserve every block value',
  );
  return images;
}
