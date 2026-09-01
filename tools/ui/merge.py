"""Transplanta o layout da UI de um cliente para outro preservando o texto local.

Base  = cena do cliente novo (geometria/estrutura).
Texto = cena atual em PT-BR (texto e widgets que o exe local exige).

Duas etapas:
 1. troca o conteudo dos campos de texto casando por (id do widget, indice do campo);
 2. reanexa os registros cujo id sumiu no layout novo -- sem eles o exe procura
    o widget por id, recebe nulo e estoura em `movsx eax, word ptr [eax+0x42]`.
"""
import sys, collections, cena

def merge(base_path, txt_path, saida):
    db = open(base_path,'rb').read()
    dt = open(txt_path,'rb').read()
    rb, _ = cena.parse(db)
    rt, _ = cena.parse(dt)

    # --- 1. texto ---
    ixt = collections.defaultdict(list)
    for r in rt:
        for i,_ in enumerate(r['tx']): ixt[(r['id'], i)].append(r['tx'][i])
    usados = collections.Counter()
    trocados = 0
    for r in rb:
        for i in range(len(r['tx'])):
            cand = ixt.get((r['id'], i))
            if not cand: continue
            k = min(usados[(r['id'],i)], len(cand)-1); usados[(r['id'],i)] += 1
            if cand[k] != r['tx'][i]:
                r['tx'][i] = cand[k]; trocados += 1

    # --- 2. widgets que sumiram (ordem original preserva pai antes de filho) ---
    ids_base = {r['id'] for r in rb}
    faltando = [r for r in rt if r['id'] not in ids_base]
    rb.extend(faltando)

    open(saida,'wb').write(cena.build(rb))
    return dict(base=len(rb)-len(faltando), texto=len(rt), trocados=trocados,
                reanexados=len(faltando),
                paineis={r['id'] for r in faltando if r['pai'] not in {x['id'] for x in faltando}})

if __name__=='__main__':
    b,t,s = sys.argv[1:4]
    r = merge(b,t,s)
    print(f"  registros base={r['base']} | campos de texto trocados={r['trocados']} | "
          f"widgets reanexados={r['reanexados']}")
