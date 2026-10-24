import {Scene,Main,SHOTS,SFX,TOTAL} from './Main';
export const project={name:'TokenGo Intro',revision:'1',fps:30,width:1920,height:1080,total:TOTAL,
 scenes:SHOTS.map(shot=>({id:shot.id,label:shot.zh,from:shot.from,duration:shot.duration,component:Scene,props:{shot},schema:[]})),
 transitions:[],captions:[],overlays:[],sfx:SFX.map(s=>({...s})),bgm:[{from:0,duration:TOTAL,src:'audio/score.wav',volume:.85}],order:[],original:Main};
