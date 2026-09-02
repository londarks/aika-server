# Aika-RS — Prompt de Revival (handoff entre sessões)

> Cole isto no começo de uma sessão nova pra me trazer de volta ao contexto.
> Escrito em 2026-08-31, atualizado em 2026-09-01. Idioma de trabalho:
> **português** (o Gabriel/londarks fala PT-BR).

---

## 1. O que é o projeto

Reescrita do servidor do MMORPG **Aika Online** em **Rust**, portando o
comportamento de um servidor privado original em **Delphi**. Testado ao vivo
contra o cliente real de 2008 do Gabriel.

**Regra de ouro:** o servidor Delphi é a autoridade. Sempre que for implementar
algo, **abra o Delphi e ache o arquivo que é dono daquele comportamento** antes
de escrever qualquer linha. Nunca invente comportamento a partir de fragmentos —
já quebrou várias vezes assim. Frase do Gabriel: *"confira sempre delphi como
ele fez"* e *"copia o dephil"*.

## 2. Onde fica cada coisa (tudo irmão de `aika-rs/`, lado a lado)

Caminhos relativos de propósito: o `CLAUDE.md` proíbe caminho absoluto num
arquivo rastreado, porque ele carrega o nome de usuário e o layout da máquina
de onde veio. De dentro de `aika-rs/`, cada um destes é `../<nome>`.

| Pasta | O que é |
|---|---|
| `aika-rs/` | **O nosso servidor** (workspace Rust). É aqui que se trabalha. É um repo git. |
| `aika-delphi-bin/Src/` | **Fonte Delphi de referência** (~103k linhas). Só leitura — NUNCA executar os binários. |
| `aika-client/` | O nosso cliente (linha de dados **BR**, protocolo 124). É o que o Gabriel usa pra testar. |
| `aika-pacote-original/` | Pacote original baixado (documentação; reimplementar em Rust, não rodar). |
| (cliente CBM, fora desta árvore) | Cliente de um servidor comercial (linha **TK**, cifrado, GameGuard). Não mexer/decifrar — é de terceiro no ar. |

### Workspace `aika-rs/`
- `crates/aika-net/` — protocolo e cifra dos pacotes.
- `crates/aika-data/` — leitores dos formatos de arquivo (itens, skills, mobs, templates, exp, drops, SL).
- `crates/aika-server/` — o servidor. Módulos: `game.rs` (dispatch + a maioria dos handlers), `world.rs` (mundo/jogadores/mobs), `mob.rs` (IA dos monstros), `combat.rs`, `ability.rs` (skills/grid), `shop.rs`, `dialog.rs` (NPC), `creation.rs` (criar personagem), `inventory.rs`, `stats.rs`, `store.rs` (Character/Account em memória), `db.rs` (SQLite), `login.rs`, `web.rs` (endpoints ASP do launcher), `http.rs`, `config.rs`, `state.rs`, `main.rs`.
- `assets/` — dados do lado servidor (texto claro): `items/ItemList.bin`, `skills/SkillData.bin`, `mobs/` (AllMobsInfo.csv + MonsterListCSV.csv), `templates/*.acc`, `ExpList.bin`, `drops/`, `npcs/*.npc`. **Ignorados no git** (dados externos).
- `config.toml` — configuração, tudo com caminhos relativos.
- `var/` — banco `aika.db`, logs. Ignorado no git.

## 3. Regras do repo (do CLAUDE.md — cumprir à risca)
- **Nunca** caminho absoluto num arquivo rastreado (vaza o nome de usuário/máquina — já aconteceu e foi problema sério). Dado externo vai em `assets/` (ignorado) e se referencia relativo.
- **Nunca** dados reais de conta, nem dados do cliente/servidor original comitados.
- Nunca executar binário do pacote original.
- Não decifrar/extrair conteúdo do CBM (servidor comercial de terceiro no ar).

## 4. Como rodar / testar (Windows, PowerShell + Bash tool disponíveis)

O binário fica travado quando o servidor está rodando. Padrão que funciona:

```powershell
# parar, buildar, subir destacado com log em arquivo (NÃO usar run_in_background
# do bash — o pipe enche e trava o servidor no write do log)
taskkill /IM aika-server.exe /F 2>$null
# build:  cargo build -p aika-server   (rodar de dentro de aika-rs/)
Start-Process -FilePath ".\target\debug\aika-server.exe" -WorkingDirectory "<aika-rs>" `
  -RedirectStandardOutput "var\server.log" -RedirectStandardError "var\server.err.log" -WindowStyle Hidden
```

- Testes: `cargo test` (rodar de `aika-rs/`). Hoje: ~265 no aika-server, ~74 no aika-data, ~9 no aika-net, ~8+10 integração. **Manter tudo verde.**
- Portas: web/launcher `127.0.0.1:8090`, login `127.0.0.1:8831`, game `127.0.0.1:8822` (nos IPs `.1`–`.4`, um por canal).
- Cliente aponta pra cá via `Setting.txt` (`1 http://127.0.0.1:8090`) e `SL.bin`.
- Para mexer no banco com o servidor parado: `python` tem módulo `sqlite3` (o `python` do bash funciona pra scripts; o do PowerShell às vezes cai no alias da Store).

## 5. Estado de teste atual
- Conta `admin` / `admin`, nation 2. 3 personagens, 100 pontos de skill, gold 999999999: `Athus` (slot 0, class_index 20 = Templária), `Samurai` (slot 1, class_index 10 = Guerreiro, **nível 99**), `ALice` (slot 2).
- **O teto jogável é 99, não 100.** Foi decidido em 2026-09-01. A faixa de nível de um item (`Level`..`MaxLvl`) é checada **só pelo cliente** — o Delphi nunca lê `MaxLvl` — e acima de 99 ele recusa em silêncio, sem mandar pacote nenhum. A sela de montaria é `10..99` e o melhor equipamento ganho de cada classe é o tier de nível 96. A `ExpList.bin` tem 100 níveis, mas esse é o teto do arquivo, não o jogável.
- O cap 50 do Delphi (`LEVEL_CAP`) é outra coisa: é só o **portão da Promotion Quest** (50→51).
- O Samurai está com o set tier 13 (nível 96), a montaria `[927] Regulus Negra` no slot 9 de equipamento e selas na bolsa.
- O Samurai tem uma **Pran** (`alice`), pedra de invocação no slot 10 de equipamento. Tabela `prans` própria, uma linha por conta.
- **Banco: SQLite ou MySQL, mesmo código.** `AIKA_DATABASE_URL` no `.env` (ignorado pelo git) ganha do `url` do `config.toml`, que ganha do `path`. Testado contra um MySQL 8.4 real: cria o schema, semeia e sobe.

## 6. Protocolo (aika-net)
- Header de 12 bytes, little-endian: `size u16@0, checksum u8@2, seed u8@3, index u16@4, opcode u16@6, time u32@8`. Corpo depois.
- Cifra própria (ver `aika-net/src/crypto.rs`): checksum cobre seed, não o payload. `Message { sender, opcode, time, body }`; `frame::encode(&msg, seed)`.
- Texto é **latin-1** em todo lugar.
- **Armadilha recorrente:** vários pacotes chegam mais curtos que o `record` Delphi (bytes "spare" do record não vão no fio). O fio decide o tamanho, não o record. Vale pra login, criação, etc.

## 7. Divisão cliente ↔ servidor (essencial entender)
O cliente é **fixo** e já tem todos os dados (nomes, ícones, árvore de skills, lojas). O servidor manda **o que existe/vale** e o cliente desenha. Consequências:
- Item: **stats/preço/tipo/efeito** vêm do `ItemList.bin` do **servidor** (texto claro, editável por nós, 31000 ids × 464 bytes, ~16714 definidos). **Ícone/nome/descrição** vêm do `ItemList4.bin` do **cliente** (cifrado). Pra item novo reaproveitando arte existente: edita só o servidor. Pra arte/nome novo: teria que repackar o cliente (cifra não-trivial; não é XOR simples — já testei).
- Espaço de ids compartilhado: players 1..2000 (cap 200), NPCs 2048..3047, **mobs 3048+**.

## 8. Formatos de arquivo (aika-data) — offsets já descobertos
- **SkillData.bin**: 12000 registros × **720 bytes** + 4 bytes de trailer (registros começam em 0). Campos: FAMILY@0, MIN_LEVEL@4, RANK@12, CLASS@156, MANA@172, PRE_COOLDOWN@180, COOLDOWN@184 (ms), DAMAGE@248, ANIMATION@356, SKILL_POINTS@148, LEARN_COST@152. **123 skills têm cooldown de 20–60min (1.2M–3.6M ms) — é dado real (buffs/utilitários), não bug.**
- **ItemList.bin**: 31000 × 464 + 4 trailer. Campos: CAN_GROUP@256, ITEM_TYPE@258, PRICE_HONOR@280, PRICE_MEDAL@284, PRICE_GOLD@288, **SELL_PRICE@292 (é o preço de compra em gold!)**, PRICE_ITEM@440, PRICE_ITEM_VALUE@442, DURATION@336.
- **Templates `.acc`** (`Data/BaseAccs/*.acc`, 7249 bytes): CHARACTER_AT=4, CHARACTER_SIZE=6384, SKILLS_AT=6433 (46 pares index/rank: 6 basics + 40 others). Dentro do registro: EQUIP@340 (16×20), INVENTORY@664 (126×20), GOLD@3184, **SkillList@4596 (60 words)**, **ItemBar@4716 (40 dwords = hotbar)**. Classes/CLASS_INFO@29 = 1/11/21/31/41/51 (Guerreiro/Templária/Atirador/Pistoleira/Feiticeiro/Clériga).
- **Registro do personagem no `0x925`** (mesmo layout do template): CLIENT_ID@0, NAME@12, NATION@28, CLASS_INFO@29, TStatus(CurrentScore)@32, MAX_HP@48, CUR_HP@52, MAX_MP@56, CUR_MP@60, **SkillPoint@82** (dentro do TStatus, offset 50), EXP@176, LEVEL@184, EQUIP@340, INVENTORY@664, GOLD@3184, SkillList@4596, ItemBar@4716.
- Mobs: `AllMobsInfo.csv` (kinds) + `MonsterListCSV.csv` (spawns). Id = `Count + 3048`. Ponto final da patrulha = 2º ponto do CSV **+5** nos dois eixos. "Mutante"/"Crenon"/guardas (20 índices fixos em IfGuard) não andam. "Max*", mutantes e guardas não são "lurados" (área inicial toda é Max → andável).

### Dado do lado do **cliente** (2026-09-01) — ferramentas em `tools/ui/`
- **Cenas de UI** (`UI/FieldScene*.bin`, `LoginScene*.bin`, `SelCharScene*.bin`): fluxo de registros, sem cabeçalho de arquivo. `registro = N palavras i32 + K campos de texto de 128 bytes`; `w[0]=tipo w[1]=id w[2]=pai w[4..7]=x,y,larg,alt` (relativos ao pai). Tipos com texto: **4 (botão) = 48B + 2 textos**, **15 (label) = 52B + 1**, 16 = 60B + 1, 33 = 36B + 1. Os demais são 48B sem texto. Padding do campo é `0x00` **ou** `0xFE` conforme o arquivo, e **`0xFE` nunca faz parte da string** — foi o que fez a primeira detecção falhar. Ids não cabem em 10^6 (existe `0x01000030`).
- **O exe busca widget por id** numa virtual do slot **`0x54`** (`push <id>; mov eax,[ecx]; call [eax+0x54]`) e **não checa nulo**: id que a cena não tem estoura em `movsx eax, word ptr [eax+0x42]`. `tools/ui/ids_exe.py` extrai os ~1865 ids exigidos. Crash de UI começa por aí.
- **Texturas `.jit`**: `JT31`/`JT33`/`JT35` = DXT1/3/5 com largura e altura em `+4`/`+8`, dados em 12. `JT20` = BGRA; flag em `+6`: `0x02` cru a partir de 30, **`0x0A` em RLE a partir de 22** (`C>=0x80` repete `C-0x7F` vezes o pixel seguinte; `C<0x80` traz `C+1` literais) com rodapé de 8 bytes.
- **Sprite de item**: índice é `u16` em **`+320`** do registro de 464 do ItemList. `atlas = idx/576 + 1` (`UI/ItemIcons01..11.jit`), `célula = idx%576`, `x,y = (cél%24)*42, (cél/24)*42`. Célula de **42**, não 32 — com 32 os recortes ficam *quase* certos e mandam a investigação para o lado errado. 11 atlas = 6336 ícones; 15 itens da tabela apontam acima disso e não têm arte em nenhum cliente que temos.

## 9. Skills — modelo (aika-server/ability.rs) — ONDE MAIS SE ERRA
- **Grid** (`GetSkillIndex`): cada classe tem 960 ids consecutivos (`class_block`), cada slot 16 ranks. `skill_index(class, slot, rank)`. `belongs_to` valida a classe. `record_slot(class, id) = (id - class_block - 1)/16` → índice 0-59 no SkillList (0-5 basics, 6-45 others).
- **Basics vs Others:** as 6 basics (slots 0-5) incluem o **ataque básico** e o retorno; toda classe nasce com elas rank 1. `bar_of` (janela K, `0x106`) manda **só as others**; as basics aparecem por outra área ("básico") e por marcadores no registro.
- **SkillList do registro NÃO é copiado do template** — é **computado** como `SetPlayerSkills`: `2` pra cada basic aprendida (slots 0-5), o nível pra cada other aprendida (6-45). Sem os marcadores `2`, o cliente **cancela o cast** (manda `0x31E`+`0x327` em vez de `0x320`). Isso é o que fazia "as magias não funcionarem".
- Rede de segurança na entrada do mundo: se `skill_list[0..6]` vier tudo zero (char antigo), marca as 6 basics com `2`.

## 10. Onde o cliente lê cada coisa (aprendido na marra)
- **Pontos de skill:** campo `SkillsPoint` do **`0x109` (`SendRefreshPoint`)**, body offset **14-16**. (Também mando no registro `0x925`@82 e no `0x107`, mas o que o display lê é o `0x109`.) Já teve bug de mandar `0` hardcoded aqui.
- **Skills aprendidas / níveis:** `0x107` (`SendPlayerSkillsLevel`) = 60 words do SkillList + SkillPoints(word) + 0xCCCC. Mandado no `world_burst`.
- **Árvore de skills (janela K):** `0x106` (`SendPlayerSkills`) = NPCIndex + SendType + 40 words (ids das others). **Ao aprender, reenviar com o NPCIndex** (SendType=0x0B) ou o treinador não redesenha e o jogador clica 2×.
- **Hotbar:** `ItemBar` no registro `0x925`@4716. Arrastar = `0x31E` (`ChangeItemBar`): tipo 2 (skill) → `ItemBar[dest]=id*16+2`, tipo 6 (item) → `=id`, tipo 0 → limpa; ecoa de volta.
- **Level up:** `SendEffect(1)` → `0x117` (`TSendClientIndexPacket`, Index=clientId, Effect=1) pra todos visíveis.

## 11. O que JÁ está implementado (dispatch atual em game.rs)
Login/token (web ASP), lista/criação/deleção de personagem, entrar no mundo (burst de pacotes na ordem do `SendToWorldSends`), mover/rotacionar, **chat** (`0xF86`: normal + sussurro), NPC dialog (`0x30F` menu), **loja** (comprar `0x313` incl. **moeda-item** via bolsa / vender `0x314`), inventário (mover `0x70F`, usar `0x31D`, jogar fora `0x32C`, **agrupar `0x332` / dividir `0x333`**), **hotbar `0x31E`**, combate (ataque `0x302?`→ `combat::OP_ATTACK`, dano com 2 animações), **skills** (usar `0x320`, **aprender `0x31C`**), morrer/reviver (`0x303`), **IA de mobs literal do `Mob/MOB.pas`** (2 threads: movimento a cada 3s, combate a cada 1s; patrulha/perseguição/leash/snap-home), drops, level up (curva + efeito), stats de equipamento.

**Persistência (SQLite, `db.rs`):** posição, gold, inventário, `skill_list` (BLOB), `item_bar` (BLOB), `skill_points`. Migração tolerante (`ALTER TABLE` que engole "duplicate column") pra banco antigo. Autosave 5s + no logout.

### Entrou em 2026-09-01
- **Ações/emotes (`0x304`)**: sentar e dançar ficam na presença e vão pra quem chega perto; andar/lançar levanta. Uma dança nunca é a pedida — o original sorteia uma de onze.
- **Hora (`0x202`)**: `DateTimeToStr(Now)` como client message. Só isso mesmo.
- **Bau (`0x137`/`0x310`/`0xF59`)**: 86 slots **da conta**, paginado (4 cofres em 80-83 destravam as páginas, 84-85 são pran), tabela `storage_items` própria, gold entra e sai. Item tipo 226 abre.
- **Regra de página** na bolsa também (bolsas em 120-125) — antes dava pra largar item em página não comprada.
- **Ficha do personagem (`0x10A`)**: ia **tudo zerado** menos a velocidade. Números agora do `GetCurrentScore` (`Mob/BaseMob.pas:3457`) — ataque **só da arma**, defesa **só da armadura**, constituição não dá defesa, peça sem durabilidade não vale nada. Reenviada quando o equipamento muda.
- **Montaria visível**: `ItemEffMontaria`/`ItemEffPedra` no spawn (offsets 34 e 32) — o array de equip do spawn tem 8 slots e não alcança o 9.
- **Prazo de item (`expiry.rs`)**: item emprestado sai carimbado. Flag `Expires` no byte 328 (conferida: 1450 itens têm, todos com duração, nenhum sem). Codificação estranha do original: montaria conta **dias** desde 01/01/2023 22:00; o resto guarda `unix >> 8` em **3 bytes a partir do byte 17**, invadindo o topo do `Refi`.
- **Buffs (`buffs.rs`)**: lista por skill, perguntada por **família**. Tipos 702 (poção, gasta) e 715/716 (`0x21B`, não gasta) fazem `AddBuff(UseEffect)`. Duração `0xFFFFFFFF` = não expira (é a da montaria); o original trunca isso no pacote e nós saturamos.
- **Guard de slot de equipamento**: item só entra no slot que o tipo manda (`GetItemEquipSlot`). Uma sela no slot 9 fazia o cliente travar carregando um cavalo que não era um.
- **Skill de montaria (`0x218`)**: `check_chosen` pula a checagem de posse — as skills da montaria são **classe 0** e não caem no bloco de classe nenhuma, então nosso próprio validador as recusava.

### Entrou em 2026-09-02
- **Promoção de classe (`promotion.rs`)**: `ClassInfo` é classe×10 + **tier** (`GetMobClass` é um `div 10`). O tier é guardado, e o teto de nível vem dele: **50, 89, 99**. O original nunca mexe nesse dígito — nem quest, nem comando — então quem promove aqui é a opção Quest do NPC, como substituto da linha de quests que o jogo real tinha (nível 50, com Pran equipada).
- **Pran (`pran.rs`)** — sistema quase inteiro, ver secção 17.
- **MySQL** além do SQLite, mesmo schema (`Dialect` reescreve a chave auto-incremental e o texto com comprimento).
- **Trace de pacotes (`trace.rs`)**: anel dos últimos 48 por conexão, nas duas direções, despejado sozinho quando o cliente cala 8 s estando no mundo. `AIKA_TRACE=1` registra tudo ao vivo. **Decodifica antes de registrar** — mostrava o quadro cifrado e mentia o opcode por um.
- **Achou dois congelamentos de cliente com ele:** (a) um ataque com id do próprio jogador virava auto-buff e enraizava o personagem — o original decide por `TargetType = 1` e nunca pelo id do pacote; (b) carimbávamos `state.uptime_ms()` no campo `time` do cabeçalho e o cliente recusava equipar com "Pode equipar depois de N seg" — **o original manda sempre 0** (`Player.pas` e `BaseMob.pas` atribuem `Header.Time` zero vezes).
- **Durabilidade**: 21 itens estavam gravados 0/0 onde a tabela diz 120/160. O original preenche as duas metades da tabela em todo item que entrega. Reparo no boot, ancorado no **teto** (desgaste só baixa a primeira metade).
- **Baú pela conversa com NPC** (opções 7 e 13). Nenhuma das duas estava ligada; a 13 é a **Central da Pran** — mesmo baú, `STORAGE_TYPE_PRANS`.

## 12. Falta (roadmap) — ordem sugerida pra jogo solo
1. **Efeitos de buff/equipamento (`EF`/`EFV` da skill e do item, lidos por `GetMobAbility` dentro do `GetCurrentScore`).** É a maior pendência: hoje o buff começa, aparece e libera a montaria, mas **não muda número nenhum** — a poção não dá os +10% e a montaria não corre mais. Os acessórios (anel, brinco, colar) também guardam o valor aí, por isso valem zero na janela C.
2. **Skills**: distribuir ponto de status (`0x213`, `GetStatusPoint` já lido), reset de skills, e **efeito do rank aprendido** (aprender sobe o rank no registro mas o combate ainda usa dano do rank-1).
3. **Inventário**: reparar (tipos 708/709, já lidos em `UseItem`), encantar, craftar.
4. Pequenos: Quest (1), Dungeon (2).
5. **Painel da Pran** — ver secção 17. Não é mais trabalho de servidor às cegas.
6. **Troca de canal** — grande e de pouco valor solo: exige antes separar `World` por canal (hoje os 4 canais dividem um só) e depois o handshake de token do `LoginIntoChannel`.
7. Só quando tiver gente: Guild(13), Party/raid(12), Amigos/duelo(8), Troca(7), Nação/relíquia(7), Correio(6), Leilão(5), Títulos/eventos/mapa(8).

**Já resolvido, não procurar:** PIN (`NumericToken` morre num `Exit;`, a parte viva já é o nosso `0xF02`) e `0x308 KarakAereo` (idem).

## 13. Padrões que funcionam / erros recorrentes
- **Funciona:** ler o `record` Delphi → verificar offsets empiricamente contra TODOS os arquivos de uma vez → implementar → teste que trava o byte/offset.
- **Erro recorrente:** inventar comportamento em vez de achar o arquivo dono dele; confundir "record Delphi" com "o que vai no fio"; assumir que o cliente lê de onde eu acho (ele lê de pacotes específicos — confirmar no Delphi qual `Send*` popula cada campo da UI).
- **Ferramenta que mente custa mais que ferramenta nenhuma.** O trace registrava o quadro cifrado e reportou opcodes errados por um durante uma sessão inteira; dois diagnósticos foram argumentados em cima de números que eram ficção.
- **Ordem importa e não é uniforme.** O mesmo par de pacotes sai em ordens opostas em caminhos diferentes do original. Seguir cada caminho como ele é, não escolher um e aplicar em todos.
- **Recusa silenciosa do cliente**: quando nenhuma linha aparece no log, o pacote não saiu do cliente. Foi assim que se achou o nome da Pran, a durabilidade e o relógio do cabeçalho.
- Ao adicionar pacote no `world_burst`, **conserta os testes de ordem/índice de frames** (`client_ready_spawns...` checa índices).
- **Log vazio = o pacote nunca saiu do cliente.** Todo pacote que chega é logado, inclusive os não implementados. Se uma ação não deixa linha nenhuma, quem recusou foi o cliente, e o motivo costuma ser a faixa de nível do item ou um pré-requisito que falta. Custou duas rodadas em 2026-09-01. Comparar o item que falha com um do **mesmo handler** que funciona, campo a campo, é o que acha.
- **Resultado errado pode ser a UI do cliente, não o protocolo.** Um pack de UI quebrado fez o cliente parar de mandar ataque e magia, e mostrou o tempo de um buff como 689 meses. Trocar pela UI original resolveu sem tocar no servidor.
- **`SELECT *` + `ALTER TABLE` no mesmo processo faz o driver entrar em pânico**, não falhar. As colunas são nomeadas em `load_accounts`/`load_characters` por isso.
- Fixture irreal esconde bug: os personagens de teste não tinham as seis bolsas nem durabilidade no equipamento, e por isso nenhum teste pegava a regra de página nem a peça quebrada.

## 14. Memória persistente
Há memória em `~/.claude/projects/.../memory/` (arquivos + `MEMORY.md`). Vale atualizar com fatos não-óbvios (offsets, quirks do Delphi) conforme surgem.

## 15. Bancada (`tools/`)
Ver [`tools/README.md`](tools/README.md). Duas pastas: `tools/ui/` (scripts de
cena, textura e desmontagem) e `tools/visualizador/` (app Tauri para navegar os
itens com a sprite ao lado, útil para montar drop e loja). Nenhuma guarda
caminho de cliente: linha de comando ou `config.toml` ignorado.

**Regra que saiu daí:** transplanta bem o dado cujo id ninguém referencia de
fora — cena de UI, textura. Não transplanta o que é indexado por id que o banco,
o servidor ou outra tabela apontam — item, skill, NPC. E **antes de editar
arquivo, conferir se o que se quer já é opção do jogo**: a barra de skill larga
que parecia bug tinha um botão em Configurações → Geral.

## 16. O mapa — o que já foi medido (nada implementado)
Levantamento de `aika-client/Env/`, só medição de tamanho, sem parser ainda:
- **`z01.hgt` é o mundo inteiro: 4096x4096 de `i16` little-endian**, 33.554.436 bytes = 2^25 + 4 de rodapé. **Confirmado renderizando** — sai o mapa legível, com crateras, dois fortes em pentágono e plantas de cidade. Alturas de -3878 a 3365, mediana 0: é **com sinal**, e ler como `u16` faz o terreno virar preto e branco em 0/65535 (`-4` vira 65532) e parecer máscara em vez de relevo.
- **`Z<zz><xx><yy>.hgt`, 175 arquivos de 131.072 bytes** = 256x256 `i16`. São as telhas: 4096/256 = 16, ou seja 16x16 = 256 posições, 175 em uso.
- **`AttributeS01.dat`, 33.554.432 bytes** = 2^25 exato. Mesma grade 4096x4096 a 2 bytes por célula: é o mapa de atributo/colisão, e é exatamente o que o servidor precisaria para validar movimento em vez de confiar no cliente.
- **`Z<...>.dat`, 52.202 bytes cada** — provável colocação de objetos por telha. Não decodificado.
- **`Objects/`: 2.234 arquivos `.MS3`** — MilkShape 3D, formato aberto e documentado. **`Mesh/`: 5.918 `.msh`** (formato próprio). **`Texture/`: 6.738 `.jit`**, formato já resolvido.

Ordem barata se um dia for mexer: `.hgt` (grade `i16` simples, já lida) → `AttributeS01.dat`
(mesma grade, e serve ao servidor hoje) → `Z*.dat` (objetos) → malhas.

## 17. Pran — o que funciona e o que não
Sistema em `pran.rs`, tabela `prans` (por conta, como o baú).

**Funciona:** nasce da Pedra da Pran que a quest entrega (`Quests.csv`: NPC 2072, quests 39/40/41, recompensa **item 100 fogo / 101 água / 102 ar** — o elemento está escrito na pedra); os números de criação são os do `FinishQuest`; é batizada (`0x3E02`, e **o cliente não a deixa sair do baú sem nome**); é invocada pela pedra no slot 10; a primeira forma é só um efeito no jogador (2/4/8 por elemento) e as seguintes têm corpo próprio com id da faixa **44241..45240**; ganha **um quinto** do que o dono mata; sobe de nível (209 vida / 356 mana); trava nos muros **4, 19 e 49**; e **evolui pela quest** (406 no muro 4, 407 no 19 — no jogo real quem faz é a **Moa Bariel**).

**Coisas que custaram caro e não devem ser reaprendidas:**
- O nome da Pran mora no **registro do personagem** (`Character.PranName[0..1]`), não em pacote de Pran. Sem ele o cliente pede nome de uma Pran já batizada, recusa deixá-la sair do baú, e não há saída.
- **A forma é o nível, não a classe** — `0..3` fada, `4` muro, `5..18` criança, `19` muro, `20..48` adolescente, `49` muro, `50..69` adulta (comentários do próprio original em `Mob/BaseMob.pas:6177`).
- O `0x907` **não carrega nível**. O único que carrega é o `0x116`, e vai `Level + 1`.
- Uma Pran nasce em **nível 0**: o `FinishQuest` não atribui `Level` nenhuma vez.
- Evoluir troca a pedra em **dois lugares**: `Pran.Equip[0]` **e** `Character.Equip[10]` (100/101/102 → 104 → 105).
- A ordem dos dois pacotes **difere por caminho**: spawn-depois-descreve na chegada, descreve-depois-spawn na evolução.

**NÃO funciona — o painel da Pran desenha sempre a primeira forma.** O corpo ao lado do jogador está certo; a janela não. Já descartado, cada um testando: todo pacote que o original manda é mandado e nenhum falta (`SetPranEquipAtributes` e `SetPranPassiveSkill` não mandam nada, `SendPranDevotionAndFood` nunca é chamado); a ordem é a dele; **todo** campo do `0x907` está preenchido, inclusive os 16 bytes de níveis de skill do `GetSkillPranLevel`; o nível sai no `0x116` na invocação e no ganho; classe, pedra da Pran e pedra vestida foram postas juntas e separadas. **Os dois caminhos que restam:** capturar um `0x907` do servidor original com uma Pran crescida e comparar byte a byte, ou enganchar no cliente pelo overlay d3d9.

**Falta na Pran:** comida descendo, devoção subindo, a bolsa dela e os seis slots de equipamento (`PRAN_EQUIP_TYPE`, um quarto container), e as dez skills fazerem alguma coisa além de serem contadas.
