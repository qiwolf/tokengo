import React from 'react';
import {Composition} from 'remotion';
import {Main} from './Main';
export const Root=()=> <Composition id="TokenGoIntro" component={Main} durationInFrames={1800} fps={30} width={1920} height={1080} defaultProps={{bgm:true}}/>;
