"""Le .jit (textura da UI do Aika) via PIL, embrulhando em DDS.
JT35 = DXT5, cabecalho de 12 bytes: magia(4) largura(4) altura(4)
JT20 = ARGB32 cru, dimensoes em 0x10/0x12
"""
import struct, io
from PIL import Image

def ler(path):
    d=open(path,'rb').read()
    m=d[:4]
    if m==b'JT35':
        w,h = struct.unpack_from('<II', d, 4)
        dxt = d[12:]
        hdr = bytearray(128)
        hdr[0:4]=b'DDS '; struct.pack_into('<I',hdr,4,124)
        struct.pack_into('<I',hdr,8,0x1|0x2|0x4|0x1000|0x80000)   # flags
        struct.pack_into('<I',hdr,12,h); struct.pack_into('<I',hdr,16,w)
        struct.pack_into('<I',hdr,20,max(1,w*h))                  # linearsize
        struct.pack_into('<I',hdr,76,32); struct.pack_into('<I',hdr,80,0x4)
        hdr[84:88]=b'DXT5'
        struct.pack_into('<I',hdr,108,0x1000)
        return Image.open(io.BytesIO(bytes(hdr)+dxt)).convert('RGBA'), f"{w}x{h} DXT5"
    if m==b'JT20':
        w,h = struct.unpack_from('<HH', d, 16)
        px = d[30:30+w*h*4]
        if len(px) < w*h*4: raise ValueError(f"dados curtos {len(px)} < {w*h*4}")
        return Image.frombytes('RGBA',(w,h),px,'raw','BGRA'), f"{w}x{h} ARGB"
    raise ValueError(f"magia desconhecida {m!r}")
