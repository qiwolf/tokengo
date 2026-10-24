import {chromium} from 'playwright';
import fs from 'node:fs';
const root=process.cwd();
if(!process.env.DEMO_CREDENTIALS)throw Error('Set DEMO_CREDENTIALS to an isolated demo credential file');
const c=JSON.parse(fs.readFileSync(process.env.DEMO_CREDENTIALS,'utf8'));
if(!/^http:\/\/127\.0\.0\.1:\d+$/.test(c.origin))throw Error('Only isolated loopback fixture allowed');
const b=await chromium.launch({executablePath:'/srv/cache/playwright/manual-1248/chrome-linux64/chrome',args:['--no-sandbox']});
const ctx=await b.newContext({viewport:{width:1920,height:1080},deviceScaleFactor:2});
const r=await ctx.request.post(c.origin+'/api/account/login',{data:{username:'admin',password:c.password},headers:{Origin:c.origin,'X-Tokengo-Panel':'1'}});if(!r.ok())throw Error('Fixture login failed');
const p=await ctx.newPage();const layout={};
for(const slug of ['overview','channels','models','users','billing','shared-models']){
 await p.goto(c.origin+'/ui/#'+slug);await p.locator('.console-layout').waitFor();await p.waitForLoadState('networkidle');
 const toggle=p.getByRole('button',{name:'切换浅色',exact:true});if(await toggle.count())await toggle.click();
 // Preserve real UI; replace private fixture origin and operator identity with explicit fictional presentation data.
 await p.evaluate(()=>{const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);let n;while(n=walker.nextNode()){n.textContent=n.textContent.replace(/https?:\/\/(?:127\.0\.0\.1|localhost)(?::\d+)?(?:[^\s<>]*)?/g,'https://api.example.com').replace(/gpt-5\.4/g,'demo-chat').replace(/gpt-5-mini/g,'demo-small');}});
 await p.evaluate(()=>document.fonts.ready);await p.waitForTimeout(700);
 const text=await p.locator('body').innerText();if(/sk-[a-zA-Z0-9]{12,}|192\.168\.|10\.0\.|127\.0\.0\.1/.test(text))throw Error('Unsafe screenshot content');
 await p.screenshot({path:root+'/public/textures/'+slug+'.png'});
 layout[slug]={viewport:{width:1920,height:1080},pageH:1080,source:'real isolated TokenGo console; fictional fixture data',elements:await p.locator('.console-sidebar,.page-head,.console-content').evaluateAll(es=>es.map(e=>{const r=e.getBoundingClientRect();return {selector:e.className,x:r.x,y:r.y,w:r.width,h:r.height}}))};
 await p.locator('.console-main .console-content').first().screenshot({path:root+'/public/textures/'+slug+'-content.png'});
}
fs.writeFileSync(root+'/public/textures/layout.json',JSON.stringify(layout,null,2));await b.close();console.log('Captured 6 real console pages at 2x with fictional data; no production access');
