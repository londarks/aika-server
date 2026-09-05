# Ferramentas de UI do cliente

Reversao do formato das cenas de UI (`UI/FieldScene*.bin`, `LoginScene*.bin`,
`SelCharScene*.bin`) e das texturas `.jit`. Nenhum arquivo do cliente vive aqui —
os caminhos sao passados na linha de comando.

## Formato da cena

Fluxo de registros, sem cabecalho de arquivo:

    registro = N palavras i32 + K campos de texto de 128 bytes
    w[0] = tipo (classe do widget)   w[1] = id   w[2] = id do pai
    w[4..7] = x, y, largura, altura (relativos ao pai)

N e K sao fixos por tipo. Os que carregam texto:

| tipo | cabecalho | campos de texto |
|------|-----------|-----------------|
| 4  (botao)  | 48 B | 2 |
| 15 (label)  | 52 B | 1 |
| 16 (edicao) | 60 B | 1 |
| 33          | 36 B | 1 |

Sozinhos, os tipos 4 e 15 cobrem ~99% dos campos com texto. Um campo e
`string latin1 + \0 + padding`, e o padding e `0x00` ou `0xFE` conforme o
arquivo. **0xFE nunca faz parte da string** — foi o que fez a primeira
deteccao falhar.

## `anchor.py`

Acha os campos de texto e o widget dono sem parsear o arquivo inteiro: como o
cabecalho termina onde o campo comeca, o dono esta em `campo - deslocamento`.
Evita ter que descobrir o tamanho de todo tipo de registro.

## `cena.py` — parse completo

Os registros com texto sao localizaveis sozinhos, e fatiam o arquivo em vaos
curtos; cada vao e resolvido por busca. Isso evita ter que descobrir o tamanho
de todos os ~15 tipos sem texto de uma vez. `parse()` devolve a lista de
registros e `build()` reserializa — o round-trip byte a byte e a validacao.

Duas armadilhas custaram tempo:

- ids nao cabem em 10^6; existem valores como `0x01000030`;
- um campo de texto vazio nao aparece na varredura, entao o registro dono cai
  no vao e o preenchedor precisa dar conta dele.

## `ids_exe.py` — o que o exe exige

O cliente busca widget por id numa chamada virtual do slot `0x54`:

    push <id> ; mov eax,[ecx] ; call dword ptr [eax+0x54]

Varre o `.text` atras desse padrao e devolve os ~1865 ids pedidos. **Um id que
o exe pede e a cena nao tem devolve nulo, e o codigo nao checa** — estoura em
`movsx eax, word ptr [eax+0x42]` (leitura de `+0x40`/`+0x42`, largura e altura
do widget). Foi exatamente o crash ao entrar no jogo na primeira tentativa.

## `merge.py` — transplante de layout

    python merge.py <cena_base> <cena_com_o_texto> <saida>

Pega a geometria do primeiro arquivo e o texto do segundo, casando por
`(id do widget, indice do campo)`, e **reanexa ao fim os registros cujo id
sumiu no layout novo** — sem isso o cliente estoura como acima. A ordem
original do arquivo de origem ja poe pai antes de filho.

Widget que so existe no layout novo fica com o texto original dele.

## `desmontar.py` — apoio

    python desmontar.py <endereco_de_runtime_em_hex>

Desmonta em volta de um endereco do relatorio do BugTrap (`errorlog.xml` dentro
do `BugReport_error_report_*.zip`), convertendo de endereco carregado para RVA.

## `jit.py` — texturas

`JT35` e DXT5 com cabecalho de 12 bytes (magia, largura, altura); `JT20` e ARGB
cru a partir de 0x1E. `ler(caminho)` devolve um `PIL.Image`.


## `geo.py` — o transplante que funciona

    python geo.py <cena_local> <cena_do_outro_cliente> <saida>

A cena **local** e a base. Nenhum registro entra ou sai, o arquivo nao muda de
tamanho, o texto nao e tocado. Casando por id, copia so `w[3..7]` (skin, x, y,
largura, altura), e so quando tipo, tamanho de cabecalho e pai batem.

### Por que nao da para ir na direcao contraria

A primeira tentativa foi usar a cena do outro cliente como base e reanexar no
fim os registros cujo id sumiu. O cliente continuou estourando no mesmo lugar
procurando o widget `0x7927` (31015) — que **estava no arquivo**, no trecho
anexado, perto do fim. Ou seja: o que passa do fim original nao e lido. O
`FieldSceneB4.bin` tinha ido de 575 KB para 618 KB, maior do que qualquer
versao. Nao fui atras de qual e o limite (buffer de leitura? contagem?), so
parei de crescer o arquivo.

### Quanto de layout realmente muda

Casando os dois clientes por id, entre 2382 e 3474 widgets batem por arquivo e
so **212 no total** tem geometria diferente. Os dois layouts sao praticamente o
mesmo. O que o cliente novo tem a mais sao janelas inteiras que so o exe dele
sabe preencher — janela de status estendida (`Crit damage`, resistencias a
`Stun`/`Silence`/`Shock`/`Snare`, `MP recovery`, pai 29664) e uma de presets de
skill (`Amnesia`, `First`..`Fourth`, `Remaining points`, `Apply skill`, pai
18203). Trazer esses registros nao adianta: o exe local nunca os popula.


## `menu_esc.py` — o conserto que era o pedido de verdade

    python menu_esc.py <FieldSceneB4.bin> <saida>

Neste build alguem colou o menu lateral dentro do painel do ESC. O painel
`31017` (em `FieldSceneB4.bin`, pai 8192) tinha 676x232: a grade
`Papel/Comunidade/Prans/Mapas/Negocios` ocupando x=17..550 e a coluna de
sistema espremida em x=568. No build CBM esse painel **nao existe** — o ESC de
la e so a coluna.

Os 42 filhos do painel: nenhum e procurado pelo exe, so o `31017`. Por isso da
para tirar a grade de cena sem risco de busca por id devolver nulo. O script
larga a grade em (20000, 20000) em vez de remover — o arquivo nao muda de
tamanho, que foi o que derrubou a tentativa de reanexar registros.

Resultado: painel 116x232, titulo mais 7 botoes em x=8, y=38..200 de 27 em 27 —
a mesma ordem do cliente original (Cancelar, Configuracoes, Mover Canal, Sala
Personagem, Sair da Conta, Configuracao UI, Sair do Jogo).


## `icones.py` — sprite de item

O indice do icone e um `u16` em **`+320`** do registro de 464 bytes do
`ItemList.bin` (a copia do servidor, que e texto claro). Os atlas sao
`UI/ItemIcons01.jit`..`11`, todos 1024x1024:

    atlas  = indice // 576 + 1
    celula = indice %  576
    x, y   = (celula % 24) * 42, (celula // 24) * 42

A celula tem **42x42**, nao 32. Medi pela energia de borda por coluna do
atlas — os picos caem de 42 em 42. Supor 32 rende recortes que parecem quase
certos (frame no lugar, item cortado) e mandam a investigacao para o lado
errado; so bate quando se filtra por nome e o escudo desenha um escudo.

`jit.py` agora abre `JT35`/`JT33`/`JT31` (DXT5/3/1) alem de `JT20` (ARGB cru).
### `JT20` com flag `0x0a` — RLE por pixel

Resolvido. Cabecalho de **22** bytes (nao 30 como o cru), depois:

    controle C >= 0x80  ->  repete (C - 0x7F) vezes o pixel de 4 bytes que segue
    controle C <  0x80  ->  seguem (C + 1) pixels literais

e um rodape de **8 bytes**. Validado pelo criterio que nao mente: nos tres
arquivos do cliente TK que usam o formato (`ItemIcons09`, `ItemIcons12`,
`win22`) sai exatamente `largura*altura` pixels deixando exatamente 8 bytes.
Os 640 bytes de `0xFF` que abrem o `ItemIcons12` sao 128 controles de 5 bytes
(controle + pixel branco), o que denunciou a estrutura.

`escrever_jt20()` grava de volta no formato cru (flag `0x02`) que o cliente BR
le, entao qualquer textura comprimida de la vira conversao, nao pesquisa.

Capacidade: 11 atlas x 576 = **6336** icones. 15 itens da tabela apontam acima
disso (Confetti, Themed Mask, Carnival Serpentine, Facion Eggs...) e ficam sem
sprite neste cliente.


## O que a quebra do JT20-RLE rendeu (e o que nao rendeu)

Com os tres arquivos comprimidos legiveis, deu para comparar os 11 atlas de
icone dos dois clientes celula a celula:

| atlas | resultado |
|---|---|
| 01-08, 10 | mesma arte (diferenca media 1,2 = so ruido de recompressao DXT) |
| 09 e 11 | arte **realmente diferente**, media 85 |
| 12 | so existe no TK, e as 15 celulas que a nossa tabela aponta estao **vazias la tambem** |

Conclusao pratica: **nao copiar 09 e 11**. Cada build atribui os proprios
indices de icone, entao a mesma celula guarda coisas diferentes — trazer o
atlas deles poe a figura errada nos nossos itens. E o `ItemIcons12` nao resolve
os 15 itens sem sprite (Confetti, Carnival Serpentine, Facion Eggs...): essa
arte veio de um cliente BR que nenhum dos dois tem.
