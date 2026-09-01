"""Devolve o menu do ESC ao formato original.

Neste build alguem colou o menu lateral (a grade Papel/Comunidade/Prans/Mapas/
Negocios) dentro do painel do ESC, que virou uma janela de 676x232 com a coluna
de sistema espremida na direita. O original e so a coluna.

Nenhum filho do painel e procurado pelo exe -- so o painel 31017 -- entao os
widgets da grade podem sair de cena sem risco de busca por id devolver nulo.
Nada e removido: eles vao para fora da tela. O arquivo nao muda de tamanho.
"""
import sys, cena

PAINEL = 31017
COLUNA = [31018, 31019, 31020, 31021, 31022, 31023, 31024, 31025]  # titulo + 7 botoes
FORA   = (20000, 20000)
LARG   = 116
MARGEM = 8

def arrumar(entrada, saida):
    d = open(entrada,'rb').read()
    r, _ = cena.parse(d)
    ix = {x['id']: x for x in r}
    if PAINEL not in ix: raise SystemExit(f"painel {PAINEL} nao encontrado em {entrada}")
    filhos = [x for x in r if x['pai'] == PAINEL]
    escondidos = movidos = 0
    for x in filhos:
        if x['id'] in COLUNA:
            x['w'][4] = 0 if x['id'] == 31018 else MARGEM   # titulo cola na borda
            if x['id'] == 31018: x['w'][6] = LARG
            movidos += 1
        else:
            x['w'][4], x['w'][5] = FORA
            escondidos += 1
    ix[PAINEL]['w'][6] = LARG
    saida_b = cena.build(r)
    assert len(saida_b) == len(d), "tamanho mudou"
    open(saida,'wb').write(saida_b)
    return movidos, escondidos, ix[PAINEL]['w'][6:8]

if __name__ == '__main__':
    m, e, wh = arrumar(sys.argv[1], sys.argv[2])
    print(f"  coluna reposicionada: {m} widgets | grade fora da tela: {e} widgets | painel agora {wh[0]}x{wh[1]}")
