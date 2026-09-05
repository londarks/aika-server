//! Exporta os ícones dos itens para uso fora do jogo — um site, por exemplo.
//!
//!     exportar <ItemList.bin> <pasta UI> <pasta de saída> [--soltos]
//!
//! Sem `--soltos` grava os atlas inteiros (`atlas01.png`..) mais um
//! `itens.json` dizendo em que atlas e em que pixel cada item começa. É o que
//! um site quer: o navegador baixa no máximo onze imagens e o resto é
//! `background-position`. Um PNG por item daria dezesseis mil requisições.
//!
//! Com `--soltos` grava um PNG de 42x42 por item, para quando são poucos itens
//! escolhidos a dedo.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use visualizador::{itens, jit};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let soltos = args.iter().any(|a| a == "--soltos");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 3 {
        eprintln!("uso: exportar <ItemList.bin> <pasta UI> <pasta de saída> [--soltos]");
        std::process::exit(2);
    }
    let (tabela_p, ui, saida) = (Path::new(pos[0]), PathBuf::from(pos[1]), PathBuf::from(pos[2]));

    let tabela = match itens::Tabela::abrir(tabela_p) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    std::fs::create_dir_all(&saida).expect("criar pasta de saída");

    let ids = tabela.povoados();
    let usados: BTreeSet<u16> =
        ids.iter().map(|&id| tabela.icone(id as usize)).filter(|&i| i != 0).collect();
    let atlas_usados: BTreeSet<u32> = usados.iter().map(|&i| itens::posicao(i).0).collect();
    eprintln!(
        "{} itens com nome, {} ícones distintos, {} atlas em uso",
        ids.len(),
        usados.len(),
        atlas_usados.len()
    );

    // Carrega só os atlas que algum item usa.
    let mut imagens = std::collections::HashMap::new();
    for &a in &atlas_usados {
        let p = ui.join(format!("ItemIcons{:02}.jit", a));
        match jit::ler(&p) {
            Ok(im) => {
                if !soltos {
                    let arq = saida.join(format!("atlas{:02}.png", a));
                    std::fs::write(&arq, im.png().expect("png do atlas")).expect("gravar atlas");
                    eprintln!("  atlas{:02}.png  {}x{}", a, im.largura, im.altura);
                }
                imagens.insert(a, im);
            }
            // Faltar atlas não é fatal: os itens dele saem com `atlas: null`.
            Err(e) => eprintln!("  atlas {a}: {e}"),
        }
    }

    let mut linhas = Vec::new();
    let mut sem_sprite = 0;
    for &id in &ids {
        let icone = tabela.icone(id as usize);
        let (a, x, y) = itens::posicao(icone);
        let tem = icone != 0 && imagens.contains_key(&a);
        if !tem {
            sem_sprite += 1;
        }
        if soltos && tem {
            let im = &imagens[&a];
            let corte = im.recorte(x, y, itens::CELULA, itens::CELULA);
            std::fs::write(saida.join(format!("{id}.png")), corte.png().expect("png"))
                .expect("gravar ícone");
        }
        linhas.push(format!(
            "  {{\"id\":{},\"nome\":{},\"nome_en\":{},\"icone\":{},{}}}",
            id,
            json_str(tabela.nome(id as usize)),
            json_str(tabela.nome_en(id as usize)),
            icone,
            if tem {
                format!("\"atlas\":{a},\"x\":{x},\"y\":{y}")
            } else {
                "\"atlas\":null,\"x\":null,\"y\":null".into()
            }
        ));
    }

    let json = format!(
        "{{\n\"celula\":{},\n\"itens\":[\n{}\n]\n}}\n",
        itens::CELULA,
        linhas.join(",\n")
    );
    std::fs::write(saida.join("itens.json"), json).expect("gravar itens.json");
    eprintln!(
        "itens.json com {} itens ({} sem sprite). Pronto em {}",
        ids.len(),
        sem_sprite,
        saida.display()
    );
}

/// Os nomes são latin-1 e chegam aqui como `char` por byte, então o escape
/// precisa cobrir aspas, barra e os controles — não dá para confiar em
/// `format!("{:?}")`.
fn json_str(s: Option<String>) -> String {
    let Some(s) = s else { return "null".into() };
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
