import {bundle} from '@remotion/bundler';
import {selectComposition,renderStill,renderMedia} from '@remotion/renderer';
import fs from 'node:fs';
const browserExecutable='/srv/cache/playwright/manual-1248/chrome-linux64/chrome';
const serveUrl=await bundle({entryPoint:'src/index.ts'});
const composition=await selectComposition({serveUrl,id:'TokenGoIntro',browserExecutable,chromiumOptions:{gl:'angle'}});
fs.mkdirSync('out/qa',{recursive:true});
const frames=[45,120,225,360,465,600,705,840,945,1065,1185,1320,1425,1560,1665,1755];
for(const frame of frames){await renderStill({composition,serveUrl,output:`out/qa/f${frame}.png`,frame,browserExecutable,chromiumOptions:{gl:'angle'}});console.log('still',frame);}
// Render picture once, then preserve it exactly while replacing only the audio track.
let last=-1;
await renderMedia({composition,serveUrl,outputLocation:'out/tokengo-intro.mp4',codec:'h264',audioCodec:'aac',crf:20,pixelFormat:'yuv420p',inputProps:{bgm:true},browserExecutable,chromiumOptions:{gl:'angle'},concurrency:4,onProgress:p=>{const n=Math.floor(p.progress*10);if(n!==last){last=n;console.log('music master',n*10+'%');}}});
const silentComposition=await selectComposition({serveUrl,id:'TokenGoIntro',inputProps:{bgm:false},browserExecutable,chromiumOptions:{gl:'angle'}});
await renderMedia({composition:silentComposition,serveUrl,outputLocation:'out/sfx-only.wav',codec:'wav',inputProps:{bgm:false},browserExecutable,chromiumOptions:{gl:'angle'}});
const {execFileSync}=await import('node:child_process');
execFileSync('ffmpeg',['-v','error','-y','-i','out/tokengo-intro.mp4','-i','out/sfx-only.wav','-map','0:v:0','-map','1:a:0','-c:v','copy','-c:a','aac','-b:a','192k','-movflags','+faststart','out/tokengo-intro-no-bgm.mp4'],{stdio:'inherit'});
console.log('complete: music master and SFX-only version; identical video stream');
