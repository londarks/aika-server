//! O banco do servidor, visto de fora.
//!
//! Escrever no `var/aika.db` com o personagem logado **não adianta**: o
//! servidor guarda o inventário em memória e regrava a cada autosave, por cima
//! do que a gente colocou. Por isso toda entrega checa antes se o banco parece
//! estar em uso e avisa; a decisão de seguir é do usuário, mas informada.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// `inventory.rs` do servidor: 0 equipado, 1 bolsa, 2 armazém.
pub const BOLSA: i64 = 1;
/// A bolsa tem 126 posições, mas as seis últimas guardam as próprias bolsas.
pub const BOLSA_LIVRES: i64 = 120;

#[derive(Serialize, Clone)]
pub struct Personagem {
    pub id: i64,
    pub nome: String,
    pub nivel: i64,
    /// O `class_index` cru do banco: 10, 20, 30…
    pub classe: i64,
    /// A mesma numeração 0-5 que o campo `CLASSE` do item usa, para os dois
    /// lados da tela falarem a mesma língua.
    pub familia: u32,
    pub classe_nome: String,
    pub gold: i64,
    pub ocupados: i64,
}

pub struct Banco {
    caminho: PathBuf,
}

impl Banco {
    pub fn novo(caminho: &Path) -> Banco {
        Banco { caminho: caminho.to_path_buf() }
    }

    pub fn caminho(&self) -> &Path {
        &self.caminho
    }

    fn abrir(&self) -> Result<Connection, String> {
        Connection::open(&self.caminho).map_err(|e| format!("{}: {e}", self.caminho.display()))
    }

    /// O SQLite deixa um `-wal` enquanto alguém está com o banco aberto em
    /// modo WAL. Não é prova de que o servidor está no ar, mas é o sinal mais
    /// barato que existe, e errar para o lado do aviso é o certo aqui.
    pub fn parece_em_uso(&self) -> bool {
        let wal = self.caminho.with_extension("db-wal");
        wal.exists() && std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false)
    }

    pub fn personagens(&self) -> Result<Vec<Personagem>, String> {
        let c = self.abrir()?;
        let mut st = c
            .prepare(
                "SELECT c.id, c.name, c.level, c.class_index, c.gold,
                        (SELECT COUNT(*) FROM items i
                          WHERE i.character_id = c.id AND i.container = ?1)
                   FROM characters c
                  WHERE c.deleted_at IS NULL
                  ORDER BY c.name",
            )
            .map_err(|e| e.to_string())?;
        let linhas = st
            .query_map(params![BOLSA], |r| {
                let classe: i64 = r.get(3)?;
                Ok(Personagem {
                    id: r.get(0)?,
                    nome: r.get(1)?,
                    nivel: r.get(2)?,
                    classe,
                    familia: familia(classe),
                    classe_nome: crate::nome_da_familia(familia(classe)).to_string(),
                    gold: r.get(4)?,
                    ocupados: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        linhas.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Põe `quantidade` cópias do item na bolsa, uma por posição livre.
    ///
    /// Uma por posição porque empilhar depende do `CAN_GROUP` do item e da
    /// regra de pilha do servidor; entregar solto sempre funciona e o jogador
    /// agrupa dentro do jogo se quiser.
    pub fn entregar(
        &self,
        personagem: i64,
        item: u32,
        quantidade: u32,
        durabilidade: i64,
    ) -> Result<Vec<i64>, String> {
        let mut c = self.abrir()?;
        let tx = c.transaction().map_err(|e| e.to_string())?;

        let ocupadas: Vec<i64> = {
            let mut st = tx
                .prepare("SELECT slot FROM items WHERE character_id = ?1 AND container = ?2")
                .map_err(|e| e.to_string())?;
            let it = st
                .query_map(params![personagem, BOLSA], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            it.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        };

        let livres: Vec<i64> = (0..BOLSA_LIVRES)
            .filter(|s| !ocupadas.contains(s))
            .take(quantidade as usize)
            .collect();
        if livres.len() < quantidade as usize {
            return Err(format!(
                "só há {} posições livres na bolsa, e você pediu {quantidade}",
                livres.len()
            ));
        }

        for &slot in &livres {
            tx.execute(
                "INSERT INTO items
                   (character_id, container, slot, item_index, appearance,
                    durability_min, durability_max, refine)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5, 1)",
                params![personagem, BOLSA, slot, item as i64, durabilidade],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(livres)
    }
}

/// `class_index` do personagem para 0-5.
///
/// Mesma conta do `class_of` em `aika-server/src/creation.rs`: o banco guarda
/// 10, 20, 30… e a classe é `/10 - 1`. Repetida aqui porque depender do crate
/// do servidor inteiro por uma divisão não vale — mas é a mesma regra, e mudar
/// uma sem a outra faz a ferramenta mentir sobre a classe.
pub fn familia(class_index: i64) -> u32 {
    ((class_index / 10) - 1).clamp(0, 5) as u32
}
