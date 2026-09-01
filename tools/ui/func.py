import struct, sys, re
from capstone import Cs, CS_ARCH_X86, CS_MODE_32

# O caminho do cliente vem da linha de comando: nada de fora do repositorio
# fica escrito aqui dentro.
if len(sys.argv) < 2:
    sys.exit("uso: %s <AIKA.exe> <endereco em hexa> [quantas linhas]" % sys.argv[0])
EXE = sys.argv[1]
d=open(EXE,'rb').read()
pe=struct.unpack_from('<I',d,0x3C)[0]
nsec=struct.unpack_from('<H',d,pe+6)[0]; optsz=struct.unpack_from('<H',d,pe+20)[0]
imgbase=struct.unpack_from('<I',d,pe+24+28)[0]
secs=[]
for i in range(nsec):
    o=pe+24+optsz+40*i
    vsz,va,rsz,ro=struct.unpack_from('<IIII',d,o+8)
    secs.append((d[o:o+8].rstrip(b'\0').decode(),va,vsz,ro,rsz))
def off(rva):
    for n,va,vsz,ro,rsz in secs:
        if va<=rva<va+vsz: return ro+(rva-va)
md=Cs(CS_ARCH_X86,CS_MODE_32)
ini=int(sys.argv[2],16); n=int(sys.argv[3]) if len(sys.argv)>3 else 400
o=off(ini-imgbase)
ult_push=None
for ins in md.disasm(d[o:o+n*6], ini):
    s=f"  0x{ins.address:08X}  {ins.mnemonic:<7} {ins.op_str}"
    if ins.mnemonic=='push' and re.fullmatch(r'0x[0-9a-f]+', ins.op_str):
        ult_push=int(ins.op_str,0)
    if 'call' in ins.mnemonic and '0x54]' in ins.op_str and ult_push:
        s += f"      <<< busca widget id={ult_push} (0x{ult_push:X})"
    if '0x2b04' in ins.op_str: s += "   *** campo +0x2B04 ***"
    print(s)
    n-=1
    if n<=0: break
