//! Pruebas del flujo 3270, direcciones, campos, entrada y negociación (servidor falso).

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::client::{Client, Tn3270Options};
use crate::datastream::StreamError;
use crate::ebcdic::{from_char, to_char};
use crate::keys::Key;
use crate::screen::{InputError, Screen, aid, code6};
use crate::session::Session;

const IAC: u8 = 255;
const EOR: u8 = 239;

fn ebc(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| from_char(c).expect("carácter sin EBCDIC"))
        .collect()
}

/// Dirección de 12 bits (buffer de 24x80 o menor).
fn addr12(a: usize) -> [u8; 2] {
    [code6((a >> 6) as u8), code6(a as u8)]
}

/// Texto de una fila: celdas sin atributo de campo, nulos como espacio.
fn row(s: &Screen, r: usize) -> String {
    (0..s.cols())
        .map(|c| to_char(s.cell(r, c).unwrap().ch))
        .collect()
}

/// Registro: EW + WCC (restablece teclado) + órdenes dadas.
fn ew(orders: &[u8]) -> Vec<u8> {
    let mut v = vec![0xF5, 0xC3];
    v.extend_from_slice(orders);
    v
}

fn sba(a: usize) -> Vec<u8> {
    let mut v = vec![0x11];
    v.extend(addr12(a));
    v
}

#[test]
fn escribe_sba_y_texto() {
    let mut s = Screen::model(2);
    let mut r = ew(&sba(80));
    r.extend(ebc("HOLA"));
    assert_eq!(s.process(&r), Ok(vec![]));
    assert_eq!(row(&s, 1)[..4], *"HOLA");
    assert_eq!(s.cell(0, 0).unwrap().ch, 0);
    assert!(!s.keyboard_locked());
}

#[test]
fn sf_crea_campo_con_atributos() {
    let mut s = Screen::model(2);
    // protegido + intenso en 0, entrada normal en 10
    let mut r = ew(&[0x1D, 0x28, 0xC8]);
    r.extend(sba(10));
    r.extend([0x1D, 0x00]);
    s.process(&r).unwrap();
    assert_eq!(s.cell(0, 0).unwrap().fa, Some(0x28));
    assert_eq!(s.cell(0, 1).unwrap().ch, from_char('H').unwrap());
    assert!(s.is_protected(1));
    assert!(!s.is_protected(11));
    assert_eq!(s.cell(0, 10).unwrap().fa, Some(0x00));
}

#[test]
fn sfe_aplica_atributo_de_campo() {
    let mut s = Screen::model(2);
    s.process(&ew(&[0x29, 0x01, 0xC0, 0x20, 0xC1])).unwrap();
    assert_eq!(s.cell(0, 0).unwrap().fa, Some(0x20));
    assert_eq!(s.cell(0, 1).unwrap().ch, from_char('A').unwrap());
}

#[test]
fn sa_cambia_atributo_extendido() {
    let mut s = Screen::model(2);
    s.process(&ew(&[0x28, 0x41, 0xF2, 0xC1])).unwrap();
    assert_eq!(s.cell(0, 0).unwrap().ext.hl, 0xF2);
}

#[test]
fn ic_pone_el_cursor() {
    let mut s = Screen::model(2);
    let mut r = ew(&sba(100));
    r.push(0x13);
    s.process(&r).unwrap();
    assert_eq!(s.cursor(), 100);
}

#[test]
fn pt_salta_al_siguiente_campo_de_entrada() {
    let mut s = Screen::model(2);
    let mut r = ew(&[0x1D, 0x00]);
    r.extend(sba(10));
    r.extend([0x1D, 0x00]);
    r.extend(sba(3));
    r.push(0x05);
    r.push(0xC9);
    s.process(&r).unwrap();
    assert_eq!(s.cell(0, 11).unwrap().ch, from_char('I').unwrap());
}

#[test]
fn ra_repite_hasta_la_direccion() {
    let mut s = Screen::model(2);
    let mut r = ew(&sba(0));
    r.push(0x3C);
    r.extend(addr12(5));
    r.push(0xC1);
    s.process(&r).unwrap();
    assert_eq!(&row(&s, 0)[..6], "AAAAA ");
}

#[test]
fn eua_borra_solo_hasta_la_direccion() {
    let mut s = Screen::model(2);
    let mut r = ew(&ebc("ABCDEF"));
    r.extend(sba(0));
    r.push(0x12);
    r.extend(addr12(3));
    s.process(&r).unwrap();
    assert_eq!(&row(&s, 0)[..6], "   DEF");
}

#[test]
fn ge_marca_caracter_de_grafico() {
    let mut s = Screen::model(2);
    s.process(&ew(&[0x08, 0xC1])).unwrap();
    let c = s.cell(0, 0).unwrap();
    assert_eq!((c.ch, c.ext.cs), (0xC1, 0xF1));
}

#[test]
fn mf_modifica_atributo_en_la_posicion() {
    let mut s = Screen::model(2);
    let mut r = ew(&[0x1D, 0x00]);
    r.extend(sba(0));
    r.extend([0x2C, 0x01, 0xC0, 0x20]);
    s.process(&r).unwrap();
    assert_eq!(s.cell(0, 0).unwrap().fa, Some(0x20));
}

#[test]
fn sba_al_final_envuelve_al_inicio() {
    let mut s = Screen::model(2);
    let mut r = ew(&sba(1919));
    r.extend(ebc("AB"));
    s.process(&r).unwrap();
    assert_eq!(s.cell(23, 79).unwrap().ch, from_char('A').unwrap());
    assert_eq!(s.cell(0, 0).unwrap().ch, from_char('B').unwrap());
}

#[test]
fn direcciones_invalidas_son_error() {
    let mut s = Screen::model(2);
    // 14 bits: 2000 > 1920 celdas
    let r = ew(&[0x11, 0x07, 0xD0]);
    assert_eq!(s.process(&r), Err(StreamError::BadAddress(2000)));
    assert_eq!(s.process(&ew(&[0x11, 0x40])), Err(StreamError::Truncated));
}

#[test]
fn registros_malformados_son_error_sin_panico() {
    let mut s = Screen::model(2);
    assert_eq!(s.process(&[]), Err(StreamError::Empty));
    assert_eq!(s.process(&[0x99]), Err(StreamError::UnknownCommand(0x99)));
    assert_eq!(
        s.process(&[0xF3, 0x00, 0x02, 0x01]),
        Err(StreamError::BadStructuredField)
    );
    assert_eq!(s.process(&[0xF3, 0x00]), Err(StreamError::Truncated));
    assert_eq!(s.process(&[0xF5]), Err(StreamError::Truncated));
}

#[test]
fn fuzz_sin_panico() {
    let mut s = Screen::model(4);
    for b in 0..=255u8 {
        let _ = s.process(&[b]);
        for x in (0..=255u8).step_by(7) {
            let _ = s.process(&[0xF5, 0xC3, b, x]);
            let _ = s.process(&[0xF3, 0x00, 0x05, 0x01, b, x]);
        }
    }
    // secuencia pseudoaleatoria determinista
    let mut seed: u32 = 0x2545_F491;
    for _ in 0..200 {
        let len = 1 + (seed % 48) as usize;
        let mut v = vec![0xF5, 0xC3];
        for _ in 0..len {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            v.push((seed >> 24) as u8);
        }
        let _ = s.process(&v);
    }
}

#[test]
fn erase_write_alternate_cambia_tamano() {
    let mut s = Screen::model(3);
    s.process(&[0x7E, 0xC3]).unwrap();
    assert_eq!((s.rows(), s.cols()), (32, 80));
    s.process(&[0xF5, 0xC3]).unwrap();
    assert_eq!((s.rows(), s.cols()), (24, 80));
}

#[test]
fn read_modified_incluye_solo_campos_con_mdt() {
    let mut s = Screen::model(2);
    let mut r = ew(&[0x1D, 0x00]);
    r.extend(sba(10));
    r.extend([0x1D, 0x00]);
    s.process(&r).unwrap();
    s.home();
    s.type_char('A').unwrap();
    s.type_char('B').unwrap();
    let out = s.read_modified(aid::ENTER, false);
    let mut esperado = vec![aid::ENTER];
    esperado.extend(addr12(3));
    esperado.push(0x11);
    esperado.extend(addr12(1));
    esperado.extend(ebc("AB"));
    assert_eq!(out, esperado);
    // Read Modified All también incluye el campo vacío de la posición 10
    let todo = s.read_modified(aid::ENTER, true);
    assert!(
        todo.windows(3)
            .any(|w| w == [0x11, addr12(11)[0], addr12(11)[1]])
    );
}

#[test]
fn entrada_respeta_protegidos_y_numericos() {
    let mut s = Screen::model(2);
    // protegido en 0, numérico en 10
    let mut r = ew(&[0x1D, 0x20]);
    r.extend(sba(10));
    r.extend([0x1D, 0x10]);
    s.process(&r).unwrap();
    s.home();
    assert_eq!(s.cursor(), 11);
    s.cursor = 0;
    assert_eq!(s.type_char('x'), Err(InputError::Protected));
    s.cursor = 11;
    assert_eq!(s.type_char('a'), Err(InputError::Numeric));
    s.type_char('7').unwrap();
    assert_eq!(s.cell(0, 11).unwrap().ch, from_char('7').unwrap());
}

#[test]
fn tabulador_entre_campos() {
    let mut s = Screen::model(2);
    let mut r = ew(&[0x1D, 0x00]);
    r.extend(sba(10));
    r.extend([0x1D, 0x00]);
    s.process(&r).unwrap();
    s.home();
    assert_eq!(s.cursor(), 1);
    s.tab();
    assert_eq!(s.cursor(), 11);
    s.tab();
    assert_eq!(s.cursor(), 1);
    s.backtab();
    assert_eq!(s.cursor(), 11);
}

#[test]
fn cliente_responde_read_partition_query() {
    let mut c = Client::new(&Tn3270Options::default());
    // WSF Read Partition (SFID 01), partición 0, QUERY (02), seguido de EOR
    let out = c.feed(&[0xF3, 0x00, 0x05, 0x01, 0x00, 0x02, IAC, EOR]);
    assert!(out.redraw);
    // resumen de QCODEs: SF de longitud 7, SFID 81, QCODE 80
    assert!(
        out.to_server
            .windows(4)
            .any(|w| w == [0x00, 0x07, 0x81, 0x80])
    );
    assert!(out.to_server.ends_with(&[IAC, EOR]));
}

#[test]
fn pa_envia_solo_el_aid() {
    let mut s = Screen::model(2);
    s.process(&ew(&[0x1D, 0x00])).unwrap();
    assert_eq!(s.press_aid(aid::PA1), vec![aid::PA1]);
    assert!(s.keyboard_locked());
}

#[test]
fn tecla_enter_envia_registro_con_eor() {
    let mut c = Client::new(&Tn3270Options::default());
    let out = c.feed(&[IAC, EOR]);
    assert!(out.to_server.is_empty());
    let out = c.feed(&[0xF5, 0xC3, 0x11, 0x40, 0x40, 0xC8, 0xC9, IAC, EOR]);
    assert!(out.to_server.is_empty());
    let out = c.key(Key::Aid(aid::ENTER));
    assert_eq!(out.to_server, [0x7D, 0x40, 0x40, 0xC8, 0xC9, IAC, EOR]);
    // con el teclado bloqueado la tecla se ignora
    let out = c.key(Key::Aid(aid::ENTER));
    assert!(out.to_server.is_empty());
}

#[tokio::test]
async fn negociacion_con_servidor_falso() {
    let (cli, mut srv) = tokio::io::duplex(8192);
    let opts = Tn3270Options::default();
    let mut s = Session::new(cli, &opts);

    let server = tokio::spawn(async move {
        // oferta del servidor: BINARY, EOR y TTYPE (DO) y su propia BINARY/EOR (WILL)
        srv.write_all(&[
            IAC, 253, 0, IAC, 253, 25, IAC, 253, 24, IAC, 251, 0, IAC, 251, 25,
        ])
        .await
        .unwrap();
        // respuestas: WILL BINARY, WILL EOR, WILL TTYPE, DO BINARY, DO EOR (en el orden recibido)
        let mut got = vec![0u8; 15];
        srv.read_exact(&mut got).await.unwrap();
        assert_eq!(
            got,
            [
                IAC, 251, 0, IAC, 251, 25, IAC, 251, 24, IAC, 253, 0, IAC, 253, 25
            ]
        );
        // SEND TERMINAL-TYPE -> el cliente responde IS con el tipo de terminal
        srv.write_all(&[IAC, 250, 24, 1, IAC, 240]).await.unwrap();
        let mut is = vec![0u8; 4 + 12 + 2];
        srv.read_exact(&mut is).await.unwrap();
        assert_eq!(&is[..4], &[IAC, 250, 24, 0]);
        assert_eq!(&is[4..16], b"IBM-3278-2-E");
        // pantalla: EW, WCC con restablecimiento de teclado, SBA 0, "HI", fin de registro
        srv.write_all(&[0xF5, 0xC3, 0x11, 0x40, 0x40, 0xC8, 0xC9, IAC, EOR])
            .await
            .unwrap();
        // ENTER del usuario: AID + SBA 0 + "HI" + EOR
        let mut rec = vec![0u8; 7];
        srv.read_exact(&mut rec).await.unwrap();
        assert_eq!(rec, [0x7D, 0x40, 0x40, 0xC8, 0xC9, IAC, EOR]);
    });

    // el cliente procesa bloques hasta que aparece la pantalla
    while s.client.screen.cell(0, 0).unwrap().ch == 0 {
        assert!(s.pump().await.unwrap());
    }
    assert!(s.client.telnet().is_binary_eor());
    assert_eq!(row(&s.client.screen, 0)[..2], *"HI");
    s.key(Key::Aid(aid::ENTER)).await.unwrap();
    server.await.unwrap();
}
