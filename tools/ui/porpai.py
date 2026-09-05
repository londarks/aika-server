"""Acha registros de cena pelo id do pai, varrendo bytes.

O parser sequencial (`cena.py`) escorrega 4 bytes em algumas regiões e passa a
ler `tipo=0 id=1` — e o round-trip byte a byte **não** pega isso, porque só
reserializa o que leu. Enquanto aquilo não estiver consertado, esta varredura é
a fonte confiável: ela não depende de alinhamento nenhum.

A ideia: num registro, o pai é a terceira palavra. Então procurar o id do pai
em toda posição alinhada e conferir se as duas palavras anteriores parecem tipo
e id acha os filhos sem precisar saber onde cada registro começa.

    python porpai.py <cena.bin> <id do pai> [<id do pai> ...]
"""

import struct
import sys

TIPO = {1: "painel", 3: "painel", 4: "botao", 15: "rotulo", 16: "edicao",
        19: "lista", 8: "combo", 33: "?33"}


def i32(d, o):
    return struct.unpack_from("<i", d, o)[0]


def registro_em(d, o):
    """Lê um registro que começa em `o`, se ele for plausível."""
    if o < 0 or o + 48 > len(d):
        return None
    t, i, p = struct.unpack_from("<3i", d, o)
    if not (0 <= t <= 120 and 0 < i < 10 ** 6 and 0 <= p < 10 ** 6):
        return None
    return {"off": o, "tipo": t, "id": i, "pai": p,
            "w": list(struct.unpack_from("<12i", d, o))}


def filhos(d, pai):
    """Todo registro cuja terceira palavra é `pai`."""
    alvo = struct.pack("<i", pai)
    achados = []
    for o in range(8, len(d) - 4, 4):
        if d[o:o + 4] != alvo:
            continue
        r = registro_em(d, o - 8)
        if r and r["pai"] == pai:
            achados.append(r)
    return achados


def procurar(d, wid):
    """O registro cujo id é `wid`, se existir."""
    alvo = struct.pack("<i", wid)
    for o in range(4, len(d) - 4, 4):
        if d[o:o + 4] != alvo:
            continue
        r = registro_em(d, o - 4)
        if r and r["id"] == wid:
            return r
    return None


def cor(v):
    return f"0x{v & 0xFFFFFFFF:08X}"


def main(caminho, pais):
    d = open(caminho, "rb").read()
    for pai in pais:
        p = procurar(d, pai)
        cab = f"pai {pai}"
        if p:
            cab += (f"  (tipo {p['tipo']}, dentro de {p['pai']}, "
                    f"{p['w'][4]},{p['w'][5]} {p['w'][6]}x{p['w'][7]}, "
                    f"estilo {p['w'][3]}, cor {cor(p['w'][8])})")
        else:
            cab += "  (não existe como registro; o exe cria)"
        print(f"\n=== {cab} ===")
        for r in sorted(filhos(d, pai), key=lambda r: (r["w"][5], r["w"][4])):
            w = r["w"]
            print(f"  off 0x{r['off']:06X}  id={r['id']:<7} "
                  f"{TIPO.get(r['tipo'], r['tipo']):<7} "
                  f"{w[4]:>5},{w[5]:<5} {w[6]:>4}x{w[7]:<4} "
                  f"estilo={w[3]:<5} cores={cor(w[8])} {cor(w[10])}")


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main(sys.argv[1], [int(a) for a in sys.argv[2:]])
