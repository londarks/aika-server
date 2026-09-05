# Ferramentas

Nada aqui é o servidor. É a bancada: ler e reescrever o dado do cliente, e
olhar o que está lá dentro.

Nenhuma ferramenta guarda caminho de cliente. Ou recebe o caminho na linha de
comando, ou lê de um `config.toml` ignorado pelo git.

| pasta | o que faz |
|---|---|
| [`ui/`](ui/) | formato das cenas de UI, texturas `.jit`, ids que o exe exige, sprite de item |
| [`visualizador/`](visualizador/) | janela para navegar os itens com a sprite ao lado, e `exportar` para levar os ícones a um site |
| [`gm/`](gm/) | procura item por classe, nível e raridade, e entrega no inventário direto no banco |

## `ui/` — cenas, texturas e o exe

| arquivo | para quê |
|---|---|
| `cena.py` | parse completo de `FieldScene*.bin`, `LoginScene*.bin`, `SelCharScene*.bin` |
| `anchor.py` | acha os campos de texto sem parsear o arquivo inteiro |
| `merge.py` | leva o layout de outro cliente preservando o texto local |
| `geo.py` | copia só a geometria de outro cliente (o caminho que não quebra) |
| `menu_esc.py` | devolve o menu do ESC ao formato original |
| `ids_exe.py` | os ~1865 ids de widget que o exe procura em runtime |
| `desmontar.py`, `campo.py` | desmontar em volta de um endereço do BugTrap; achar onde um campo é escrito |
| `jit.py` | texturas: DXT1/3/5, `JT20` cru e `JT20` em RLE |
| `icones.py` | id de item → sprite |

O raciocínio de cada um, e principalmente **as tentativas que não funcionaram**,
está no [`ui/README.md`](ui/README.md). Vale ler antes de repetir: transplantar
cena inteira entre builds custou três crashes, e o conserto que valia era uma
opção dentro do jogo.

## Regra que saiu dessas sessões

Transplanta bem o dado cujo id **ninguém referencia de fora**: cena de UI,
textura. Não transplanta o que é indexado por um id que o banco, o servidor ou
outra tabela apontam — item, skill, NPC. Foi por isso que a UI foi e a tabela
de itens não.

E antes de editar arquivo: **conferir se o que se quer já é uma opção do jogo.**
A barra de skill larga tinha um botão em Configurações → Geral.
