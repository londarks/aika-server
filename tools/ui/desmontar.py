import struct, sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_32

# O caminho do cliente vem da linha de comando: nada de fora do repositorio
# fica escrito aqui dentro.
if len(sys.argv) < 2:
    sys.exit("uso: %s <AIKA.exe> <endereco em hexa> [mais enderecos]" % sys.argv[0])
EXE = sys.argv[1]
d=open(EXE,'rb').read()
pe=struct.unpack_from('<I',d,0x3C)[0]
nsec=struct.unpack_from('<H',d,pe+6)[0]; optsz=struct.unpack_from('<H',d,pe+20)[0]
imgbase=struct.unpack_from('<I',d,pe+24+28)[0]
secs=[]
for i in range(nsec):
    o=pe+24+optsz+40*i
    name=d[o:o+8].rstrip(b'\0').decode()
    vsz,va,rsz,ro=struct.unpack_from('<IIII',d,o+8)
    secs.append((name,va,vsz,ro,rsz))
def off(rva):
    for n,va,vsz,ro,rsz in secs:
        if va <= rva < va+vsz: return ro + (rva-va)
    return None
md=Cs(CS_ARCH_X86, CS_MODE_32)
BASE_RUN = 0x00490000
for arg in sys.argv[2:]:
    run = int(arg,16)
    rva = run - BASE_RUN
    start = rva - 0x40
    o=off(start)
    print(f"\n===== {arg}  (RVA 0x{rva:X}) =====")
    for ins in md.disasm(d[o:o+0xA0], imgbase+start):
        mark = "  <<<< CRASH" if ins.address == imgbase+rva else ""
        print(f"  0x{ins.address:08X}  {ins.mnemonic:<8} {ins.op_str}{mark}")
