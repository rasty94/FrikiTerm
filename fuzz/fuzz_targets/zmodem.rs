#![no_main]
use friki_fuzz::{Fake, rt};
use friki_zmodem::{FileProtocol, detect, receive_ex};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let _ = detect(data);
    let Some((p, rest)) = data.split_first() else {
        return;
    };
    let proto = match p % 3 {
        0 => FileProtocol::Xmodem,
        1 => FileProtocol::Ymodem,
        _ => FileProtocol::Zmodem,
    };
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    let mut io = Fake(Cursor::new(rest.to_vec()));
    let r = rt().block_on(async {
        // Tope de 1 MiB para no llenar el disco.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            receive_ex(&mut io, proto, &dest, &mut |_| {}, 1 << 20),
        )
        .await
    });
    let r = r.expect("la recepcion no debe colgarse");
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
