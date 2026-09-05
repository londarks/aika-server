//! Converte uma textura `.jit` do cliente em PNG.
//!
//!     jit2png <entrada.jit> <saida.png>
//!
//! Serve para conferir o decodificador contra outra implementação e para
//! olhar um atlas inteiro sem abrir a janela.

fn main() {
    let mut a = std::env::args().skip(1);
    let (Some(entrada), Some(saida)) = (a.next(), a.next()) else {
        eprintln!("uso: jit2png <entrada.jit> <saida.png>");
        std::process::exit(2);
    };
    match visualizador::jit::ler(std::path::Path::new(&entrada)) {
        Ok(im) => {
            println!("{}x{}", im.largura, im.altura);
            std::fs::write(&saida, im.png().expect("codificar png")).expect("gravar");
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
