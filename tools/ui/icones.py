"""id de item -> sprite.

O indice do icone e um u16 em `+320` no registro de 464 bytes do ItemList.
Os atlas sao `UI/ItemIcons01.jit`..`ItemIcons11.jit`, todos 1024x1024.

    atlas  = indice // 576 + 1
    celula = indice %  576
    x, y   = (celula % 24) * 42, (celula // 24) * 42

A celula tem **42x42**, nao 32 -- 24 cabem por linha (24*42 = 1008, sobram 16
pixels na borda). Medi isso pela energia de borda por coluna do atlas: os picos
caem de 42 em 42, come�ando em x=1. Supor 32 gera recortes que parecem quase
certos e nao sao, o que custa tempo.

Capacidade: 11 atlas x 576 = 6336 icones. Indices acima disso vivem num 12o
atlas que este cliente nao tem.
"""
import struct, os
import jit

CEL, POR_LINHA = 42, 24
POR_ATLAS = POR_LINHA * POR_LINHA          # 576
REG_ITEM, OFF_ICONE = 464, 320

def indice(tabela: bytes, item_id: int) -> int:
    return struct.unpack_from('<H', tabela, item_id * REG_ITEM + OFF_ICONE)[0]

def recorte(dir_ui, idx):
    """PIL.Image 42x42 do icone, ou None se o atlas nao existe aqui."""
    atlas, cel = idx // POR_ATLAS + 1, idx % POR_ATLAS
    p = os.path.join(dir_ui, f"ItemIcons{atlas:02d}.jit")
    if not os.path.exists(p): return None
    im, _ = jit.ler(p)
    x, y = (cel % POR_LINHA) * CEL, (cel // POR_LINHA) * CEL
    return im.crop((x, y, x + CEL, y + CEL))

def nome(tabela, item_id, ingles=True):
    o = item_id * REG_ITEM + (64 if ingles else 0)
    s = tabela[o:o+64].split(b'\x00')[0]
    return s.decode('latin1') if s else None
