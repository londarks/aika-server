"""Troca o texto de widgets de uma cena, por id.

    python texto.py <cena.bin> <saida.bin> <id>=<texto> [<id>=<texto> ...]

Texto vazio apaga o rótulo. O campo tem 128 bytes fixos e o padding original é
preservado, então o arquivo não muda de tamanho — o que importa porque este
cliente é sensível a isso.

Serve para o caso mais comum de todos: renomear uma janela sem abrir editor.
"""

import sys
import cena


def nome_atual(campo: bytes) -> str:
    return campo.split(bytes(1))[0].decode("latin1")


def trocar(campo: bytes, texto: str) -> bytes:
    """Mantém os 128 bytes e o resto do padding que já estava lá."""
    b = texto.encode("latin1", "replace")[:126]
    return (b + b"\x00" + campo[len(b) + 1 :])[:128].ljust(128, b"\x00")


def main(entrada, saida, pares):
    d = open(entrada, "rb").read()
    regs, _ = cena.parse(d)
    porid = {}
    for r in regs:
        porid.setdefault(r["id"], r)

    for par in pares:
        alvo, _, novo = par.partition("=")
        wid = int(alvo)
        r = porid.get(wid)
        if r is None:
            sys.exit(f"id {wid} não existe em {entrada}")
        if not r["tx"]:
            sys.exit(f"id {wid} é tipo {r['tipo']}, que não tem campo de texto")
        antes = nome_atual(r["tx"][0])
        r["tx"][0] = trocar(r["tx"][0], novo)
        print(f"  {wid}: {antes!r} -> {novo!r}")

    saida_b = cena.build(regs)
    assert len(saida_b) == len(d), "o arquivo mudou de tamanho"
    open(saida, "wb").write(saida_b)
    print(f"{len(saida_b)} bytes, igual ao original")


if __name__ == "__main__":
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2], sys.argv[3:])
