# Ferramenta de GM

Procura um item, escolhe um personagem, entrega. Escreve direto no
`var/aika.db`.

É de propósito uma ferramenta **de fora do jogo** em vez de uma janela dentro
dele: assim não fica brecha exposta no cliente, e quem não tem o executável não
tem o poder.

## Rodar

    cd src-tauri
    cargo tauri dev
    cargo build --release

Antes, copie `config.exemplo.toml` para `config.toml`. Três caminhos, relativos
ao próprio arquivo:

```toml
item_list = "../../assets/items/ItemList.bin"   # tabela do servidor
ui        = "../../../aika-client/UI"           # ícones
banco     = "../../var/aika.db"                 # onde escreve
```

Para conferir sem abrir a janela:

    cargo run -- --verificar

Lista os personagens, conta os itens por classe e faz uma busca. **Não** testa
uma entrega: teste que escreve no banco de verdade é pior que teste nenhum.

## O aviso que importa

Com o personagem **logado**, o servidor guarda o inventário em memória e
regrava a cada autosave — por cima do que a ferramenta colocou. A entrega some
sem erro nenhum.

A ferramenta detecta o `-wal` do SQLite e avisa quando o banco parece em uso,
mas isso é heurística. A regra segura: **deslogar o personagem antes de
entregar.**

## Como o item entra

Uma linha na tabela `items` por unidade, na primeira posição livre da bolsa:

```sql
INSERT INTO items (character_id, container, slot, item_index, appearance,
                   durability_min, durability_max, refine)
```

`container = 1` é a bolsa (`0` equipado, `2` armazém, de `inventory.rs`). A
bolsa tem 126 posições mas as seis últimas guardam as próprias bolsas, então a
ferramenta só usa `0..119`.

Uma linha por unidade em vez de uma pilha: empilhar depende do `CAN_GROUP` do
item e da regra de pilha do servidor. Solto sempre funciona, e o jogador
agrupa dentro do jogo.

## Os filtros

**Classe.** Os dois lados falam a mesma numeração, **0 a 5**, que é a do
`class_of` em `aika-server/src/creation.rs` (`class_index / 10 - 1`). O banco
guarda o personagem como 10, 20, 30…; o `HANDOFF.md` confirma `class_index 20 =
Templária` e `10 = Guerreiro`. A primeira versão desta ferramenta tinha duas
numerações convivendo (1-6 para o personagem, 0-5 para o item) — davam o mesmo
resultado por coincidência, e é o tipo de coisa que mente em silêncio quando
alguém mexe num lado só.

O campo `CLASSE` do item (`+300`) guarda a classe na dezena:
`0x` Guerreiro, `1x` Templária, `2x` Atirador, `3x` Pistoleira, `4x`
Feiticeiro, `5x` Clériga; a unidade é variante. Confirmei pela evidência, não
pelo palpite — toda pistola cai em 31/32 e todo cajado em 41/42. Item com
`CLASSE` zero (11.115 deles) não tem restrição e aparece para qualquer classe,
então o filtro nunca o esconde.

Valores acima de 100 aparecem em poucos registros e não seguem o padrão de
dezenas; são tratados como sem restrição em vez de sumirem.

**Nível** é `+330`, **raridade** é `+390` (0 a 7), **tipo** é `+258`.

## Reaproveitamento

Depende da lib do `visualizador` por caminho, e usa o mesmo `itens.rs` e
`jit.rs`. Se cada um tivesse o seu leitor, a ferramenta poderia mostrar um item
na tela e entregar outro no banco.
