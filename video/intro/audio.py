"""Deterministic original ambient score and short air transition. No third party samples."""
from pathlib import Path
import math,random,struct,wave
P=Path(__file__).parent/'public'/'audio';P.mkdir(exist_ok=True)
rate=48000;total=84;random.seed(25)
chords=[(130.8128,164.8138,195.9977),(110,130.8128,164.8138),(87.3071,110,130.8128),(97.9989,123.4708,146.8324)]
with wave.open(str(P/'score.wav'),'wb') as w:
 w.setnchannels(2);w.setsampwidth(2);w.setframerate(rate)
 for start in range(0,rate*total,rate):
  out=bytearray()
  for i in range(start,min(start+rate,rate*total)):
   t=i/rate;c=chords[int(t//7.5)%4];phase=t%7.5
   env=min(1,t/2,(total-t)/3)*min(1,phase/.7,(7.5-phase)/.9)
   left=sum(math.sin(2*math.pi*f*t)+.20*math.sin(2*math.pi*f*2*t) for f in c)/3*.15*env
   right=sum(math.sin(2*math.pi*(f+.13)*t)+.20*math.sin(2*math.pi*f*2*t) for f in c)/3*.15*env
   # soft pulse on 64 BPM; intentionally ambient, no camera pumping
   beat=t%(60/64);pulse=.025*math.sin(2*math.pi*55*t)*math.exp(-beat*15)*min(1,t/3,(total-t)/3)
   out.extend(struct.pack('<hh',int((left+pulse)*32767),int((right+pulse)*32767)))
  w.writeframesraw(out)
with wave.open(str(P/'air.wav'),'wb') as w:
 w.setnchannels(1);w.setsampwidth(2);w.setframerate(rate);out=bytearray();prev=0
 for i in range(int(rate*.7)):
  t=i/rate;prev=.94*prev+.06*random.uniform(-1,1);v=prev*.25*math.sin(math.pi*t/.7)**2
  out.extend(struct.pack('<h',int(v*32767)))
 w.writeframes(out)
print('Original score and air SFX generated; seeded deterministic source')
