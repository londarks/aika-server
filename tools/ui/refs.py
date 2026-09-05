"""Quem no exe usa uma string, e o que faz em volta dela.

    python refs.py <AIKA.exe> "UI\\PranHair.bin"
    python refs.py <AIKA.exe> --hex 0031A077        (offset de arquivo)

O `desmontar.py` precisa de um endereco que o BugTrap ja tenha dado. Este
nao: acha a string sozinho, converte para endereco virtual, varre as secoes
de codigo atras da constante de 4 bytes que a empurra na pilha, e desmonta
em volta de cada uso.

Nenhum caminho de cliente vive aqui: o exe vem na linha de comando.
"""
import struct
import sys

from capstone import CS_ARCH_X86, CS_MODE_32, Cs

ANTES, DEPOIS = 0x60, 0x60


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


def para_va(secs, imgbase, offset):
    for _, va, vsz, ro, rsz in secs:
        if ro <= offset < ro + rsz:
            return imgbase + va + (offset - ro)
    return None


def para_offset(secs, imgbase, va):
    rva = va - imgbase
    for _, sva, vsz, ro, _ in secs:
        if sva <= rva < sva + vsz:
            return ro + (rva - sva)
    return None


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    d = open(sys.argv[1], 'rb').read()
    imgbase, secs = secoes(d)

    if sys.argv[2] == '--hex':
        alvos = [int(sys.argv[3], 16)]
    else:
        agulha = sys.argv[2].encode('latin1')
        alvos = []
        i = d.find(agulha)
        while i != -1:
            alvos.append(i)
            i = d.find(agulha, i + 1)
        if not alvos:
            sys.exit('essa string nao esta no exe')

    md = Cs(CS_ARCH_X86, CS_MODE_32)
    for offset in alvos:
        va = para_va(secs, imgbase, offset)
        fim = d.find(b'\0', offset)
        print(f"\n===== 0x{offset:06X}  VA 0x{va:08X}  {d[offset:fim].decode('latin1')!r}")
        if va is None:
            print('  (fora de qualquer secao mapeada)')
            continue

        # Quem empurra esse endereco. Em 32 bits e uma constante de 4 bytes
        # dentro de um `push` ou de um `mov`, entao basta procurar os bytes.
        agulha = struct.pack('<I', va)
        usos = []
        for nome, sva, vsz, ro, rsz in secs:
            corpo = d[ro:ro + rsz]
            i = corpo.find(agulha)
            while i != -1:
                usos.append((nome, ro + i, imgbase + sva + i))
                i = corpo.find(agulha, i + 1)
        if not usos:
            print('  ninguem referencia (talvez montado em runtime)')
        for nome, uso_off, uso_va in usos:
            print(f"\n  --- usado em {nome} 0x{uso_va:08X} (arquivo 0x{uso_off:06X})")
            inicio = uso_off - ANTES
            for ins in md.disasm(d[inicio:uso_off + DEPOIS], uso_va - ANTES):
                marca = '   <<<<' if uso_va - 4 <= ins.address <= uso_va else ''
                print(f"    0x{ins.address:08X}  {ins.mnemonic:<7} {ins.op_str}{marca}")


if __name__ == '__main__':
    main()
