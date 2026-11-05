import {test,expect} from '@playwright/test';
import {withFieldHost} from './field-host.mjs';
import {workspaceClient} from './workspace-client.mjs';

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

for(const width of [390,820])test(`3D toggle returns responsive focus to More at ${width}px`,async({page})=>{
  // Activating the secondary action must not leave focus on a hidden button.
  await withFieldHost(async url=>{
    await page.setViewportSize({width,height:900});await page.goto(url);
    const more=page.getByRole('button',{name:'More',exact:true});
    for(const pressed of ['true','false']){
      await more.click();
      const toggle=page.getByRole('button',{name:'3D scene',exact:true});
      await toggle.focus();await page.keyboard.press('Enter');
      await expect(more).toHaveAttribute('aria-expanded','false');
      await expect(more).toBeFocused();
      await more.click();await expect(toggle).toHaveAttribute('aria-pressed',pressed);
      await page.keyboard.press('Escape');
    }
  });
});

for(const width of [390,1440])test(`search controls retain their individual focus outlines at ${width}px`,async({page})=>{
  // A clipped compound search must show which control is focused, including
  // the input-to-button keyboard transition, without clipping its outline.
  await withFieldHost(async url=>{
    const {m,rpc}=await workspaceClient(url);
    await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:'Searchable device'}}]})});
    await page.setViewportSize({width,height:900});await page.goto(url);
    const input=page.getByRole('textbox',{name:'Search inventory',exact:true});
    const button=page.getByRole('button',{name:'Search',exact:true});
    await input.focus();
    for(const control of [input,button]){
      await expect(control).toBeFocused();
      const outline=await control.evaluate(element=>{
        const style=getComputedStyle(element);
        return {visible:element.matches(':focus-visible')&&style.outlineStyle!=='none'&&parseFloat(style.outlineWidth)>0,
          outerEdge:parseFloat(style.outlineOffset)+parseFloat(style.outlineWidth)};
      });
      expect(outline.visible).toBe(true);
      expect(outline.outerEdge).toBeLessThanOrEqual(0);
      await page.screenshot({path:test.info().outputPath(`search-${width}-${control===input?'input':'button'}.png`)});
      if(control===input)await page.keyboard.press('Tab');
    }
  });
});
