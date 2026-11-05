import {test,expect} from '@playwright/test';
import {withFieldHost} from './field-host.mjs';

test('Escape returns tablet actions focus to More',async({page})=>{
  // Closing a responsive menu must leave its keyboard user on a visible
  // control rather than on the document body or a hidden secondary action.
  await withFieldHost(async url=>{
    await page.setViewportSize({width:820,height:1000});await page.goto(url);
    const more=page.getByRole('button',{name:'More',exact:true});
    await more.click();
    await page.getByRole('button',{name:'Export CSV',exact:true}).focus();
    await page.keyboard.press('Escape');
    await expect(more).toHaveAttribute('aria-expanded','false');
    await expect(more).toBeFocused();
  });
});
