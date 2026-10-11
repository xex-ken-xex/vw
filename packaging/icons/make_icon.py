"""vw のアイコン（packaging/icons の vw.ico / vw.png / vw-64.rgba）を作る。

    python3 packaging/icons/make_icon.py packaging/icons

Pillow が要る。絵を変えたときだけ実行し、できたファイルをコミットする。
"""
import sys
from PIL import Image, ImageDraw, ImageFilter
N=1024
def rr(d,box,r,**k): d.rounded_rectangle(box,radius=r,**k)
img=Image.new("RGBA",(N,N),(0,0,0,0))
# 背景: 濃い灰色の角丸の板に、上から下へ少し明るさを変える
bg=Image.new("RGBA",(N,N))
for y in range(N):
    t=y/N
    c=tuple(int(a+(b-a)*t) for a,b in zip((58,62,70),(30,32,37)))
    ImageDraw.Draw(bg).line([(0,y),(N,y)],fill=c+(255,))
mask=Image.new("L",(N,N),0); rr(ImageDraw.Draw(mask),(40,40,N-40,N-40),200,fill=255)
img.paste(bg,(0,0),mask)
d=ImageDraw.Draw(img)
# 後ろの写真（複数枚並べるビューワの印）
rr(d,(250,170,850,620),60,fill=(84,140,220,255))
# 手前の写真
fx0,fy0,fx1,fy1=170,330,770,830
rr(d,(fx0-26,fy0-26,fx1+26,fy1+26),80,fill=(240,242,245,255))
photo=Image.new("RGBA",(fx1-fx0,fy1-fy0))
pd=ImageDraw.Draw(photo)
w,h=photo.size
for y in range(h):
    t=y/h
    c=tuple(int(a+(b-a)*t) for a,b in zip((96,170,255),(170,215,255)))
    pd.line([(0,y),(w,y)],fill=c+(255,))
pd.ellipse((w*0.62,h*0.14,w*0.84,h*0.36),fill=(255,214,90,255))
pd.polygon([(0,h),(w*0.36,h*0.42),(w*0.62,h*0.78),(w*0.62,h)],fill=(46,110,70,255))
pd.polygon([(w*0.38,h),(w*0.70,h*0.56),(w,h*0.86),(w,h)],fill=(32,86,56,255))
pm=Image.new("L",photo.size,0); rr(ImageDraw.Draw(pm),(0,0,w,h),56,fill=255)
img.paste(photo,(fx0,fy0),pm)
out=sys.argv[1]
img.resize((256,256),Image.LANCZOS).save(f"{out}/vw.png")
sizes=[(s,s) for s in (16,20,24,32,40,48,64,128,256)]
img.resize((256,256),Image.LANCZOS).save(f"{out}/vw.ico",sizes=sizes)
# 起動時のウインドウのアイコン用。PNG のままだと eframe で読むのに image クレートが要るので、生の RGBA で持つ。
open(f"{out}/vw-64.rgba","wb").write(img.resize((64,64),Image.LANCZOS).tobytes())
