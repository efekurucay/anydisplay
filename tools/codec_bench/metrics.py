import sys
from pathlib import Path
import json
import re
import subprocess

root=Path(sys.argv[1]).resolve()
results=[]
for meta_path in sorted(root.glob('*.h264.json'))+sorted(root.glob('*.hevc.json')):
    if '.android.' in meta_path.name: continue
    encoded=Path(str(meta_path)[:-5])
    meta=json.loads(meta_path.read_text())
    assert (meta['width'], meta['height'], meta['fps'], meta['input_frames']) == (1920, 1080, 60, 180)
    assert meta['output_frames'] == 180 and meta['dropped'] == 0, 'incomplete encoder output'
    probe=subprocess.run(['ffprobe','-v','error','-count_frames','-select_streams','v:0',
        '-show_entries','stream=nb_read_frames','-of','json',str(encoded)],
        text=True,stdout=subprocess.PIPE,check=True)
    assert int(json.loads(probe.stdout)['streams'][0]['nb_read_frames']) == 180, 'decoded frame count mismatch'
    scene=encoded.name.split('-')[0]
    common=['ffmpeg','-hide_banner','-nostats','-threads','2','-r','60','-i',str(encoded),
        '-f','rawvideo','-pixel_format','nv12','-video_size','1920x1080','-framerate','60','-i',str(root/f'{scene}.nv12'),
        '-filter_complex_threads','1']
    scores={}
    for metric in ['ssim','psnr']:
        graph=f'[0:v]setpts=PTS-STARTPTS,format=yuv420p[a];[1:v]setpts=PTS-STARTPTS,format=yuv420p[b];[a][b]{metric}'
        p=subprocess.run(common+['-lavfi',graph,'-frames:v','180','-f','null','-'],text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True)
        matches=re.findall(r'All:([0-9.]+)',p.stderr) if metric=='ssim' else re.findall(r'average:([0-9.]+)',p.stderr)
        if not matches: raise RuntimeError(p.stderr)
        scores[metric]=float(matches[-1])
    row={k:v for k,v in meta.items() if k not in ['frame_sizes','keyframes']}
    row.update(scores,scene=scene,file=encoded.name)
    results.append(row)
    print(json.dumps(row),flush=True)
(root/'metrics.json').write_text(json.dumps(results,indent=2))
