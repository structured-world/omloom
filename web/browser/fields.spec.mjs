import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';

for (const width of [1440, 390]) test(`integer forms preserve empty and exact int64 values at ${width}px`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  // This follows the rendered form through the real API, not a mocked field parser.
  await withFieldHost(async url => {
    await page.goto(url);
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Typed resource');
    await page.getByRole('dialog').getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    if (width === 390) {
      await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
      await page.getByRole('button', { name: 'More', exact: true }).click();
    }
    await page.getByRole('button', { name: 'Company fields', exact: true }).click();
    let dialog=page.getByRole('dialog');
    await dialog.getByLabel('Name', { exact: true }).fill('Capacity');
    await dialog.getByRole('combobox', { name: 'Type', exact: true }).selectOption({ label: 'integer' });
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
    await dialog.getByRole('checkbox', { name: 'virtual machine', exact: true }).check();
    await dialog.getByRole('button', { name: 'Review / save definition', exact: true }).click();
    await expect(dialog).toBeHidden();
    if (width === 390) await page.getByRole('button', { name: 'Typed resource', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: 'Typed resource', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
    dialog=page.getByRole('dialog');
    await dialog.getByLabel('Capacity change', { exact: true }).selectOption('set');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('alert')).toContainText('Empty input is not zero');
    await dialog.getByLabel('Capacity', { exact: true }).fill('0');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Capacity');
    await expect(page.getByRole('region', { name: 'Resource details' }).locator('dd').filter({ hasText: /^0$/ })).toHaveCount(1);
    // int64 input must never pass through an imprecise JavaScript Number.
    for (const exact of ['9007199254740993', '9223372036854775807', '-9223372036854775808']) {
      await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
      dialog = page.getByRole('dialog');
      await dialog.getByLabel('Capacity change', { exact: true }).selectOption('set');
      await dialog.getByLabel('Capacity', { exact: true }).fill(exact);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).locator('dd').filter({ hasText: new RegExp(`^${exact}$`) })).toHaveCount(1);
    }
    // Bulk edits use the same typed contract and leave every unselected field intact.
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select Typed resource', exact: true }).check();
    await page.getByRole('button', { name: 'Review bulk change', exact: true }).click();
    dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Field', exact: true }).selectOption({ label: 'Capacity' }, { timeout: 5000 });
    await dialog.getByRole('combobox', { name: 'Change', exact: true }).selectOption('set', { timeout: 5000 });
    await dialog.getByRole('button', { name: 'Apply reviewed change', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Empty input is not zero');
    await dialog.getByLabel('Value', { exact: true }).fill('9007199254740993');
    await dialog.getByRole('button', { name: 'Apply reviewed change', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: 'Typed resource', exact: true }).click();
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' }).locator('dd').filter({ hasText: /^9007199254740993$/ })).toHaveCount(1);
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: 'Typed resource', exact: true })).toBeVisible();
    // Applicability removal is separately reviewed; it cannot silently erase a value.
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Add filter', exact: true }).click();
    dialog = page.getByRole('dialog');
    await dialog.getByLabel('Field', { exact: true }).selectOption({ label: 'Capacity' });
    await dialog.getByLabel('Equals', { exact: true }).fill('9007199254740993');
    await dialog.getByRole('button', { name: 'Apply filter', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('button', { name: 'Typed resource', exact: true })).toBeVisible();
    expect(new URL(page.url()).hash).toContain('9007199254740993');
    await page.getByRole('button', { name: 'Remove filter 1', exact: true }).click();
    if (width === 1440) await page.getByRole('button', { name: 'Typed resource', exact: true }).click();
    if (width === 390) await page.getByRole('button', { name: 'More', exact: true }).click();
    await page.getByRole('button', { name: 'Company fields', exact: true }).click();
    dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Definition', exact: true }).selectOption({ label: 'Capacity' });
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).uncheck();
    await dialog.getByRole('button', { name: 'Review / save definition', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Migration preview' })).toContainText('1 values need repair');
    await expect(dialog.getByRole('button', { name: 'Apply reviewed migration', exact: true })).toBeDisabled();
    await dialog.getByRole('checkbox', { name: 'Remove local values from excluded resource kinds', exact: true }).check();
    await dialog.getByRole('button', { name: 'Review / save definition', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Migration preview' })).toContainText('1 values will be removed. 0 values need repair');
    await dialog.getByRole('button', { name: 'Apply reviewed migration', exact: true }).click();
    await expect(dialog).toBeHidden();
    if (width === 390) await page.getByRole('button', { name: 'Typed resource', exact: true }).click();
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Capacity');
  });
});
