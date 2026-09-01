"""Parse completo de uma cena de UI, usando os campos de texto como esqueleto.

Os registros com texto sao localizaveis sozinhos (anchor.py). Eles fatiam o
arquivo em vaos curtos; cada vao e preenchido por busca, o que mantem o espaco
de procura pequeno e a tabela tipo->tamanho compartilhada entre os vaos.
"""
import struct, anchor

TAM_TEXTO = {4:(48,2), 15:(52,1), 16:(60,1), 33:(36,1)}
HDRS = (48,52,44,56,60,36,40,64,68,72,76,80,84,88,92,96,100,32,28,24,104,108,112,116,120)

def i32(d,o): return struct.unpack_from('<i', d, o)[0]

def _ancoras(d):
    achados,_ = anchor.extrair(d)
    a={}
    for x in achados:
        h,k = TAM_TEXTO[x['tipo']]
        a[x['rec']] = (x['tipo'], h, k)   # dono() ja devolve o inicio do registro
    # descarta ancora que invade a anterior (falso positivo dentro de um registro)
    limpo={}; fim=0
    for ini in sorted(a):
        t,h,k = a[ini]
        if ini < fim: continue
        limpo[ini]=(t,h,k); fim = ini+h+128*k
    return limpo

def _preenche(d, ini, fim, tab):
    """particiona [ini,fim) em registros; devolve lista ou None"""
    pilha=[(ini, [])]
    vistos=set()
    while pilha:
        pos, acc = pilha.pop()
        if pos == fim: return acc
        if pos > fim or (pos,len(acc)) in vistos: continue
        vistos.add((pos,len(acc)))
        if pos+12 > len(d): continue
        t = i32(d,pos)
        if not (0 <= t <= 120 and 0 < i32(d,pos+4) < 0x40000000 and 0 <= i32(d,pos+8) < 0x40000000):
            continue
        opts = [tab[t]] if t in tab else [(h,0) for h in HDRS]
        if t in tab and pos+tab[t][0]+128*tab[t][1] > fim:
            opts = [(h,0) for h in HDRS]        # tamanho da tabela nao cabe: reabre
        for h,k in reversed(opts):
            if pos+h+128*k <= fim: pilha.append((pos+h+128*k, acc+[(pos,t,h,k)]))
    return None

def parse(d):
    anc=_ancoras(d)
    tab=dict(TAM_TEXTO)
    recs=[]; pos=0
    chaves=sorted(anc)
    for a in chaves + [len(d)]:
        if a > pos:
            fill=_preenche(d,pos,a,tab)
            if fill is None: raise ValueError(f"vao 0x{pos:X}..0x{a:X} nao fecha")
            for (o,t,h,k) in fill:
                recs.append((o,t,h,k))     # nao fixa tamanho vindo de palpite de vao
            pos=a
        if a < len(d):
            t,h,k = anc[a]; recs.append((a,t,h,k)); pos = a+h+128*k
    saida=[]
    for o,t,h,k in recs:
        w=list(struct.unpack_from(f'<{h//4}i', d, o))
        saida.append({'off':o,'tipo':t,'id':w[1],'pai':w[2],'w':w,
                      'tx':[d[o+h+128*j:o+h+128*(j+1)] for j in range(k)]})
    return saida, tab

def build(recs):
    out=bytearray()
    for r in recs:
        out += struct.pack(f'<{len(r["w"])}i', *r['w'])
        for t in r['tx']: out += t
    return bytes(out)

if __name__=='__main__':
    import sys, collections
    for p in sys.argv[1:]:
        d=open(p,'rb').read()
        try:
            r,tab=parse(d)
            ok = build(r)==d
            ids=[x['id'] for x in r]
            print(f"{p.split('/')[-1]:20s} {len(d):>7}B {len(r):>5} regs  ids={len(set(ids))}  "
                  f"round-trip={'OK' if ok else 'FALHOU'}  tipos={len(tab)}")
        except Exception as e:
            print(f"{p.split('/')[-1]:20s} ERRO: {e}")
