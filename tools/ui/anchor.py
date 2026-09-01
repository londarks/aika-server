"""Localiza campos de texto e o widget dono, sem precisar parsear o arquivo inteiro.

Um campo de texto tem 128 bytes: string latin1 + \0 + padding (0x00 ou 0xFE).
O cabecalho do registro termina onde o campo comeca, entao o dono esta em
  s - desloc,  com w[0]=tipo e w[1]=id.
Os pares (tipo, desloc) sao descobertos por votacao no proprio arquivo.
"""
import struct, collections, sys

def i32(d,o): return struct.unpack_from('<i', d, o)[0]

def is_text(d,o):
    if o < 0 or o+128 > len(d): return False
    f = d[o:o+128]
    z = f.find(b'\0')
    if z <= 0: return False                     # exige string nao vazia
    if any(c < 0x20 or c >= 0xFE for c in f[:z]): return False  # 0xFE e padding, nao texto
    return all(c in (0x00,0xFE) for c in f[z+1:])   # padding estrito

def campos(d):
    """offsets de campos de texto com conteudo"""
    # o byte anterior ao campo nunca e imprimivel: ou fim do cabecalho (binario)
    # ou padding do campo anterior (0x00/0xFE)
    return [o for o in range(1, len(d)-128)
            if 0x20 <= d[o] < 0xFE and d[o-1] in (0x00, 0xFE) and is_text(d,o)]

def votos(d, offs):
    v=collections.Counter()
    for s in offs:
        for desloc in range(24, 200, 4):
            o = s-desloc
            if o < 0: break
            t,i,p = struct.unpack_from('<3i', d, o)
            if 0 <= t <= 120 and 0 < i < 10**6 and 0 <= p < 10**6:
                v[(t,desloc)] += 1
    return v

if __name__=='__main__':
    for p in sys.argv[1:]:
        d=open(p,'rb').read(); offs=campos(d)
        v=votos(d,offs)
        print(f"\n== {p.split('/')[-1]}  {len(d)}B  {len(offs)} campos com texto")
        for (t,dl),c in v.most_common(14):
            print(f"   tipo {t:>4}  desloc {dl:>4}  ->  {c}")

# (tipo, deslocamento do inicio do registro ate o campo, indice do campo no registro)
DONOS = [(15,52,0), (4,48,0), (4,176,1), (16,60,0), (33,36,0)]

def dono(d, s):
    for t,dl,idx in DONOS:
        o = s-dl
        if o < 0: continue
        tt,i,p = struct.unpack_from('<3i', d, o)
        if tt==t and 0 < i < 0x40000000 and 0 <= p < 0x40000000:
            return {'rec':o, 'tipo':t, 'id':i, 'pai':p, 'campo':idx, 'off':s}
    return None

def extrair(d):
    achados=[]; orfaos=[]
    for s in campos(d):
        r = dono(d,s)
        (achados if r else orfaos).append(r if r else s)
    return achados, orfaos
