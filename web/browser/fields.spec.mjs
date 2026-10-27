import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('empty optional integer is not silently accepted as zero', async ({ page }) => {
  // This follows the rendered form through the real API, not a mocked field parser.
  const data = await mkdtemp(join(tmpdir(), 'omloom-field-ui-'));
  const child = spawn('target/debug/omloom-dev', ['--data', data, '--ui', 'dist/app', '--fixture', 'operator'], { stdio: ['ignore', 'pipe', 'pipe'] });
  const url = await new Promise((resolve,reject) => {
    let log=''; const timer=setTimeout(()=>reject(Error(`Startup timed out: ${log}`)),20000);
    child.stdout.on('data',chunk=>{log+=chunk;const match=log.match(/http:\/\/127\.0\.0\.1:\d+/);if(match){clearTimeout(timer);resolve(match[0]);}});
    child.stderr.on('data',chunk=>{log+=chunk;}); child.once('exit',code=>{clearTimeout(timer);reject(Error(`Host exited ${code}: ${log}`));});
  });
  try {
    await page.goto(url);
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Typed resource');
    await page.getByRole('dialog').getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await page.getByRole('button', { name: 'Company fields', exact: true }).click();
    let dialog=page.getByRole('dialog');
    await dialog.getByLabel('Name', { exact: true }).fill('Capacity');
    await dialog.getByRole('combobox', { name: 'Type', exact: true }).selectOption({ label: 'integer' });
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
    await dialog.getByRole('checkbox', { name: 'virtual machine', exact: true }).check();
    await dialog.getByRole('button', { name: 'Review / save definition', exact: true }).click();
    await expect(dialog).toBeHidden();
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
    // Bulk edits use the same typed contract and leave every unselected field intact.
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select Typed resource', exact: true }).check();
    await page.getByRole('button', { name: 'Review bulk change', exact: true }).click();
    dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Field', exact: true }).selectOption({ label: 'Capacity' }, { timeout: 5000 });
    await dialog.getByRole('combobox', { name: 'Change', exact: true }).selectOption('set', { timeout: 5000 });
    await dialog.getByRole('button', { name: 'Apply reviewed change', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Empty input is not zero');
    await dialog.getByLabel('Value', { exact: true }).fill('42');
    await dialog.getByRole('button', { name: 'Apply reviewed change', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: 'Typed resource', exact: true }).click();
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' }).locator('dd').filter({ hasText: /^42$/ })).toHaveCount(1);
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: 'Typed resource', exact: true })).toBeVisible();
    // Applicability removal is separately reviewed; it cannot silently erase a value.
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
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Capacity');
  } finally {
    if (child.exitCode===null && child.signalCode===null) { const exit=new Promise(resolve=>child.once('exit',resolve));child.kill('SIGINT');await exit; }
    await rm(data,{recursive:true,force:true});
  }
});
