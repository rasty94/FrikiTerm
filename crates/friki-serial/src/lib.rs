//! Sesiones por puerto serie (TODO.md §4).

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use friki_core::config::{FlowControl, Parity, SerialConfig};
use friki_core::transport::{Stream, StreamEvent, StreamReader, StreamWriter};
use serialport::{DataBits, SerialPort, StopBits};
use tokio::sync::mpsc;

fn err(e: serialport::Error) -> io::Error {
    io::Error::other(e)
}

/// Puertos serie disponibles (nombre de dispositivo), ordenados.
pub fn list_ports() -> io::Result<Vec<String>> {
    let mut v: Vec<String> = serialport::available_ports()
        .map_err(err)?
        .into_iter()
        .map(|p| p.port_name)
        .collect();
    v.sort();
    Ok(v)
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_owned())
}

type Mapped = (
    DataBits,
    serialport::Parity,
    StopBits,
    serialport::FlowControl,
);

/// Traduce la configuración a los tipos de `serialport` (separado para probarlo sin hardware).
fn map_config(cfg: &SerialConfig) -> io::Result<Mapped> {
    let bits = match cfg.data_bits {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        8 => DataBits::Eight,
        _ => return Err(invalid("bits de datos: 5 a 8")),
    };
    let stop = match cfg.stop_bits {
        1 => StopBits::One,
        2 => StopBits::Two,
        _ => return Err(invalid("bits de parada: 1 o 2")),
    };
    let parity = match cfg.parity {
        Parity::None => serialport::Parity::None,
        Parity::Odd => serialport::Parity::Odd,
        Parity::Even => serialport::Parity::Even,
    };
    let flow = match cfg.flow {
        FlowControl::None => serialport::FlowControl::None,
        FlowControl::Software => serialport::FlowControl::Software,
        FlowControl::Hardware => serialport::FlowControl::Hardware,
    };
    Ok((bits, parity, stop, flow))
}

struct Writer(Arc<Mutex<Box<dyn SerialPort>>>);

#[async_trait::async_trait]
impl StreamWriter for Writer {
    async fn write(&self, data: &[u8]) -> io::Result<()> {
        let port = self.0.clone();
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut p = port
                .lock()
                .map_err(|_| io::Error::other("puerto envenenado"))?;
            p.write_all(&data)?;
            p.flush()
        })
        .await
        .map_err(io::Error::other)?
    }

    async fn resize(&self, _cols: u16, _rows: u16) -> io::Result<()> {
        Ok(())
    }
}

struct Reader(mpsc::Receiver<StreamEvent>);

#[async_trait::async_trait]
impl StreamReader for Reader {
    async fn next_event(&mut self) -> Option<StreamEvent> {
        self.0.recv().await
    }
}

/// Abre el puerto con la configuración dada. `resize` no hace nada. Desconectar el
/// dispositivo produce `StreamEvent::Lost`.
pub async fn open(cfg: &SerialConfig) -> io::Result<Stream> {
    let (bits, parity, stop, flow) = map_config(cfg)?;
    let (device, baud) = (cfg.device.clone(), cfg.baud);
    let (writer, mut reader) = tokio::task::spawn_blocking(move || {
        let port = serialport::new(device, baud)
            .data_bits(bits)
            .parity(parity)
            .stop_bits(stop)
            .flow_control(flow)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(err)?;
        let reader = port.try_clone().map_err(err)?;
        Ok::<_, io::Error>((port, reader))
    })
    .await
    .map_err(io::Error::other)??;

    let (tx, rx) = mpsc::channel(64);
    // Hilo propio: la lectura serie es bloqueante (con timeout para notar que se cerró el lector).
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let ev = match reader.read(&mut buf) {
                Ok(0) => StreamEvent::Lost("dispositivo cerrado".into()),
                Ok(n) => StreamEvent::Data(buf[..n].to_vec()),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                    ) =>
                {
                    if tx.is_closed() {
                        return;
                    }
                    continue;
                }
                Err(e) => StreamEvent::Lost(e.to_string()),
            };
            let end = matches!(ev, StreamEvent::Lost(_));
            if tx.blocking_send(ev).is_err() || end {
                return;
            }
        }
    });
    Ok((
        Arc::new(Writer(Arc::new(Mutex::new(writer)))),
        Box::new(Reader(rx)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_config() {
        let cfg = SerialConfig {
            data_bits: 7,
            parity: Parity::Even,
            stop_bits: 2,
            flow: FlowControl::Hardware,
            ..Default::default()
        };
        let (b, p, s, f) = map_config(&cfg).unwrap();
        assert_eq!(b, DataBits::Seven);
        assert_eq!(p, serialport::Parity::Even);
        assert_eq!(s, StopBits::Two);
        assert_eq!(f, serialport::FlowControl::Hardware);
    }

    #[test]
    fn rejects_bad_config() {
        for (d, s) in [(4, 1), (9, 1), (8, 0), (8, 3)] {
            let cfg = SerialConfig {
                data_bits: d,
                stop_bits: s,
                ..Default::default()
            };
            assert!(map_config(&cfg).is_err(), "{d} {s}");
        }
    }

    #[tokio::test]
    async fn missing_device_fails() {
        let cfg = SerialConfig {
            device: "/dev/friki-no-existe".into(),
            ..Default::default()
        };
        assert!(open(&cfg).await.is_err());
    }

    // Solo Linux: en macOS serialport fija la velocidad con IOSSIOSPEED, que un PTY rechaza (ENOTTY).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn pty_pair_roundtrip_and_lost() {
        use serialport::TTYPort;
        let (mut master, slave) = TTYPort::pair().unwrap();
        let cfg = SerialConfig {
            device: slave.name().unwrap(),
            ..Default::default()
        };
        let (w, mut r) = open(&cfg).await.unwrap();
        master.write_all(b"ping").unwrap();
        let mut got = Vec::new();
        while got.len() < 4 {
            match r.next_event().await {
                Some(StreamEvent::Data(d)) => got.extend(d),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(got, b"ping");
        w.write(b"pong").await.unwrap();
        let mut buf = [0u8; 4];
        master.set_timeout(Duration::from_secs(5)).unwrap();
        master.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"pong");
        w.resize(10, 10).await.unwrap();
        // Desaparece el dispositivo
        drop(master);
        drop(slave);
        let ev = tokio::time::timeout(Duration::from_secs(5), r.next_event()).await;
        assert!(matches!(ev, Ok(Some(StreamEvent::Lost(_)))), "{ev:?}");
    }
}
