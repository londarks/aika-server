"""Mapa dos pacotes que o cliente trata, tirado do proprio exe.

    python pacotes.py <AIKA.exe>              tudo
    python pacotes.py <AIKA.exe> 907          so esse opcode
    python pacotes.py <AIKA.exe> --cadeias    agrupado por cadeia de despacho

Serve para parar de adivinhar o que o cliente faz com um pacote. O despacho
dele nao e uma tabela de saltos: e uma cadeia de `cmp edi, <opcode>` seguida
de `jne` para a proxima e de um `call` para quem trata. Este script varre a
`.text` inteira atras desse padrao e imprime opcode -> tratador.

O exe vem na linha de comando; nenhum caminho de cliente vive aqui.
"""
import struct
import sys
from collections import defaultdict

from capstone import CS_ARCH_X86, CS_MODE_32, Cs

# A faixa em que os opcodes do jogo vivem. Abaixo disso sao tamanhos de pilha
# e constantes soltas; acima, nada que o protocolo use.
MENOR, MAIOR = 0x100, 0xFFF0
# Quantas instrucoes depois do `cmp` ainda contam como "o corpo desse ramo".
ALCANCE = 8


def secoes(d):
    pe = struct.unpack_from('<I', d, 0x3C)[0]
    nsec = struct.unpack_from('<H', d, pe + 6)[0]
    optsz = struct.unpack_from('<H', d, pe + 20)[0]
    imgbase = struct.unpack_from('<I', d, pe + 24 + 28)[0]
    out = []
    for i in range(nsec):
        o = pe + 24 + optsz + 40 * i
        nome = d[o:o + 8].rstrip(b'\0').decode('latin1')
        vsz, va, rsz, ro = struct.unpack_from('<IIII', d, o + 8)
        out.append((nome, va, vsz, ro, rsz))
    return imgbase, out


def varrer(d, imgbase, secs):
    """Todo `cmp reg, opcode` com o `call` que vem logo depois."""
    texto = [s for s in secs if s[0] == '.text'][0]
    _, va, _, ro, rsz = texto
    md = Cs(CS_ARCH_X86, CS_MODE_32)
    md.skipdata = True

    achados = []
    janela = []
    for ins in md.disasm(d[ro:ro + rsz], imgbase + va):
        janela.append(ins)
        if len(janela) > ALCANCE + 1:
            janela.pop(0)

        # O `cmp` fica no inicio da janela; procuramos para frente.
        pivo = janela[0]
        if pivo.mnemonic != 'cmp' or ', 0x' not in pivo.op_str:
            continue
        alvo, _, imm = pivo.op_str.rpartition(', ')
        try:
            imm = int(imm, 16)
        except ValueError:
            continue
        if not (MENOR <= imm <= MAIOR) or alvo.startswith(('byte', 'word')):
            continue
        # Um `cmp` de opcode e sempre contra um registrador, nunca contra
        # memoria: o valor ja foi lido do cabecalho para um registrador.
        if '[' in alvo:
            continue

        for seguinte in janela[1:]:
            if seguinte.mnemonic == 'call' and seguinte.op_str.startswith('0x'):
                achados.append((pivo.address, imm, alvo, int(seguinte.op_str, 16)))
                break
    return achados


def switches(d, imgbase, secs, off):
    """Os despachos em forma de `switch`, que e como o cliente trata o jogo.

    O padrao que o compilador gera:

        cmp eax, <teto>            ; acima disso, outro ramo
        sub eax, <base>
        cmp eax, <quantos>
        ja  <default>
        movzx eax, byte ptr [eax + <indices>]   ; um byte por opcode
        jmp dword ptr [eax*4 + <saltos>]

    Duas tabelas: a de indices tem um byte por opcode e a de saltos os
    enderecos. Opcodes que o cliente ignora apontam todos para o mesmo ramo,
    que e o default -- da para reconhece-lo por ser o mais repetido.
    """
    import re
    texto = [x for x in secs if x[0] == '.text'][0]
    _, va, _, ro, rsz = texto
    md = Cs(CS_ARCH_X86, CS_MODE_32)
    md.skipdata = True
    idx = re.compile(r'^e\w\w, byte ptr \[e\w\w \+ (0x[0-9a-f]+)\]$')
    salto = re.compile(r'^dword ptr \[e\w\w\*4 \+ (0x[0-9a-f]+)\]$')

    achados, janela = [], []
    for ins in md.disasm(d[ro:ro + rsz], imgbase + va):
        janela.append(ins)
        if len(janela) > 6:
            janela.pop(0)
        if ins.mnemonic != 'jmp':
            continue
        m = salto.match(ins.op_str)
        if not m:
            continue
        tab = int(m.group(1), 16)
        indices = base = quantos = None
        for a in janela:
            if a.mnemonic == 'movzx':
                mm = idx.match(a.op_str)
                if mm:
                    indices = int(mm.group(1), 16)
            elif a.mnemonic == 'sub' and ', 0x' in a.op_str:
                try:
                    base = int(a.op_str.rsplit(', ', 1)[1], 16)
                except ValueError:
                    pass
            elif a.mnemonic == 'cmp' and ', 0x' in a.op_str:
                try:
                    quantos = int(a.op_str.rsplit(', ', 1)[1], 16)
                except ValueError:
                    pass
        if indices and base and quantos and MENOR <= base <= MAIOR:
            achados.append((ins.address, base, quantos, indices, tab))
    return achados


def ler_switch(d, off, md, base, quantos, indices, tab):
    """Opcode -> tratador, para um despacho ja localizado."""
    oi, ot = off(indices), off(tab)
    if oi is None or ot is None:
        return {}
    fora = {}
    for k in range(quantos + 1):
        i = d[oi + k]
        alvo = struct.unpack_from('<I', d, ot + i * 4)[0]
        o = off(alvo)
        if o is None:
            continue
        trata = None
        for ins in md.disasm(d[o:o + 0x20], alvo):
            if ins.mnemonic == 'call' and ins.op_str.startswith('0x'):
                trata = int(ins.op_str, 16)
                break
            if ins.mnemonic == 'jmp':
                break
        fora[base + k] = (alvo, trata)
    return fora


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    d = open(sys.argv[1], 'rb').read()
    imgbase, secs = secoes(d)
    achados = varrer(d, imgbase, secs)

    if '--switch' in sys.argv:
        md = Cs(CS_ARCH_X86, CS_MODE_32)

        def off(va):
            r = va - imgbase
            for _, sva, vsz, ro, _ in secs:
                if sva <= r < sva + vsz:
                    return ro + (r - sva)
            return None

        for end, base, quantos, indices, tab in switches(d, imgbase, secs, off):
            mapa = ler_switch(d, off, md, base, quantos, indices, tab)
            if len(mapa) < 8:
                continue
            from collections import Counter
            padrao = Counter(v[0] for v in mapa.values()).most_common(1)[0][0]
            reais = {k: v for k, v in mapa.items() if v[0] != padrao and v[1]}
            print(f"\n=== switch em 0x{end:08X}: "
                  f"opcodes 0x{base:03X}..0x{base + quantos:03X}")
            print(f"    indices 0x{indices:08X}  saltos 0x{tab:08X}  "
                  f"default 0x{padrao:08X}  ({len(reais)} tratados)")
            for op in sorted(reais):
                print(f"      0x{op:03X} -> 0x{reais[op][1]:08X}")
        return

    filtro = None
    cadeias = '--cadeias' in sys.argv
    for a in sys.argv[2:]:
        if not a.startswith('--'):
            filtro = int(a, 16)

    if cadeias:
        # Comparacoes vizinhas pertencem ao mesmo despacho. O corte de 0x80
        # separa uma cadeia da seguinte sem juntar as duas.
        grupos, atual = [], []
        for item in sorted(achados):
            if atual and item[0] - atual[-1][0] > 0x80:
                grupos.append(atual)
                atual = []
            atual.append(item)
        if atual:
            grupos.append(atual)
        for g in sorted(grupos, key=len, reverse=True):
            if len(g) < 3:
                continue
            print(f"\n=== cadeia em 0x{g[0][0]:08X}  ({len(g)} opcodes)")
            for end, op, reg, alvo in g:
                print(f"    0x{op:04X} -> 0x{alvo:08X}      ({reg}, em 0x{end:08X})")
        return

    por_opcode = defaultdict(list)
    for end, op, reg, alvo in achados:
        por_opcode[op].append((end, alvo))
    print(f"{len(achados)} comparacoes de opcode, {len(por_opcode)} opcodes distintos\n")
    for op in sorted(por_opcode):
        if filtro is not None and op != filtro:
            continue
        for end, alvo in por_opcode[op]:
            print(f"  0x{op:04X} -> 0x{alvo:08X}   (cmp em 0x{end:08X})")


if __name__ == '__main__':
    main()
