import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import subprocess

root = Path(sys.argv[1]).resolve()
root.mkdir(parents=True, exist_ok=True)
font = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 18)
small = ImageFont.truetype('/System/Library/Fonts/Menlo.ttc', 13)
image = Image.new('RGB', (1920, 2160), '#f7f8fa')
draw = ImageDraw.Draw(image)
draw.rectangle((0, 0, 259, 2160), fill='#15202b')
for row in range(80):
    y = row * 29 + 16
    draw.text((16, y), f'{row+1:02d}  AnyDisplay', font=small, fill='#a5c8e3')
    draw.text((285, y), f'{row+1:03d}  func updateFrame(index: Int, pts: CMTime) -> Bool {{', font=font, fill='#28303c')
    draw.text((1020, y), 'H.264 / HEVC | 1920 x 1080 @ 60Hz', font=font, fill='#005bbb')
    draw.text((305, y+17), 'Small text: abcdefghijklmnopqrstuvwxyz 0123456789  [] {} ()', font=small, fill='#915000')
    draw.line((280, y+27, 1900, y+27), fill='#d5dce2', width=1)
    draw.rectangle((1710, y+2, 1880, y+17), fill=['#e24d58','#2ca86a','#377dce'][row % 3])
image.save(root/'desktop.png')
common = ['ffmpeg','-hide_banner','-loglevel','error','-y']
subprocess.run(common + ['-loop','1','-framerate','60','-i',str(root/'desktop.png'),'-vf',
    "crop=1920:1080:0:mod(n*4\\,1080),scale=in_range=full:out_range=tv:out_color_matrix=bt709,format=nv12",
    '-frames:v','180','-f','rawvideo',str(root/'desktop.nv12')], check=True)
subprocess.run(common + ['-f','lavfi','-i','testsrc2=size=1920x1080:rate=60','-vf','format=nv12',
    '-frames:v','180','-f','rawvideo',str(root/'motion.nv12')], check=True)
print('Generated desktop scroll and motion fixtures (180 frames each)')
