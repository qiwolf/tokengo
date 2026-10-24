"""Verify final video properties, full decodability, audio headroom and visual parity."""
from pathlib import Path
import subprocess,json,hashlib,re,socket
P=Path(__file__).parent;O=P/'out';E=O/'verification';E.mkdir(exist_ok=True,parents=True)
assert socket.gethostname()=='build-test','This delivery is verified on build-test'
def run(args):
 r=subprocess.run(args,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 if r.returncode:raise RuntimeError(str(args)+r.stderr)
 return r
results={'host':socket.gethostname(),'files':{},'commands':[]}
for name in ['tokengo-intro.mp4','tokengo-intro-no-bgm.mp4']:
 p=O/name;cmd=['ffprobe','-v','error','-show_format','-show_streams','-of','json',str(p)];r=run(cmd);data=json.loads(r.stdout);(E/(name+'.ffprobe.json')).write_text(r.stdout)
 v=next(s for s in data['streams'] if s['codec_type']=='video');a=next(s for s in data['streams'] if s['codec_type']=='audio')
 assert (v['width'],v['height'],v['codec_name'],v['r_frame_rate'])==(1920,1080,'h264','30/1');assert v['pix_fmt'] in ['yuv420p','yuvj420p'];assert abs(float(data['format']['duration'])-60)<.1
 assert int(v['nb_frames'])==1800 and a['codec_name']=='aac'
 cmd2=['ffmpeg','-v','error','-i',str(p),'-f','null','-'];r=run(cmd2);(E/(name+'.decode.log')).write_text(r.stderr or 'Full decode passed; no reported errors\n')
 cmd3=['ffmpeg','-hide_banner','-i',str(p),'-af','volumedetect','-vn','-f','null','-'];r=run(cmd3);(E/(name+'.volume.log')).write_text(r.stderr);peak=float(re.search(r'max_volume: ([-\d.]+)',r.stderr).group(1));assert peak<-.5
 cmd4=['ffmpeg','-v','error','-i',str(p),'-map','0:v','-f','framemd5','-'];frames=run(cmd4).stdout; (E/(name+'.framemd5')).write_text(frames)
 digest=hashlib.sha256('\n'.join(line for line in frames.splitlines() if not line.startswith('#')).encode()).hexdigest()
 head_audio=run(['ffmpeg','-hide_banner','-i',str(p),'-t','3','-af','volumedetect','-vn','-f','null','-']).stderr
 (E/(name+'.intro-volume.log')).write_text(head_audio)
 intro_peak=float(re.search(r'max_volume: ([-\d.]+)',head_audio).group(1))
 if 'no-bgm' in name:assert intro_peak < -75,'No-music intro must be silent before first SFX'
 else:assert intro_peak > -60,'Music master intro must contain the score'
 audio_hash=run(['ffmpeg','-v','error','-i',str(p),'-map','0:a','-f','md5','-']).stdout.strip()
 results['files'][name]={'intro_peak_db':intro_peak,'decoded_audio_hash':audio_hash,'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'size':p.stat().st_size,'width':v['width'],'height':v['height'],'pixel_format':v['pix_fmt'],'fps':v['r_frame_rate'],'frames':int(v['nb_frames']),'duration':float(data['format']['duration']),'audio_peak_db':peak,'decoded_frames_hash':digest,'full_decode_exit_code':0}
 results['commands'].extend([cmd,cmd2,cmd3,cmd4])
assert len({v['decoded_audio_hash'] for v in results['files'].values()})==2,'Music and SFX-only variants must have different audio'
assert len({v['decoded_frames_hash'] for v in results['files'].values()})==1,'BGM variants must have identical video frames'
frames=[45,120,225,360,465,600,705,840,945,1065,1185,1320,1425,1560,1665,1755]
Q=O/'qa-final';Q.mkdir(exist_ok=True)
sel='+'.join(f'eq(n\\,{n})' for n in frames)
run(['ffmpeg','-v','error','-y','-i',str(O/'tokengo-intro.mp4'),'-vf',f'select={sel}', '-fps_mode','vfr',str(Q/'shot-%02d.png')])
run(['ffmpeg','-v','error','-y','-i',str(O/'tokengo-intro.mp4'),'-ss','3','-frames:v','1',str(O/'tokengo-intro-poster.jpg')])
run(['ffmpeg','-v','error','-y','-framerate','1','-i',str(Q/'shot-%02d.png'),'-vf','scale=480:270,tile=4x4','-frames:v','1',str(E/'contact-sheet.jpg')])
results['final_extracted_frames']=frames
(E/'verification.json').write_text(json.dumps(results,indent=2));print(json.dumps(results['files'],indent=2));print('PASS: 1080p / 60 seconds / 1800 frames / H264+AAC / full decode / no clipping / identical visual variants')
