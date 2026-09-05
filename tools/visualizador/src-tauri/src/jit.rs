//! Texturas `.jit` do cliente.
//!
//! Quatro variantes, todas com a largura e a altura no cabeçalho:
//!
//! | magia  | conteúdo                        | dados começam em |
//! |--------|---------------------------------|------------------|
//! | `JT31` | DXT1                            | 12               |
//! | `JT33` | DXT3                            | 12               |
//! | `JT35` | DXT5                            | 12               |
//! | `JT20` | BGRA cru (flag `0x02` em +6)    | 30               |
//! | `JT20` | BGRA em RLE (flag `0x0A` em +6) | 22               |
//!
//! O DXT vem sem mipmap nos arquivos de ícone; se houver cauda ela é ignorada,
//! porque só o nível 0 interessa.

use std::path::Path;

pub struct Imagem {
    pub largura: u32,
    pub altura: u32,
    /// RGBA, quatro bytes por pixel.
    pub rgba: Vec<u8>,
}

impl Imagem {
    /// Recorta uma região. Fora dos limites vira transparente.
    pub fn recorte(&self, x0: u32, y0: u32, w: u32, h: u32) -> Imagem {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let sy = y0 + y;
            if sy >= self.altura {
                continue;
            }
            for x in 0..w {
                let sx = x0 + x;
                if sx >= self.largura {
                    continue;
                }
                let o = ((sy * self.largura + sx) * 4) as usize;
                let d = ((y * w + x) * 4) as usize;
                rgba[d..d + 4].copy_from_slice(&self.rgba[o..o + 4]);
            }
        }
        Imagem { largura: w, altura: h, rgba }
    }

    pub fn png(&self) -> Result<Vec<u8>, String> {
        let buf = image::RgbaImage::from_raw(self.largura, self.altura, self.rgba.clone())
            .ok_or("buffer com tamanho errado")?;
        let mut saida = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut saida, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(saida.into_inner())
    }
}

pub fn ler(caminho: &Path) -> Result<Imagem, String> {
    let d = std::fs::read(caminho).map_err(|e| format!("{}: {e}", caminho.display()))?;
    if d.len() < 32 {
        return Err(format!("{}: curto demais", caminho.display()));
    }
    match &d[0..4] {
        b"JT31" => dxt(&d, 1),
        b"JT33" => dxt(&d, 3),
        b"JT35" => dxt(&d, 5),
        b"JT20" => {
            let w = u16::from_le_bytes([d[16], d[17]]) as u32;
            let h = u16::from_le_bytes([d[18], d[19]]) as u32;
            let bgra = if d[6] == 0x0A {
                rle(&d, w, h)?
            } else {
                let n = (w * h * 4) as usize;
                if d.len() < 30 + n {
                    return Err(format!("{}: dados curtos", caminho.display()));
                }
                d[30..30 + n].to_vec()
            };
            Ok(Imagem { largura: w, altura: h, rgba: bgra_para_rgba(bgra) })
        }
        outra => Err(format!("{}: magia desconhecida {outra:?}", caminho.display())),
    }
}

fn bgra_para_rgba(mut v: Vec<u8>) -> Vec<u8> {
    for p in v.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    v
}

/// `JT20` com flag `0x0A`.
///
/// Cabeçalho de 22 bytes — não 30 como o cru — e um rodapé de 8 que não é lido:
///
/// - controle `>= 0x80`: repete `C - 0x7F` vezes o pixel de 4 bytes seguinte;
/// - controle `<  0x80`: seguem `C + 1` pixels literais.
///
/// Confere quando sai exatamente `largura * altura` pixels; foi assim que o
/// formato foi identificado, e por isso o estouro aqui é erro e não aviso.
fn rle(d: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let alvo = (w * h) as usize;
    let mut saida = Vec::with_capacity(alvo * 4);
    let mut p = 22usize;
    let mut px = 0usize;
    while px < alvo {
        let c = *d.get(p).ok_or("fluxo RLE terminou cedo")?;
        p += 1;
        let n = if c & 0x80 != 0 { (c - 0x7F) as usize } else { (c + 1) as usize };
        let precisa = if c & 0x80 != 0 { 4 } else { 4 * n };
        if p + precisa > d.len() {
            return Err("fluxo RLE terminou cedo".into());
        }
        if c & 0x80 != 0 {
            let pixel = &d[p..p + 4];
            for _ in 0..n {
                saida.extend_from_slice(pixel);
            }
            p += 4;
        } else {
            saida.extend_from_slice(&d[p..p + 4 * n]);
            p += 4 * n;
        }
        px += n;
    }
    if px != alvo {
        return Err(format!("RLE gerou {px} pixels, esperado {alvo}"));
    }
    Ok(saida)
}

fn dxt(d: &[u8], variante: u8) -> Result<Imagem, String> {
    let w = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    let h = u32::from_le_bytes([d[8], d[9], d[10], d[11]]);
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(format!("dimensões implausíveis {w}x{h}"));
    }
    let passo = if variante == 1 { 8 } else { 16 };
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let (bw, bh) = ((w + 3) / 4, (h + 3) / 4);
    let mut o = 12usize;
    for by in 0..bh {
        for bx in 0..bw {
            if o + passo > d.len() {
                return Err("dados DXT curtos".into());
            }
            let bloco = &d[o..o + passo];
            o += passo;
            let (alfa, cor) = match variante {
                1 => ([255u8; 16], bloco),
                3 => (alfa_dxt3(&bloco[0..8]), &bloco[8..16]),
                _ => (alfa_dxt5(&bloco[0..8]), &bloco[8..16]),
            };
            let cores = cores_dxt(cor, variante == 1);
            let idx = u32::from_le_bytes([cor[4], cor[5], cor[6], cor[7]]);
            for i in 0..16u32 {
                let (px, py) = (bx * 4 + i % 4, by * 4 + i / 4);
                if px >= w || py >= h {
                    continue;
                }
                let c = cores[((idx >> (2 * i)) & 3) as usize];
                let dst = ((py * w + px) * 4) as usize;
                rgba[dst] = c[0];
                rgba[dst + 1] = c[1];
                rgba[dst + 2] = c[2];
                // No DXT1 o quarto índice é transparente quando c0 <= c1.
                rgba[dst + 3] = if variante == 1 { c[3] } else { alfa[i as usize] };
            }
        }
    }
    Ok(Imagem { largura: w, altura: h, rgba })
}

fn r565(v: u16) -> [u8; 4] {
    let (r, g, b) = ((v >> 11) & 0x1F, (v >> 5) & 0x3F, v & 0x1F);
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
        255,
    ]
}

fn cores_dxt(b: &[u8], dxt1: bool) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (a, c) = (r565(c0), r565(c1));
    let mistura = |x: u8, y: u8, p: u32, q: u32| ((x as u32 * p + y as u32 * q) / (p + q)) as u8;
    // O modo de três cores só existe no DXT1; no 3 e no 5 os quatro índices
    // são sempre interpolados e o alfa vem do bloco próprio.
    if dxt1 && c0 <= c1 {
        [
            a,
            c,
            [mistura(a[0], c[0], 1, 1), mistura(a[1], c[1], 1, 1), mistura(a[2], c[2], 1, 1), 255],
            [0, 0, 0, 0],
        ]
    } else {
        [
            a,
            c,
            [mistura(a[0], c[0], 2, 1), mistura(a[1], c[1], 2, 1), mistura(a[2], c[2], 2, 1), 255],
            [mistura(a[0], c[0], 1, 2), mistura(a[1], c[1], 1, 2), mistura(a[2], c[2], 1, 2), 255],
        ]
    }
}

fn alfa_dxt3(b: &[u8]) -> [u8; 16] {
    let mut a = [0u8; 16];
    for i in 0..16 {
        let nib = if i % 2 == 0 { b[i / 2] & 0x0F } else { b[i / 2] >> 4 };
        a[i] = nib * 17; // 0..15 -> 0..255
    }
    a
}

fn alfa_dxt5(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut tab = [0u32; 8];
    tab[0] = a0;
    tab[1] = a1;
    if a0 > a1 {
        for i in 2..8 {
            tab[i] = ((8 - i as u32) * a0 + (i as u32 - 1) * a1) / 7;
        }
    } else {
        for i in 2..6 {
            tab[i] = ((6 - i as u32) * a0 + (i as u32 - 1) * a1) / 5;
        }
        tab[6] = 0;
        tab[7] = 255;
    }
    let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
    let mut a = [0u8; 16];
    for i in 0..16 {
        a[i] = tab[((bits >> (3 * i)) & 7) as usize] as u8;
    }
    a
}
