#![no_main]
use friki_fuzz::{Fake, rt};
use friki_xfer::ftp::parse_listing;
use friki_xfer::{local_target, safe_name, scp};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let _ = safe_name(&s);
    let _ = local_target(Path::new("/x/y"), &s);
    let lines: Vec<String> = s.lines().map(str::to_owned).collect();
    let _ = parse_listing("/d", &lines, false);
    let _ = parse_listing("/d", &lines, true);

    // Cabeceras SCP hostiles: nada debe escribirse fuera de `dest`.
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let mut io = Fake(Cursor::new(data.to_vec()));
    let r = rt().block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            scp::download_stream(&mut io, &dest, true),
        )
        .await
    });
    r.expect("scp no debe colgarse").ok();
    for e in std::fs::read_dir(dir.path()).unwrap() {
        assert_eq!(e.unwrap().path(), dest);
    }
});
