"""Le .jit (textura da UI do Aika) via PIL, embrulhando em DDS.
JT35 = DXT5, cabecalho de 12 bytes: magia(4) largura(4) altura(4)
JT20 = ARGB32 cru, dimensoes em 0x10/0x12
"""
import struct, io
from PIL import Image

def rle_descomprime(d):
    """JT20 com flag 0x0A: RLE sobre pixels de 4 bytes, cabecalho de 22 bytes.

      controle C >= 0x80 -> repete (C - 0x7F) vezes o pixel que segue
      controle C <  0x80 -> seguem (C + 1) pixels literais

    Validado pelo unico criterio que nao mente: nos tres arquivos que usam esse
    formato (ItemIcons09, ItemIcons12 e win22 do cliente TK) sai exatamente
    largura*altura pixels deixando exatamente 8 bytes de rodape.
    """
    w,h = struct.unpack_from('<HH', d, 16)
    p, px, out = 22, 0, bytearray()
    while px < w*h:
        c = d[p]; p += 1
        if c & 0x80:
            n = c - 0x7F; out += d[p:p+4] * n; p += 4
        else:
            n = c + 1;    out += d[p:p+4*n];   p += 4*n
        px += n
    return bytes(out)

def escrever_jt20(caminho, im):
    """Grava JT20 cru (flag 0x02), que e o que o cliente BR le."""
    w,h = im.size
    cab = bytearray(30)
    cab[0:4] = b'JT20'; cab[6] = 0x02
    struct.pack_into('<HHH', cab, 16, w, h, 0x0820)
    open(caminho,'wb').write(bytes(cab) + im.convert('RGBA').tobytes('raw','BGRA'))

def ler(path):
    d=open(path,'rb').read()
    m=d[:4]
    if m in (b'JT35', b'JT33', b'JT31'):
        cc = {b'JT35':b'DXT5', b'JT33':b'DXT3', b'JT31':b'DXT1'}[m]
        w,h = struct.unpack_from('<II', d, 4)
        dxt = d[12:]
        hdr = bytearray(128)
        hdr[0:4]=b'DDS '; struct.pack_into('<I',hdr,4,124)
        struct.pack_into('<I',hdr,8,0x1|0x2|0x4|0x1000|0x80000)   # flags
        struct.pack_into('<I',hdr,12,h); struct.pack_into('<I',hdr,16,w)
        struct.pack_into('<I',hdr,20,max(1,w*h))                  # linearsize
        struct.pack_into('<I',hdr,76,32); struct.pack_into('<I',hdr,80,0x4)
        hdr[84:88]=cc
        struct.pack_into('<I',hdr,108,0x1000)
        return Image.open(io.BytesIO(bytes(hdr)+dxt)).convert('RGBA'), f"{w}x{h} {cc.decode()}"
    if m==b'JT20':
        w,h = struct.unpack_from('<HH', d, 16)
        if d[6] == 0x0A:                      # variante comprimida (RLE por pixel)
            return Image.frombytes('RGBA',(w,h),rle_descomprime(d),'raw','BGRA'), f"{w}x{h} ARGB-RLE"
        px = d[30:30+w*h*4]
        if len(px) < w*h*4: raise ValueError(f"dados curtos {len(px)} < {w*h*4}")
        return Image.frombytes('RGBA',(w,h),px,'raw','BGRA'), f"{w}x{h} ARGB"
    raise ValueError(f"magia desconhecida {m!r}")
