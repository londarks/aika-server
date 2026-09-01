"""Aplica a geometria do layout de outro cliente sobre a cena local.

Direcao segura: a cena local e a base. Nenhum registro entra ou sai, o tamanho
do arquivo nao muda, o texto nao e tocado. Casando por id, copia so
`w[3..7]` = skin, x, y, largura, altura -- e so quando tipo, tamanho de
cabecalho e pai batem, para nao mudar o significado das palavras.
"""
import sys, collections, cena

def aplicar(local_path, outro_path, saida):
    dl = open(local_path,'rb').read()
    rl, _ = cena.parse(dl)
    ro, _ = cena.parse(open(outro_path,'rb').read())
    ix = collections.defaultdict(list)
    for x in ro: ix[x['id']].append(x)
    usados = collections.Counter(); mudou = 0; pulou = 0
    for r in rl:
        c = ix.get(r['id'])
        if not c: continue
        k = min(usados[r['id']], len(c)-1); usados[r['id']] += 1
        y = c[k]
        if y['tipo'] != r['tipo'] or len(y['w']) != len(r['w']) or y['pai'] != r['pai']:
            pulou += 1; continue
        if r['w'][3:8] != y['w'][3:8]:
            r['w'][3:8] = y['w'][3:8]; mudou += 1
    saida_b = cena.build(rl)
    assert len(saida_b) == len(dl), "tamanho mudou"
    open(saida,'wb').write(saida_b)
    return mudou, pulou, len(rl)

if __name__=='__main__':
    a,b,s = sys.argv[1:4]
    m,p,n = aplicar(a,b,s)
    print(f"  {n} registros | geometria atualizada em {m} | ignorados por tipo/pai diferente {p}")
