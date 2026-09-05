"""Troca palavras de um registro de cena, no lugar, sem reserializar.

    python remendo.py <entrada.bin> <saida.bin> <id>.<palavra>=<valor> ...
    python remendo.py cena.bin nova.bin 8198.cor=0x55FFFFFF 8198.y=140

Por que no lugar: o parser sequencial (`cena.py`) escorrega em algumas regiões,
e reserializar um arquivo mal lido grava lixo. Aqui nada é reconstruído — o
arquivo é copiado byte a byte e só as palavras pedidas mudam. O tamanho é o
mesmo por construção, e regiões que o parser leria errado ficam intocadas.

Os nomes das palavras, na ordem em que aparecem no registro:

    tipo(0) id(1) pai(2) estilo(3) x(4) y(5) larg(6) alt(7) cor(8) ?(9) texto(10)

`cor` e `texto` são ARGB: `0xAAFFFFFF` é branco a 67% de alfa. Descoberto pela
aba "Grupos(Y)", que tem `texto=0xFF88FFBB` e é a única esverdeada na tela.
"""

import struct
import sys

PALAVRA = {"tipo": 0, "id": 1, "pai": 2, "estilo": 3, "x": 4, "y": 5,
           "larg": 6, "alt": 7, "cor": 8, "cor2": 9, "texto": 10}


def procurar(d, wid):
    """Offset do registro cujo id é `wid`, achado por varredura de bytes.

    Não usa o parser sequencial de propósito: aqui não importa onde cada
    registro começa, só que a segunda palavra seja o id e a primeira um tipo
    plausível.
    """
    alvo = struct.pack("<i", wid)
    for o in range(4, len(d) - 48, 4):
        if d[o:o + 4] != alvo:
            continue
        t, i, p = struct.unpack_from("<3i", d, o - 4)
        if 0 <= t <= 120 and i == wid and 0 <= p < 10 ** 6:
            return o - 4
    return None


def main(entrada, saida, pedidos):
    d = bytearray(open(entrada, "rb").read())
    for pedido in pedidos:
        alvo, _, valor = pedido.partition("=")
        sid, _, campo = alvo.partition(".")
        if campo not in PALAVRA:
            sys.exit(f"palavra '{campo}' não existe. Use: {', '.join(PALAVRA)}")
        wid = int(sid)
        off = procurar(d, wid)
        if off is None:
            sys.exit(f"id {wid} não achado em {entrada}")
        pos = off + 4 * PALAVRA[campo]
        antes = struct.unpack_from("<i", d, pos)[0]
        novo = int(valor, 0)
        if novo > 0x7FFFFFFF:
            novo -= 1 << 32
        struct.pack_into("<i", d, pos, novo)
        print(f"  {wid}.{campo}: {antes & 0xFFFFFFFF:#010x} -> {novo & 0xFFFFFFFF:#010x}"
              f"   (registro em 0x{off:06X})")
    assert len(d) == len(open(entrada, "rb").read()), "tamanho mudou"
    open(saida, "wb").write(bytes(d))
    print(f"{len(d)} bytes, igual ao original")


if __name__ == "__main__":
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2], sys.argv[3:])
