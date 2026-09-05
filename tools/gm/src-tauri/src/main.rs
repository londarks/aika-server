#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Ferramenta de GM: procura um item, escolhe um personagem, entrega.
//!
//! Escreve direto no `var/aika.db`. É deliberadamente uma ferramenta de fora
//! do jogo em vez de uma janela dentro dele — assim não fica brecha exposta no
//! cliente, e quem não tem o executável não tem o poder.
//!
//! Reaproveita o leitor de tabela e de textura do `visualizador`. Se cada um
//! tivesse o seu, a ferramenta poderia mostrar um item e entregar outro.

mod banco;

use base64::Engine;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use visualizador::{itens, jit};

struct Estado {
    tabela: itens::Tabela,
    dir_ui: PathBuf,
    banco: banco::Banco,
    atlas: Vec<Option<Result<jit::Imagem, String>>>,
    cache: HashMap<u16, String>,
    ids: Vec<u32>,
}

#[derive(Serialize)]
struct Item {
    id: u32,
    nome: Option<String>,
    nome_en: Option<String>,
    descricao: Option<String>,
    classe: u32,
    classe_nome: String,
    tipo: u16,
    nivel: u16,
    raridade: u8,
    png: Option<String>,
}

#[derive(Serialize)]
struct Pagina {
    total: usize,
    itens: Vec<Item>,
}

#[derive(Serialize)]
struct Resumo {
    itens: usize,
    banco: String,
    em_uso: bool,
    classes: Vec<(u32, String)>,
}

#[derive(Serialize)]
struct Entrega {
    slots: Vec<i64>,
    aviso: Option<String>,
}

/// Item sem restrição de classe. Zero é "qualquer uma"; valores acima de 100
/// aparecem em poucos registros e não seguem o padrão de dezenas, então são
/// tratados como sem restrição em vez de sumirem do filtro.
fn familia_do_item(classe: u32) -> Option<u32> {
    if classe == 0 || classe >= 100 {
        None
    } else {
        Some(classe / 10)
    }
}

pub fn nome_da_familia(f: u32) -> &'static str {
    match f {
        0 => "Guerreiro",
        1 => "Templária",
        2 => "Atirador",
        3 => "Pistoleira",
        4 => "Feiticeiro",
        5 => "Clériga",
        _ => "?",
    }
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
            self.atlas[i] = Some(jit::ler(&self.dir_ui.join(format!("ItemIcons{:02}.jit", atlas))));
        }
        let img = self.atlas[i].as_ref().unwrap().as_ref().ok()?;
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
    Resumo {
        itens: e.ids.len(),
        banco: e.banco.caminho().display().to_string(),
        em_uso: e.banco.parece_em_uso(),
        classes: (0..6).map(|f| (f, nome_da_familia(f).to_string())).collect(),
    }
}

#[tauri::command]
fn personagens(estado: tauri::State<Mutex<Estado>>) -> Result<Vec<banco::Personagem>, String> {
    estado.lock().unwrap().banco.personagens()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn buscar(
    estado: tauri::State<Mutex<Estado>>,
    texto: String,
    classe: Option<u32>,
    nivel_max: Option<u16>,
    raridade: Option<u8>,
    inicio: usize,
    quantidade: usize,
) -> Pagina {
    let mut e = estado.lock().unwrap();
    let alvo = texto.trim().to_lowercase();
    let numero = alvo.parse::<u32>().ok();

    let casa = |id: u32, e: &Estado| -> bool {
        let i = id as usize;
        if let Some(f) = classe {
            // Item sem restrição serve a todo mundo, então nunca é filtrado.
            if let Some(fi) = familia_do_item(e.tabela.classe(i)) {
                if fi != f {
                    return false;
                }
            }
        }
        if let Some(n) = nivel_max {
            if e.tabela.nivel(i) > n {
                return false;
            }
        }
        if let Some(r) = raridade {
            if e.tabela.raridade(i) != r {
                return false;
            }
        }
        if alvo.is_empty() {
            return true;
        }
        if numero == Some(id) {
            return true;
        }
        let pt = e.tabela.nome(i).unwrap_or_default().to_lowercase();
        let en = e.tabela.nome_en(i).unwrap_or_default().to_lowercase();
        pt.contains(&alvo) || en.contains(&alvo)
    };

    let filtrados: Vec<u32> = e.ids.iter().copied().filter(|&id| casa(id, &e)).collect();
    let total = filtrados.len();
    let fatia: Vec<u32> = filtrados.into_iter().skip(inicio).take(quantidade).collect();

    let itens = fatia
        .into_iter()
        .map(|id| {
            let i = id as usize;
            let classe = e.tabela.classe(i);
            let icone = e.tabela.icone(i);
            Item {
                id,
                nome: e.tabela.nome(i),
                nome_en: e.tabela.nome_en(i),
                descricao: e.tabela.descricao(i),
                classe,
                classe_nome: familia_do_item(classe)
                    .map(|f| nome_da_familia(f).to_string())
                    .unwrap_or_else(|| "todas".into()),
                tipo: e.tabela.tipo(i),
                nivel: e.tabela.nivel(i),
                raridade: e.tabela.raridade(i),
                png: e.sprite(icone),
            }
        })
        .collect();

    Pagina { total, itens }
}

#[tauri::command]
fn entregar(
    estado: tauri::State<Mutex<Estado>>,
    personagem: i64,
    item: u32,
    quantidade: u32,
) -> Result<Entrega, String> {
    let e = estado.lock().unwrap();
    if quantidade == 0 || quantidade > 120 {
        return Err("quantidade fora de 1..120".into());
    }
    let aviso = e.banco.parece_em_uso().then(|| {
        "o banco parece estar em uso. Se o personagem estiver logado, o autosave \
         do servidor vai regravar o inventário por cima desta entrega — feche o \
         jogo, ou pelo menos deslogue o personagem, e confira depois."
            .to_string()
    });
    let slots = e.banco.entregar(personagem, item, quantidade, 0)?;
    Ok(Entrega { slots, aviso })
}

fn ler_config(caminho: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let Ok(txt) = std::fs::read_to_string(caminho) else { return m };
    for linha in txt.lines() {
        let linha = linha.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = linha.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    m
}

fn achar_config() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AIKA_GM_CONFIG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    ["config.toml", "../config.toml", "../../config.toml"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .and_then(|p| std::fs::canonicalize(p).ok())
}

/// Exercita tudo menos a janela, inclusive uma leitura do banco. Uma entrega
/// não é testada aqui de propósito: teste que escreve no banco de verdade é
/// pior que teste nenhum.
fn verificar(mut e: Estado) {
    println!("itens com nome: {}", e.ids.len());
    println!("banco: {} (em uso: {})", e.banco.caminho().display(), e.banco.parece_em_uso());
    match e.banco.personagens() {
        Ok(ps) => {
            println!("{} personagens:", ps.len());
            for p in &ps {
                println!(
                    "  #{:<3} {:<14} nv {:<3} {:<11} {:>3} itens na bolsa",
                    p.id, p.nome, p.nivel, p.classe_nome, p.ocupados
                );
            }
        }
        Err(err) => println!("ERRO lendo personagens: {err}"),
    }
    for f in 0..6u32 {
        let n = e.ids.iter().filter(|&&id| familia_do_item(e.tabela.classe(id as usize)) == Some(f)).count();
        println!("  itens exclusivos de {:<11} {n}", nome_da_familia(f));
    }
    let livres = e.ids.iter().filter(|&&id| familia_do_item(e.tabela.classe(id as usize)).is_none()).count();
    println!("  itens sem restrição de classe: {livres}");
    let p = buscar_interno(&mut e, "staff", Some(4), None, None, 0, 3);
    println!("\nbusca 'staff' + Feiticeiro -> {} resultados", p.total);
    for it in &p.itens {
        println!(
            "  #{:<6} nv{:<3} {:<11} sprite {:<5} {}",
            it.id,
            it.nivel,
            it.classe_nome,
            if it.png.is_some() { "sim" } else { "não" },
            it.nome_en.clone().unwrap_or_default()
        );
    }
}

/// Mesma filtragem que o comando, sem o `State` — o `verificar` precisa dela.
fn buscar_interno(
    e: &mut Estado,
    texto: &str,
    classe: Option<u32>,
    nivel_max: Option<u16>,
    raridade: Option<u8>,
    inicio: usize,
    quantidade: usize,
) -> Pagina {
    let alvo = texto.trim().to_lowercase();
    let filtrados: Vec<u32> = e
        .ids
        .iter()
        .copied()
        .filter(|&id| {
            let i = id as usize;
            if let Some(f) = classe {
                if let Some(fi) = familia_do_item(e.tabela.classe(i)) {
                    if fi != f {
                        return false;
                    }
                }
            }
            if let Some(n) = nivel_max {
                if e.tabela.nivel(i) > n {
                    return false;
                }
            }
            if let Some(r) = raridade {
                if e.tabela.raridade(i) != r {
                    return false;
                }
            }
            alvo.is_empty()
                || e.tabela.nome(i).unwrap_or_default().to_lowercase().contains(&alvo)
                || e.tabela.nome_en(i).unwrap_or_default().to_lowercase().contains(&alvo)
        })
        .collect();
    let total = filtrados.len();
    let itens = filtrados
        .into_iter()
        .skip(inicio)
        .take(quantidade)
        .map(|id| {
            let i = id as usize;
            let classe = e.tabela.classe(i);
            let icone = e.tabela.icone(i);
            Item {
                id,
                nome: e.tabela.nome(i),
                nome_en: e.tabela.nome_en(i),
                descricao: e.tabela.descricao(i),
                classe,
                classe_nome: familia_do_item(classe)
                    .map(|f| nome_da_familia(f).to_string())
                    .unwrap_or_else(|| "todas".into()),
                tipo: e.tabela.tipo(i),
                nivel: e.tabela.nivel(i),
                raridade: e.tabela.raridade(i),
                png: e.sprite(icone),
            }
        })
        .collect();
    Pagina { total, itens }
}

fn main() {
    let config = achar_config();
    let base = config
        .as_ref()
        .and_then(|c| c.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let vals = config.as_ref().map(|c| ler_config(c)).unwrap_or_default();
    let resolver = |chave: &str, padrao: &str| -> PathBuf {
        let p = PathBuf::from(vals.get(chave).map(String::as_str).unwrap_or(padrao));
        if p.is_absolute() {
            p
        } else {
            base.join(p)
        }
    };

    let tabela = match itens::Tabela::abrir(&resolver("item_list", "../../assets/items/ItemList.bin")) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("não abriu a tabela de itens.\n  {e}\n\nAjuste o config.toml ao lado.");
            std::process::exit(1);
        }
    };
    let ids = tabela.povoados();
    let estado = Estado {
        tabela,
        dir_ui: resolver("ui", "../../../aika-client/UI"),
        banco: banco::Banco::novo(&resolver("banco", "../../var/aika.db")),
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
        .invoke_handler(tauri::generate_handler![resumo, personagens, buscar, entregar])
        .run(tauri::generate_context!())
        .expect("erro ao subir a janela");
}
