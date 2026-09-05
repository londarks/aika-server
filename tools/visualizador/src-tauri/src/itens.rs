//! A tabela de itens e o mapa de um item para a sua sprite.
//!
//! A cópia do servidor (`assets/items/ItemList.bin`) é texto claro: registros
//! fixos de 464 bytes, sem cabeçalho, indexados pelo id do item. O layout dos
//! campos está em `crates/aika-data/src/itemlist.rs`; aqui só se lê o punhado
//! que a tela usa.
//!
//! O índice do ícone é um `u16` em `+320`. Os atlas são `UI/ItemIcons01.jit` em
//! diante, todos 1024x1024, com célula de **42x42** e 24 por linha:
//!
//! ```text
//! atlas  = índice / 576 + 1
//! célula = índice % 576
//! x, y   = (célula % 24) * 42, (célula / 24) * 42
//! ```
//!
//! Célula de 42 e não 32: 24 * 42 = 1008, sobrando 16 pixels de borda. Supor 32
//! rende recortes que parecem quase certos e não são.

pub const REGISTRO: usize = 464;
pub const CELULA: u32 = 42;
pub const POR_LINHA: u32 = 24;
pub const POR_ATLAS: u16 = (POR_LINHA * POR_LINHA) as u16; // 576

const NOME: usize = 0;
const NOME_EN: usize = 64;
const DESCRICAO: usize = 128;
const ITEM_TYPE: usize = 258;
const CLASSE: usize = 300;
const NIVEL: usize = 330;
const ICONE: usize = 320;
const RARIDADE: usize = 390;

/// As seis, na ordem em que a tela de criação oferece. O campo `CLASSE` do
/// item usa a mesma numeração do `class_index` do personagem: dezena = classe,
/// unidade = variante. Zero quer dizer "qualquer uma".
pub const CLASSES: [(u32, &str); 6] = [
    (1, "Guerreiro"),
    (2, "Templária"),
    (3, "Atirador"),
    (4, "Pistoleira"),
    (5, "Feiticeiro"),
    (6, "Clériga"),
];

/// `class_index` do personagem (10, 20, 30…) para o índice 1-6 acima.
pub fn familia_do_personagem(class_index: i64) -> u32 {
    (class_index as u32 / 10).clamp(0, 6)
}

/// `CLASSE` do item para a mesma família. O campo guarda 1, 2, 11, 12, 22…:
/// a unidade é a classe e a dezena a variante, ao contrário do personagem.
pub fn familia_do_item(classe: u32) -> u32 {
    if classe == 0 { 0 } else { classe % 10 }
}

pub struct Tabela {
    dados: Vec<u8>,
}

impl Tabela {
    pub fn abrir(caminho: &std::path::Path) -> Result<Tabela, String> {
        let dados = std::fs::read(caminho).map_err(|e| format!("{}: {e}", caminho.display()))?;
        if dados.len() < REGISTRO {
            return Err(format!("{}: nem um registro cabe", caminho.display()));
        }
        Ok(Tabela { dados })
    }

    pub fn total(&self) -> usize {
        self.dados.len() / REGISTRO
    }

    fn campo(&self, id: usize, off: usize, tam: usize) -> &[u8] {
        let o = id * REGISTRO + off;
        &self.dados[o..o + tam]
    }

    /// Nome localizado. `None` quando o registro está vazio.
    pub fn nome(&self, id: usize) -> Option<String> {
        texto(self.campo(id, NOME, 64))
    }

    pub fn nome_en(&self, id: usize) -> Option<String> {
        texto(self.campo(id, NOME_EN, 64))
    }

    pub fn descricao(&self, id: usize) -> Option<String> {
        texto(self.campo(id, DESCRICAO, 128))
    }

    pub fn classe(&self, id: usize) -> u32 {
        let c = self.campo(id, CLASSE, 4);
        u32::from_le_bytes([c[0], c[1], c[2], c[3]])
    }

    pub fn tipo(&self, id: usize) -> u16 {
        let c = self.campo(id, ITEM_TYPE, 2);
        u16::from_le_bytes([c[0], c[1]])
    }

    pub fn nivel(&self, id: usize) -> u16 {
        let c = self.campo(id, NIVEL, 2);
        u16::from_le_bytes([c[0], c[1]])
    }

    /// Zero a sete.
    pub fn raridade(&self, id: usize) -> u8 {
        self.campo(id, RARIDADE, 1)[0]
    }

    pub fn icone(&self, id: usize) -> u16 {
        let c = self.campo(id, ICONE, 2);
        u16::from_le_bytes([c[0], c[1]])
    }

    /// Ids com nome, que é o que dá para mostrar.
    pub fn povoados(&self) -> Vec<u32> {
        (0..self.total())
            .filter(|&i| self.nome(i).is_some() || self.nome_en(i).is_some())
            .map(|i| i as u32)
            .collect()
    }
}

/// O campo é latin-1 terminado em zero, com lixo depois do terminador em
/// registros que já foram reescritos — por isso corta no primeiro zero.
fn texto(b: &[u8]) -> Option<String> {
    let fim = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    if fim == 0 {
        return None;
    }
    let s: String = b[..fim].iter().map(|&c| c as char).collect();
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Onde a sprite de um índice de ícone mora: número do atlas e canto da célula.
pub fn posicao(indice: u16) -> (u32, u32, u32) {
    let atlas = indice / POR_ATLAS + 1;
    let celula = (indice % POR_ATLAS) as u32;
    (atlas as u32, (celula % POR_LINHA) * CELULA, (celula / POR_LINHA) * CELULA)
}
