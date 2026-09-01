"""Ids de widget que o exe procura em runtime.

Chamada virtual do slot 0x54: `push <id> ... call dword ptr [reg+0x54]`
(ou `mov eax,[reg+0x54]; call eax`). O id vem como imediato logo antes.
"""
import struct, re, json, collections

import sys

# O caminho do cliente vem da linha de comando: nada de fora do repositorio
# fica escrito aqui dentro.
if len(sys.argv) < 2:
    sys.exit("uso: %s <AIKA.exe>" % sys.argv[0])
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

# locais da chamada
sites=[]
for m in re.finditer(rb'\xFF[\x50\x51\x52\x53\x56\x57]\x54', blob): sites.append(m.start())
for m in re.finditer(rb'\x8B[\x40\x41\x42\x43\x46\x47]\x54\xFF\xD0', blob): sites.append(m.start())

ids=collections.Counter()
for s in sites:
    for back in range(5, 40):
        o=s-back
        if o < 0: break
        if blob[o]==0x68:
            v=struct.unpack_from('<I', blob, o+1)[0]
            if 0 < v < 0x10000:
                ids[v]+=1
            break
print(f"locais de chamada [reg+0x54]: {len(sites)}   ids distintos: {len(ids)}")
print(f"faixa: {min(ids)} (0x{min(ids):X}) .. {max(ids)} (0x{max(ids):X})")
json.dump(sorted(ids), open('ids_exe.json','w'))
