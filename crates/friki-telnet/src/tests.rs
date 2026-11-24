use super::proto::*;
use super::*;
use tokio::io::duplex;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

fn opts(tls: bool, raw: bool) -> TelnetOptions {
    TelnetOptions {
        tls,
        raw,
        term: "xterm-256color".into(),
        connect_timeout_secs: 5,
    }
}

fn run(session: &mut Session, parser: &mut Parser, bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    session.process(parser.feed(bytes))
}

#[test]
fn parser_splits_data_commands_and_escapes() {
    let mut p = Parser::default();
    let items = p.feed(&[
        b'a', IAC, IAC, b'b', IAC, 253, 31, b'c', IAC, 250, 24, 1, IAC, 240,
    ]);
    assert_eq!(
        items,
        vec![
            Item::Data(vec![b'a', 255, b'b']),
            Item::Neg(253, 31),
            Item::Data(vec![b'c']),
            Item::Sub(vec![24, 1]),
        ]
    );
}

#[test]
fn parser_handles_split_chunks() {
    let mut p = Parser::default();
    assert!(p.feed(&[IAC]).is_empty());
    assert!(p.feed(&[253]).is_empty());
    assert_eq!(p.feed(&[31]), vec![Item::Neg(253, 31)]);
}

#[test]
fn naws_sent_on_do_and_on_resize_with_iac_escaped() {
    let mut s = Session::new("vt100", 80, 24);
    let mut p = Parser::default();
    let (_, reply) = run(&mut s, &mut p, &[IAC, 253, OPT_NAWS]);
    assert_eq!(
        reply,
        [
            IAC, 251, OPT_NAWS, IAC, 250, OPT_NAWS, 0, 80, 0, 24, IAC, 240
        ]
    );
    // 255 columnas: el byte 255 se duplica
    let sb = s.resize(255, 24).unwrap();
    assert_eq!(sb, [IAC, 250, OPT_NAWS, 0, IAC, IAC, 0, 24, IAC, 240]);
}

#[test]
fn resize_without_naws_sends_nothing() {
    let mut s = Session::new("vt100", 80, 24);
    assert!(s.resize(100, 30).is_none());
}

#[test]
fn ttype_answers_send() {
    let mut s = Session::new("vt100", 80, 24);
    let mut p = Parser::default();
    run(&mut s, &mut p, &[IAC, 253, OPT_TTYPE]);
    let (_, reply) = run(&mut s, &mut p, &[IAC, 250, OPT_TTYPE, 1, IAC, 240]);
    let mut want = vec![IAC, 250, OPT_TTYPE, 0];
    want.extend(b"vt100");
    want.extend([IAC, 240]);
    assert_eq!(reply, want);
}

#[test]
fn hostile_repeated_negotiation_never_loops() {
    let mut s = Session::new("vt100", 80, 24);
    let mut p = Parser::default();
    let mut flood = Vec::new();
    for _ in 0..1000 {
        flood.extend([
            IAC, 253, OPT_SGA, IAC, 251, OPT_ECHO, IAC, 253, 99, IAC, 251, 98,
        ]);
    }
    let (_, reply) = run(&mut s, &mut p, &flood);
    // WILL SGA, DO ECHO, WONT 99, DONT 98: una vez cada uno
    assert_eq!(
        reply,
        [
            IAC, 251, OPT_SGA, IAC, 253, OPT_ECHO, IAC, 252, 99, IAC, 254, 98
        ]
    );
    // DONT/WONT sobre lo ya apagado no generan respuesta
    let (_, reply) = run(&mut s, &mut p, &[IAC, 254, 99, IAC, 252, 98]);
    assert!(reply.is_empty());
}

#[test]
fn encode_escapes_iac_and_cr() {
    let mut s = Session::new("vt100", 80, 24);
    assert_eq!(s.encode(&[b'a', 255, b'\r']), [b'a', 255, 255, b'\r', 0]);
    let mut p = Parser::default();
    run(&mut s, &mut p, &[IAC, 253, OPT_BINARY]);
    assert_eq!(s.encode(b"\r"), b"\r");
}

#[test]
fn incoming_cr_nul_is_cr() {
    let mut s = Session::new("vt100", 80, 24);
    let mut p = Parser::default();
    let (d, _) = run(&mut s, &mut p, b"a\r\0b\r");
    assert_eq!(d, b"a\rb\r");
    let (d, _) = run(&mut s, &mut p, b"\0c");
    assert_eq!(d, b"c");
}

#[tokio::test]
async fn stream_negotiates_and_delivers_data() {
    let (client, mut server) = duplex(4096);
    let (w, mut r) = start(client, &opts(false, false), 80, 24);
    server
        .write_all(&[IAC, 253, OPT_NAWS, b'h', b'i'])
        .await
        .unwrap();
    assert_eq!(
        r.next_event().await,
        Some(StreamEvent::Data(b"hi".to_vec()))
    );
    let mut got = [0u8; 12];
    server.read_exact(&mut got).await.unwrap();
    assert_eq!(&got[..3], [IAC, 251, OPT_NAWS]);
    w.resize(100, 40).await.unwrap();
    let mut sb = [0u8; 9];
    server.read_exact(&mut sb).await.unwrap();
    assert_eq!(sb, [IAC, 250, OPT_NAWS, 0, 100, 0, 40, IAC, 240]);
    w.write(b"x\r").await.unwrap();
    let mut x = [0u8; 3];
    server.read_exact(&mut x).await.unwrap();
    assert_eq!(x, [b'x', b'\r', 0]);
    drop(server);
    assert_eq!(r.next_event().await, Some(StreamEvent::Closed));
}

#[tokio::test]
async fn raw_mode_passes_bytes_untouched() {
    let (client, mut server) = duplex(4096);
    let (w, mut r) = start(client, &opts(false, true), 80, 24);
    server.write_all(&[IAC, 253, 1, b'z']).await.unwrap();
    assert_eq!(
        r.next_event().await,
        Some(StreamEvent::Data(vec![IAC, 253, 1, b'z']))
    );
    w.write(&[255, b'\r']).await.unwrap();
    let mut x = [0u8; 2];
    server.read_exact(&mut x).await.unwrap();
    assert_eq!(x, [255, b'\r']);
}

// ---- TLS con servidor local de prueba ----

struct TestCert {
    der: CertificateDer<'static>,
    key: Vec<u8>,
}

fn cert_for(name: &str) -> TestCert {
    let ck = rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
    TestCert {
        der: ck.cert.der().clone(),
        key: ck.signing_key.serialize_der(),
    }
}

/// Servidor TLS que saluda con "hola" y cierra. Devuelve el puerto.
async fn tls_server(c: &TestCert) -> u16 {
    let cfg = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![c.der.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(c.key.clone())),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let a = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut t) = a.accept(s).await {
                    let _ = t.write_all(b"hola").await;
                    let _ = t.flush().await;
                    let mut b = [0u8; 1];
                    let _ = t.read(&mut b).await;
                }
            });
        }
    });
    port
}

fn roots_with(c: &TestCert) -> RootCertStore {
    let mut r = RootCertStore::empty();
    r.add(c.der.clone()).unwrap();
    r
}

#[tokio::test]
async fn tls_accepts_trusted_cert_with_matching_name() {
    let c = cert_for("localhost");
    let port = tls_server(&c).await;
    let (_w, mut r) = connect_with(
        "localhost",
        port,
        &opts(true, true),
        80,
        24,
        Some(roots_with(&c)),
    )
    .await
    .unwrap();
    assert_eq!(
        r.next_event().await,
        Some(StreamEvent::Data(b"hola".to_vec()))
    );
}

#[tokio::test]
async fn tls_rejects_wrong_name() {
    let c = cert_for("otro.example");
    let port = tls_server(&c).await;
    let r = connect_with(
        "localhost",
        port,
        &opts(true, true),
        80,
        24,
        Some(roots_with(&c)),
    )
    .await;
    assert!(r.is_err());
}

#[tokio::test]
async fn tls_rejects_unknown_ca_with_system_roots() {
    let c = cert_for("localhost");
    let port = tls_server(&c).await;
    // `connect` público usa solo las raíces del sistema: el certificado propio no es de confianza
    let r = connect("localhost", port, &opts(true, true), 80, 24).await;
    assert!(r.is_err());
}

// ---- Integración con telnetd en Docker (scripts/test-telnetd.sh up) ----

#[tokio::test]
#[ignore = "necesita FRIKI_TEST_TELNET_PORT (scripts/test-telnetd.sh up)"]
async fn docker_telnetd_echo() {
    let port: u16 = std::env::var("FRIKI_TEST_TELNET_PORT")
        .expect("FRIKI_TEST_TELNET_PORT")
        .parse()
        .unwrap();
    let (w, mut r) = connect("127.0.0.1", port, &opts(false, false), 80, 24)
        .await
        .unwrap();
    w.write(b"echo $((6*7))ok\r").await.unwrap();
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(StreamEvent::Data(d))) =
            tokio::time::timeout(Duration::from_secs(2), r.next_event()).await
        else {
            continue;
        };
        seen.extend(d);
        if String::from_utf8_lossy(&seen).contains("42ok\r\n") {
            return;
        }
    }
    panic!("sin respuesta: {:?}", String::from_utf8_lossy(&seen));
}
