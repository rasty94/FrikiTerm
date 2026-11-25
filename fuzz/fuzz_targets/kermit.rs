#![no_main]
use std::time::Duration;

use friki_fuzz::rt;
use friki_zmodem::{FileProtocol, receive_ex};
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let r = rt().block_on(async {
        // Extremo del receptor y extremo del "remoto": los bytes entran por `peer`.
        let (io, mut peer) = tokio::io::duplex(1 << 16);
        let d = &dest;
        // `io` se mueve al futuro: al terminar el receptor se cierra y `peer` ve EOF.
        let recv = async move {
            let mut io = io;
            let mut progress = |_| {};
            receive_ex(&mut io, FileProtocol::Kermit, d, &mut progress, 1 << 20).await
        };
        let feed = async {
            let _ = peer.write_all(data).await;
            let _ = peer.shutdown().await;
            // Descarta los ACK/NAK del receptor hasta que cierre su extremo.
            let mut sink = Vec::new();
            let _ = peer.read_to_end(&mut sink).await;
        };
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(recv, feed).0 }).await
    });
    let r = r.expect("la recepcion Kermit no debe colgarse");
    if let Ok(r) = r {
        for f in r.files {
            assert!(f.starts_with(&dest), "escritura fuera del destino: {f:?}");
        }
    }
    // Nada creado fuera de `dest`.
    for e in std::fs::read_dir(dir.path()).unwrap() {
        assert_eq!(e.unwrap().path(), dest);
    }
});
