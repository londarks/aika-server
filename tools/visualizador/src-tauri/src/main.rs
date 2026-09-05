#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Navegador de itens: a tabela do servidor de um lado, as sprites do cliente
//! do outro.
//!
//! Os dois arquivos que ele lê são dado de cliente e não moram no repositório;
//! os caminhos vêm de um `config.toml` ao lado, que é ignorado pelo git. Veja
//! `config.exemplo.toml`.

use visualizador::{itens, jit};

use base64::Engine;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Estado {
    tabela: itens::Tabela,
    dir_ui: PathBuf,
    /// Índice 0 = `ItemIcons01.jit`. Carregado sob demanda; `Err` fica guardado
    /// para não tentar de novo a cada rolagem.
    atlas: Vec<Option<Result<jit::Imagem, String>>>,
    /// PNG em base64 por índice de ícone.
    cache: HashMap<u16, String>,
    ids: Vec<u32>,
}

#[derive(Serialize)]
struct Item {
    id: u32,
    nome: Option<String>,
    nome_en: Option<String>,
    icone: u16,
    atlas: u32,
    png: Option<String>,
}

#[derive(Serialize)]
struct Pagina {
    total: usize,
    itens: Vec<Item>,
}

#[derive(Serialize)]
struct Resumo {
    total_itens: usize,
    com_nome: usize,
    atlas_encontrados: usize,
    config: String,
}

impl Estado {
    fn sprite(&mut self, indice: u16) -> Option<String> {
        if indice == 0 {
            return None;
        }
        if let Some(p) = self.cache.get(&indice) {
            return Some(p.clone());
        }
        let (atlas, x, y) = itens::posicao(indice);
        let i = (atlas - 1) as usize;
        if i >= self.atlas.len() {
            return None;
        }
        if self.atlas[i].is_none() {
            let caminho = self.dir_ui.join(format!("ItemIcons{:02}.jit", atlas));
            self.atlas[i] = Some(jit::ler(&caminho));
        }
        let img = match self.atlas[i].as_ref().unwrap() {
            Ok(im) => im,
            Err(_) => return None,
        };
        let png = img.recorte(x, y, itens::CELULA, itens::CELULA).png().ok()?;
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        );
        self.cache.insert(indice, url.clone());
        Some(url)
    }
}

#[tauri::command]
fn resumo(estado: tauri::State<Mutex<Estado>>) -> Resumo {
    let e = estado.lock().unwrap();
    let achados = (1..=12)
        .filter(|k| e.dir_ui.join(format!("ItemIcons{:02}.jit", k)).exists())
        .count();
    Resumo {
        total_itens: e.tabela.total(),
        com_nome: e.ids.len(),
        atlas_encontrados: achados,
        config: e.dir_ui.display().to_string(),
    }
}

#[tauri::command]
fn buscar(
    estado: tauri::State<Mutex<Estado>>,
    texto: String,
    inicio: usize,
    quantidade: usize,
) -> Pagina {
    let mut e = estado.lock().unwrap();
    pagina(&mut e, &texto, inicio, quantidade)
}

fn pagina(e: &mut Estado, texto: &str, inicio: usize, quantidade: usize) -> Pagina {
    let alvo = texto.trim().to_lowercase();

    let casa = |id: u32, e: &Estado| -> bool {
        if alvo.is_empty() {
            return true;
        }
        if let Ok(n) = alvo.parse::<u32>() {
            if id == n {
                return true;
            }
        }
        let pt = e.tabela.nome(id as usize).unwrap_or_default().to_lowercase();
        let en = e.tabela.nome_en(id as usize).unwrap_or_default().to_lowercase();
        pt.contains(&alvo) || en.contains(&alvo)
    };

    let filtrados: Vec<u32> = e.ids.iter().copied().filter(|&id| casa(id, e)).collect();
    let total = filtrados.len();
    let fatia: Vec<u32> = filtrados.into_iter().skip(inicio).take(quantidade).collect();

    let itens = fatia
        .into_iter()
        .map(|id| {
            let icone = e.tabela.icone(id as usize);
            let (atlas, _, _) = itens::posicao(icone);
            Item {
                id,
                nome: e.tabela.nome(id as usize),
                nome_en: e.tabela.nome_en(id as usize),
                icone,
                atlas,
                png: e.sprite(icone),
            }
        })
        .collect();

    Pagina { total, itens }
}

/// Exercita tudo menos a janela: config, tabela, atlas e recorte da sprite.
/// Sem isso a unica forma de saber se o backend funciona e olhar a tela.
fn verificar(mut e: Estado) {
    println!("tabela: {} registros, {} com nome", e.tabela.total(), e.ids.len());
    println!("pasta UI: {}", e.dir_ui.display());
    for consulta in ["", "shield", "poção", "1280"] {
        let p = pagina(&mut e, consulta, 0, 4);
        println!("
busca {consulta:?} -> {} resultados", p.total);
        for it in &p.itens {
            println!(
                "  #{:<6} icone {:<5} atlas {:<3} sprite {:<7} {}",
                it.id,
                it.icone,
                it.atlas,
                it.png.as_ref().map(|p| format!("{}B", p.len())).unwrap_or("nenhuma".into()),
                it.nome_en.clone().or(it.nome.clone()).unwrap_or_default()
            );
        }
    }
    let sem = (0..12).filter(|i| matches!(&e.atlas[*i], Some(Err(_)))).count();
    println!("
atlas que falharam ao carregar: {sem}");
}

/// `chave = "valor"`, uma por linha, `#` comenta. Não vale um crate de TOML só
/// para dois caminhos.
fn ler_config(caminho: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let Ok(txt) = std::fs::read_to_string(caminho) else {
        return m;
    };
    for linha in txt.lines() {
        let linha = linha.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = linha.split_once('=') {
            m.insert(
                k.trim().to_string(),
                v.trim().trim_matches(['"', '\'']).to_string(),
            );
        }
    }
    m
}

/// Procura o `config.toml`: variável de ambiente, diretório atual, e o pai —
/// que é onde ele fica quando se roda `cargo tauri dev` de dentro de
/// `src-tauri`.
fn achar_config() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AIKA_VISUALIZADOR_CONFIG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    for c in ["config.toml", "../config.toml", "../../config.toml"] {
        let p = PathBuf::from(c);
        if p.is_file() {
            return std::fs::canonicalize(p).ok();
        }
    }
    None
}

fn main() {
    let config = achar_config();
    let base = config
        .as_ref()
        .and_then(|c| c.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let vals = config.as_ref().map(|c| ler_config(c)).unwrap_or_default();

    let resolver = |chave: &str, padrao: &str| -> PathBuf {
        let v = vals.get(chave).map(String::as_str).unwrap_or(padrao);
        let p = PathBuf::from(v);
        if p.is_absolute() {
            p
        } else {
            base.join(p)
        }
    };

    let caminho_tabela = resolver("item_list", "../../assets/items/ItemList.bin");
    let dir_ui = resolver("ui", "../../../aika-client/UI");

    let tabela = match itens::Tabela::abrir(&caminho_tabela) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "não abriu a tabela de itens.\n  {e}\n\n\
                 Aponte `item_list` e `ui` no config.toml ao lado do app.\n\
                 Config em uso: {}",
                config.map(|c| c.display().to_string()).unwrap_or("nenhum".into())
            );
            std::process::exit(1);
        }
    };

    let ids = tabela.povoados();
    let estado = Estado {
        tabela,
        dir_ui,
        atlas: (0..12).map(|_| None).collect(),
        cache: HashMap::new(),
        ids,
    };

    if std::env::args().any(|a| a == "--verificar") {
        verificar(estado);
        return;
    }

    tauri::Builder::default()
        .manage(Mutex::new(estado))
        .invoke_handler(tauri::generate_handler![resumo, buscar])
        .run(tauri::generate_context!())
        .expect("erro ao subir a janela");
}
