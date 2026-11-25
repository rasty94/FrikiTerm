//! Servidor FTP mínimo: una descarga que el servidor aborta (426) debe ser un error, y el
//! modo pasivo no debe conectar a la IP que dicte la respuesta PASV.

use friki_xfer::ftp::{FtpOptions, open};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

async fn serve(ctl: tokio::net::TcpStream, final_reply: &'static str) {
    let (rd, mut wr) = ctl.into_split();
    let mut rd = BufReader::new(rd);
    let _ = wr.write_all(b"220 hola\r\n").await;
    let mut data: Option<TcpListener> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let cmd = line.trim().to_ascii_uppercase();
        let reply: String = if cmd.starts_with("USER") {
            "331 pass\r\n".into()
        } else if cmd.starts_with("PASS") {
            "230 ok\r\n".into()
        } else if cmd.starts_with("TYPE") {
            "200 ok\r\n".into()
        } else if cmd.starts_with("REST") {
            "350 ok\r\n".into()
        } else if cmd.starts_with("PASV") {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let p = l.local_addr().unwrap().port();
            data = Some(l);
            // IP no enrutable: solo con el apaño NAT se conecta a 127.0.0.1.
            format!("227 pasivo (192,0,2,1,{},{})\r\n", p >> 8, p & 255)
        } else if cmd.starts_with("RETR") {
            let Some(l) = data.take() else { return };
            let _ = wr.write_all(b"150 enviando\r\n").await;
            if let Ok((mut d, _)) = l.accept().await {
                let _ = d.write_all(b"parcial").await;
            }
            final_reply.into()
        } else {
            "502 no\r\n".into()
        };
        let _ = wr.write_all(reply.as_bytes()).await;
    }
}

async fn download(final_reply: &'static str) -> std::io::Result<Vec<u8>> {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((c, _)) = l.accept().await {
            tokio::spawn(serve(c, final_reply));
        }
    });
    let fs = open(
        &FtpOptions {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            tls: false,
            passive: true,
        },
        "p",
    )
    .await
    .unwrap();
    let mut r = fs.open_read("/f", 0).await.unwrap();
    let mut out = Vec::new();
    r.read_to_end(&mut out).await?;
    Ok(out)
}

#[tokio::test]
async fn download_aborted_by_server_is_an_error() {
    assert!(download("426 abortada\r\n").await.is_err());
}

#[tokio::test]
async fn completed_download_is_ok() {
    assert_eq!(download("226 hecho\r\n").await.unwrap(), b"parcial");
}
