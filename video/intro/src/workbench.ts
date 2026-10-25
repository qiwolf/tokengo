import React from 'react';
import {Audio,interpolate,staticFile,useCurrentFrame} from 'remotion';
import {Scene,Main,SHOTS,SFX,TOTAL} from './Main';
const EditableScene=(shot:typeof SHOTS[number])=>React.createElement(Scene,{shot});
const Score=({volume=.85}:{volume?:number})=>{
 const f=useCurrentFrame();
 return React.createElement(Audio,{src:staticFile('audio/score.wav'),volume:interpolate(f,[0,60,TOTAL-90,TOTAL],[0,volume,volume,0],{extrapolateLeft:'clamp',extrapolateRight:'clamp'})});
};
export const WORKBENCH={name:'TokenGo Intro · q18',revision:'2',fps:30,width:1920,height:1080,total:TOTAL,background:'#f7f8fa',
 shots:SHOTS.map(shot=>({id:shot.id,label:shot.zh,from:shot.from,duration:shot.duration,component:EditableScene,props:{...shot},durationProp:'duration',schema:[
  {type:'textarea',key:'zh',label:'中文标题',default:shot.zh},
  {type:'textarea',key:'en',label:'English title',default:shot.en},
  {type:'textarea',key:'sub',label:'中文字幕',default:shot.sub},
  {type:'textarea',key:'subEn',label:'English caption',default:shot.subEn}
 ]})),
 transitions:[],captions:[],overlays:[{id:'original-score',label:'原创配乐 · Original score',from:0,duration:TOTAL,component:Score,props:{volume:.85},schema:[{type:'number',key:'volume',label:'音量',default:.85,min:0,max:1,step:.05}]}],
 sfx:SFX.map(s=>({...s})),bgm:[],order:['overlays'],original:Main};
export const project=WORKBENCH;
