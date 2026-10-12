//! 送信を手元で試すための受け手。`cargo run -p vw-dicom --example store_scp -- <ポート> <保存先> [AE タイトル] [--uncompressed-only]`
//!
//! 受け取ったファイルを保存先に `<SOP Instance UID>.dcm` で置き、アソシエーションごとに件数を出す。
//! `--uncompressed-only` を付けると、圧縮の転送構文を受けない相手として振る舞う。

use std::net::TcpListener;
use std::path::PathBuf;

use vw_dicom::net::receiver::Receiver;

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let uncompressed_only = args.iter().any(|a| a == "--uncompressed-only");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let port = positional.first().map_or("11112", |s| s.as_str());
    let dir = PathBuf::from(positional.get(1).map_or("received", |s| s.as_str()));
    let ae_title = positional.get(2).map_or("STORESCP", |s| s.as_str());
    std::fs::create_dir_all(&dir)?;
    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))?;
    println!(
        "listening on port {port} as {ae_title}, saving to {}",
        dir.display()
    );
    let receiver = Receiver {
        ae_title: ae_title.to_owned(),
        accept_compressed: !uncompressed_only,
        dir,
        store_status: 0,
    };
    for stream in listener.incoming() {
        match receiver.serve(stream?) {
            Ok(r) => println!("association: {} echo, {} files", r.echoes, r.files.len()),
            Err(e) => println!("association failed: {e}"),
        }
    }
    Ok(())
}
