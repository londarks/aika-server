# Visualizador de itens

Janela para navegar os itens do jogo com a sprite ao lado do nome. Serve para
decidir o que entra em drop, loja e NPC sem ter que adivinhar pelo id.

Tauri 2 com Rust; a interface é um `index.html` estático, sem npm e sem build
de front.

## Rodar

    cd src-tauri
    cargo tauri dev          # janela, recompila ao salvar
    cargo build --release    # binário sozinho, com o front embutido

Antes disso, copie `config.exemplo.toml` para `config.toml` e confira os dois
caminhos. Eles são relativos ao próprio arquivo:

```toml
item_list = "../../assets/items/ItemList.bin"   # tabela do servidor
ui        = "../../../aika-client/UI"           # pasta UI do cliente
```

O `config.toml` é ignorado pelo git porque aponta para dado de cliente, que não
mora no repositório.

Para conferir o backend sem abrir a janela:

    cargo run -- --verificar

Ele imprime quantos itens leu, faz quatro buscas e diz se algum atlas falhou.
Se a janela abrir em branco o problema é o front; se isso aqui falhar, é dado.

    cargo run --bin jit2png -- <entrada.jit> <saida.png>

Converte uma textura do cliente em PNG, com o mesmo decodificador que a janela
usa — se os dois divergissem a conferência não valeria nada.

## Levar os ícones para fora (site, wiki, planilha)

    cargo run --release --bin exportar -- <ItemList.bin> <pasta UI> <saída>

Grava `atlas01.png`..`atlas11.png` e um `itens.json`:

```json
{ "celula": 42,
  "itens": [ {"id":1280,"nome":"Escudo Lapis","nome_en":"Lapis Shield",
              "icone":241,"atlas":1,"x":42,"y":420} ] }
```

É o formato que um site quer: o navegador baixa no máximo onze imagens e o
resto é `background-position`. Um PNG por item seriam dezesseis mil
requisições. No CSS:

```css
.icone { width:42px; height:42px;
         background-image:url("/atlas01.png");
         background-position:-42px -420px; }
```

Saída de hoje: 16.714 itens, 5.158 ícones distintos, 11 atlas, **24 MB**. Para
web vale converter os PNG em WebP — a arte é 1024x1024 com alfa e encolhe
bastante. `--soltos` grava um PNG de 42x42 por item, para quando são poucos
escolhidos a dedo.

## De onde vem cada coisa

**Os nomes e o índice do ícone** saem da cópia do servidor,
`assets/items/ItemList.bin`, que é texto claro: registros fixos de 464 bytes
indexados pelo id, sem cabeçalho. A cópia do cliente (`ItemList4.bin`) é a
mesma tabela cifrada com um keystream por posição, e não serve aqui. O layout
completo dos campos está em `crates/aika-data/src/itemlist.rs`.

**A sprite** sai de `UI/ItemIcons01.jit` em diante. O índice é um `u16` em
`+320` do registro:

```
atlas  = índice / 576 + 1
célula = índice % 576
x, y   = (célula % 24) * 42, (célula / 24) * 42
```

A célula tem **42x42**, não 32: 24 * 42 = 1008, sobrando 16 pixels de borda.
Com 32 os recortes ficam quase certos — o frame aparece, o item fica cortado —
e é fácil concluir que o campo do índice está errado quando não está.

## Formatos de textura

`jit.rs` lê as quatro variantes que os clientes usam:

| magia  | conteúdo                     | dados começam em |
|--------|------------------------------|------------------|
| `JT31` | DXT1                         | 12 |
| `JT33` | DXT3                         | 12 |
| `JT35` | DXT5                         | 12 |
| `JT20` | BGRA cru, flag `0x02` em +6  | 30 |
| `JT20` | BGRA em RLE, flag `0x0A`     | 22 |

O RLE é por pixel de 4 bytes, com rodapé de 8 que não se lê:

```
controle >= 0x80  ->  repete (C - 0x7F) vezes o pixel seguinte
controle <  0x80  ->  seguem (C + 1) pixels literais
```

O decodificador foi conferido contra o do Pillow: `JT20` cru e RLE saem byte a
byte iguais; DXT3 e DXT5 diferem no máximo 1 por canal, que é o arredondamento
do 565 para 888, com o alfa idêntico.

## Limites conhecidos

Este cliente tem 11 atlas, ou **6336** ícones. Quinze itens da tabela apontam
acima disso — Confetti, Carnival Serpentine, Facion Eggs e outros de evento —
e aparecem com o quadriculado de "sem sprite". O cliente TK tem um
`ItemIcons12.jit`, mas as células que faltam estão **vazias lá também**: essa
arte veio de um cliente brasileiro que nenhum dos dois tem.

Não copiar `ItemIcons09` nem `ItemIcons11` do cliente TK. Cada build atribui os
próprios índices, então a mesma célula guarda coisas diferentes e os itens
ficariam com a figura errada. Os outros nove atlas são a mesma arte.
