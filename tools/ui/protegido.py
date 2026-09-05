"""Diz se um AIKA*.exe esta protegido (WinLicense) ou aberto.

    python protegido.py <caminho do exe>

Sem rodar nada: le a entropia das secoes e o nome delas. Codigo aberto fica
perto de 6.5; packer estoura para 8.0 e cria secoes como .winlice / .boot.
"""
import struct, math, collections, sys

def ent(b):
    if not b: return 0.0
    c=collections.Counter(b); n=len(b)
    return -sum((v/n)*math.log2(v/n) for v in c.values())

def olhar(p):
    d=open(p,'rb').read()
    pe=struct.unpack_from('<I',d,0x3C)[0]
    nsec=struct.unpack_from('<H',d,pe+6)[0]; optsz=struct.unpack_from('<H',d,pe+20)[0]
    epr=struct.unpack_from('<I',d,pe+24+16)[0]
    print(f"\n{p}  ({len(d):,} bytes)")
    suspeito=[]; ep_secao=None; text_ent=None
    for i in range(nsec):
        o=pe+24+optsz+40*i
        nome=d[o:o+8].rstrip(b'\0').decode('latin1','replace')
        vsz,va,rsz,ro=struct.unpack_from('<IIII',d,o+8)
        e=ent(d[ro:ro+rsz])
        if va<=epr<va+vsz: ep_secao=nome or "(sem nome)"
        if nome=='.text': text_ent=e
        flag = ""
        if nome.lower() in ('.winlice','.boot','.vm_sec','.themida','.vmp0','.vmp1','.enigma'):
            suspeito.append(nome); flag="  <- marca de packer"
        elif e>=7.5 and rsz>10000:
            flag="  <- entropia alta (cifrado?)"
        print(f"   {nome:10s} raw={rsz:9d}  entropia={e:.2f}{flag}")
    print(f"\n   entry point cai em: {ep_secao}")
    veredicto = "ABERTO — da para desmontar e remendar" if (
        text_ent and text_ent<7.0 and not suspeito and ep_secao=='.text'
    ) else "PROTEGIDO — packer no caminho"
    print(f"   >>> {veredicto}")
    if suspeito: print(f"       secoes de packer: {suspeito}")

if __name__=='__main__':
    if len(sys.argv)<2:
        sys.exit(__doc__)
    for p in sys.argv[1:]: olhar(p)
