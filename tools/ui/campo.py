"""Acha onde um campo (this+desloc) e escrito."""
import struct, re, sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_32

# O caminho do cliente vem da linha de comando: nada de fora do repositorio
# fica escrito aqui dentro.
if len(sys.argv) < 2:
    sys.exit("uso: %s <AIKA.exe> <campo em hexa>" % sys.argv[0])
EXE = sys.argv[1]
d=open(EXE,'rb').read()
pe=struct.unpack_from('<I',d,0x3C)[0]
nsec=struct.unpack_from('<H',d,pe+6)[0]; optsz=struct.unpack_from('<H',d,pe+20)[0]
imgbase=struct.unpack_from('<I',d,pe+24+28)[0]
for i in range(nsec):
    o=pe+24+optsz+40*i
    if d[o:o+8].rstrip(b'\0')==b'.text':
        vsz,va,rsz,ro=struct.unpack_from('<IIII',d,o+8); break
blob=d[ro:ro+min(vsz,rsz)]
alvo=int(sys.argv[2],16)
pat=struct.pack('<I', alvo)
md=Cs(CS_ARCH_X86,CS_MODE_32)
for m in re.finditer(re.escape(pat), blob):
    p=m.start()
    ini=max(0,p-0x60)
    ult=None; achou=False; linhas=[]
    for ins in md.disasm(blob[ini:p+0x18], imgbase+va+ini):
        if ins.mnemonic=='push' and re.fullmatch(r'0x[0-9a-f]+', ins.op_str):
            ult=int(ins.op_str,0)
        marca=''
        if 'call' in ins.mnemonic and '0x54]' in ins.op_str and ult:
            marca=f"   <<< id={ult} (0x{ult:X})"
        linhas.append(f"    0x{ins.address:08X}  {ins.mnemonic:<7} {ins.op_str}{marca}")
        if hex(alvo) in ins.op_str and ins.mnemonic=='mov' and ins.op_str.startswith('dword ptr ['):
            achou=True
    if achou:
        print(f"=== escrita em +0x{alvo:X} perto de 0x{imgbase+va+p:08X} ===")
        print("\n".join(linhas[-10:])); print()
