//! Scripting con Rhai (TODO.md §7). La API imita la de SecureCRT (docs/12) en lo esencial:
//! `crt.Screen.Send`, `WaitForString(s)`, `ReadString`, `Get`, `crt.Session.Connect`, …
//! con timeouts obligatorios y sandbox: sin disco ni red salvo permiso explícito.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, Position, Scope};
use tokio::runtime::Handle;

#[cfg(test)]
mod tests;

/// Lo que el script puede hacer sobre la sesión. La TUI lo implementa para un panel.
#[async_trait::async_trait]
pub trait ScriptHost: Send + Sync {
    async fn send(&self, text: &str) -> Result<(), String>;
    /// Índice de la cadena encontrada, o `None` si vence el timeout.
    async fn wait_for_strings(
        &self,
        needles: &[String],
        timeout: Duration,
    ) -> Result<Option<usize>, String>;
    /// Texto recibido hasta (sin incluir) una de `until`; `None` si vence el timeout.
    async fn read_string(
        &self,
        until: &[String],
        timeout: Duration,
    ) -> Result<Option<String>, String>;
    /// Filas visibles como texto.
    fn screen_text(&self) -> Vec<String>;
    /// (fila, columna), base 1 como en SecureCRT.
    fn cursor(&self) -> (u16, u16);
    async fn connect(&self, target: &str) -> Result<(), String>;
    async fn disconnect(&self) -> Result<(), String>;
    fn connected(&self) -> bool;
    /// Mensaje para el usuario (barra de estado o diálogo).
    fn message(&self, text: &str);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    /// Directorios que el script puede leer.
    pub read: Vec<PathBuf>,
    /// Directorios donde puede escribir.
    pub write: Vec<PathBuf>,
    /// Puede abrir o cerrar conexiones.
    pub connect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_operations: u64,
    pub max_runtime: Duration,
    /// Timeout por defecto de las esperas si el script no da uno.
    pub default_wait: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_operations: 10_000_000,
            max_runtime: Duration::from_secs(3600),
            default_wait: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("compilación: {0}")]
    Compile(String),
    #[error("ejecución: {0}")]
    Runtime(String),
    #[error("permiso denegado: {0}")]
    Denied(String),
    #[error("límite superado: {0}")]
    Limit(String),
}

/// Tope de lectura/escritura de ficheros y de cadenas del script.
const MAX_BYTES: usize = 16 << 20;
/// Granularidad de las esperas para poder cancelar.
const CHUNK: Duration = Duration::from_millis(50);

#[derive(Clone)]
enum Signal {
    Quit(i64),
    Denied(String),
    Limit(String),
    Cancelled,
}

type R<T> = Result<T, Box<EvalAltResult>>;

/// Las señales usan `ErrorTerminated`: un `try/catch` del script no puede tragárselas.
fn sig(s: Signal) -> Box<EvalAltResult> {
    EvalAltResult::ErrorTerminated(Dynamic::from(s), Position::NONE).into()
}

fn rt(msg: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(msg.into()), Position::NONE).into()
}

struct Ctx {
    host: Arc<dyn ScriptHost>,
    handle: Handle,
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    connect: bool,
    limits: Limits,
    start: Instant,
    args: Vec<String>,
    cancel: CancelToken,
}

type Cx = Arc<Ctx>;

#[derive(Clone)]
struct Crt(Cx);
#[derive(Clone)]
struct Screen(Cx);
#[derive(Clone)]
struct Session(Cx);
#[derive(Clone)]
struct Dialog(Cx);
#[derive(Clone)]
struct Fs(Cx);

fn secs(d: &Dynamic) -> R<f64> {
    d.as_float()
        .ok()
        .or_else(|| d.as_int().ok().map(|i| i as f64))
        .filter(|v| v.is_finite())
        .ok_or_else(|| rt("el timeout debe ser un número de segundos"))
}

fn needles(d: &Dynamic) -> R<Vec<String>> {
    if let Ok(s) = d.clone().into_string() {
        return Ok(vec![s]);
    }
    let arr = d
        .clone()
        .into_array()
        .map_err(|_| rt("se esperaba una cadena o un array de cadenas"))?;
    arr.into_iter()
        .map(|x| {
            x.into_string()
                .map_err(|_| rt("el array solo admite cadenas"))
        })
        .collect()
}

impl Ctx {
    fn remaining(&self) -> R<Duration> {
        if self.cancel.is_cancelled() {
            return Err(sig(Signal::Cancelled));
        }
        self.limits
            .max_runtime
            .checked_sub(self.start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| sig(Signal::Limit("tiempo máximo de ejecución".into())))
    }

    /// Timeout efectivo: el dado (en segundos) o `default_wait`, nunca más del tiempo restante.
    fn wait_dur(&self, t: Option<&Dynamic>) -> R<Duration> {
        let rem = self.remaining()?;
        let d = match t {
            Some(t) => Duration::from_secs_f64(secs(t)?.clamp(0.0, 86_400.0)),
            None => self.limits.default_wait,
        };
        Ok(d.min(rem))
    }

    /// `block_on` en trozos de `CHUNK` comprobando la cancelación entre uno y otro.
    fn block<F: std::future::Future>(&self, f: F) -> R<F::Output> {
        let mut f = Box::pin(f);
        loop {
            self.remaining()?;
            if let Ok(v) = self.handle.block_on(tokio::time::timeout(CHUNK, &mut f)) {
                return Ok(v);
            }
        }
    }

    fn wait(&self, n: Vec<String>, t: Option<&Dynamic>) -> R<Option<usize>> {
        let d = self.wait_dur(t)?;
        self.block(self.host.wait_for_strings(&n, d))?.map_err(rt)
    }

    fn read(&self, n: Vec<String>, t: Option<&Dynamic>) -> R<String> {
        let d = self.wait_dur(t)?;
        let r = self.block(self.host.read_string(&n, d))?.map_err(rt)?;
        Ok(r.unwrap_or_default())
    }

    fn need_connect(&self) -> R<()> {
        if self.connect {
            Ok(())
        } else {
            Err(sig(Signal::Denied("conectar/desconectar".into())))
        }
    }

    /// Ruta canónica dentro de uno de los directorios permitidos (los symlinks que
    /// escapan quedan fuera porque se resuelven antes de comparar).
    fn resolve(&self, path: &str, write: bool) -> R<PathBuf> {
        let p = Path::new(path);
        let resolved = if std::fs::symlink_metadata(p).is_ok() {
            p.canonicalize()
        } else if write {
            let parent = match p.parent() {
                Some(d) if !d.as_os_str().is_empty() => d,
                _ => Path::new("."),
            };
            match p.file_name() {
                Some(n) => parent.canonicalize().map(|d| d.join(n)),
                None => return Err(rt("nombre de fichero no válido")),
            }
        } else {
            return Err(rt(format!("no existe: {path}")));
        }
        .map_err(|e| rt(format!("{path}: {e}")))?;
        if self.allowed(&resolved, write) {
            Ok(resolved)
        } else {
            Err(self.denied(path, write))
        }
    }

    fn allowed(&self, p: &Path, write: bool) -> bool {
        let dirs = if write { &self.write } else { &self.read };
        dirs.iter().any(|d| p.starts_with(d))
    }

    fn denied(&self, path: &str, write: bool) -> Box<EvalAltResult> {
        sig(Signal::Denied(format!(
            "{} {path}",
            if write { "escritura en" } else { "lectura de" }
        )))
    }

    /// Abre sin seguir symlinks finales ni bloquear en FIFOs, y valida sobre el
    /// descriptor (fichero regular y ruta real dentro de lo permitido) para cerrar el TOCTOU.
    /// No trunca: quien escribe lo hace después de validar.
    fn open_checked(&self, path: &str, write: bool) -> R<std::fs::File> {
        let p = self.resolve(path, write)?;
        let mut o = std::fs::OpenOptions::new();
        if write {
            o.write(true).create(true);
        } else {
            o.read(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let f = o.open(&p).map_err(|e| rt(format!("{path}: {e}")))?;
        let md = f.metadata().map_err(|e| rt(format!("{path}: {e}")))?;
        if !md.is_file() {
            return Err(rt(format!("{path}: no es un fichero regular")));
        }
        let real = fd_path(&f, &p).map_err(|e| rt(format!("{path}: {e}")))?;
        if self.allowed(&real, write) {
            Ok(f)
        } else {
            Err(self.denied(path, write))
        }
    }
}

/// Ruta real del descriptor. Sin forma portátil: ruta canónica del padre + nombre.
#[allow(unreachable_code)]
fn fd_path(f: &std::fs::File, p: &Path) -> std::io::Result<PathBuf> {
    let _ = p;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        return std::fs::read_link(format!("/proc/self/fd/{}", f.as_raw_fd()));
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: `buf` mide PATH_MAX, lo que F_GETPATH exige; el fd es válido mientras viva `f`.
        let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
        if r == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        return Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n])));
    }
    // ponytail: sin /proc ni F_GETPATH, padre canónico + nombre (queda una ventana mínima)
    let _ = f;
    match (p.parent(), p.file_name()) {
        (Some(d), Some(n)) => d.canonicalize().map(|d| d.join(n)),
        _ => Err(std::io::ErrorKind::InvalidInput.into()),
    }
}

fn canon(dirs: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter().filter_map(|d| d.canonicalize().ok()).collect()
}

fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<Crt>("Crt")
        .register_type_with_name::<Screen>("Screen")
        .register_type_with_name::<Session>("Session")
        .register_type_with_name::<Dialog>("Dialog")
        .register_type_with_name::<Fs>("FileSystem");
    engine
        .register_get("Screen", |c: &mut Crt| Screen(c.0.clone()))
        .register_get("Session", |c: &mut Crt| Session(c.0.clone()))
        .register_get("Dialog", |c: &mut Crt| Dialog(c.0.clone()))
        .register_get("FileSystem", |c: &mut Crt| Fs(c.0.clone()))
        .register_get("Arguments", |c: &mut Crt| -> rhai::Array {
            c.0.args.iter().map(|a| Dynamic::from(a.clone())).collect()
        })
        .register_fn("Sleep", |c: &mut Crt, ms: i64| -> R<()> {
            let end = Instant::now() + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
            loop {
                let rem = c.0.remaining()?;
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(());
                }
                std::thread::sleep(left.min(rem).min(CHUNK));
            }
        })
        .register_fn("Quit", |_: &mut Crt, n: i64| -> R<()> {
            Err(sig(Signal::Quit(n)))
        });

    // crt.Screen
    engine
        .register_fn("Send", |s: &mut Screen, t: &str| -> R<()> {
            s.0.remaining()?;
            s.0.block(s.0.host.send(t))?.map_err(rt)
        })
        .register_fn("WaitForString", |s: &mut Screen, n: &str| -> R<bool> {
            Ok(s.0.wait(vec![n.to_string()], None)?.is_some())
        })
        .register_fn(
            "WaitForString",
            |s: &mut Screen, n: &str, t: Dynamic| -> R<bool> {
                Ok(s.0.wait(vec![n.to_string()], Some(&t))?.is_some())
            },
        )
        .register_fn("WaitForStrings", |s: &mut Screen, n: Dynamic| -> R<i64> {
            Ok(s.0.wait(needles(&n)?, None)?.map_or(0, |i| i as i64 + 1))
        })
        .register_fn(
            "WaitForStrings",
            |s: &mut Screen, n: Dynamic, t: Dynamic| -> R<i64> {
                Ok(s.0
                    .wait(needles(&n)?, Some(&t))?
                    .map_or(0, |i| i as i64 + 1))
            },
        )
        .register_fn("ReadString", |s: &mut Screen, n: Dynamic| -> R<String> {
            s.0.read(needles(&n)?, None)
        })
        .register_fn(
            "ReadString",
            |s: &mut Screen, n: Dynamic, t: Dynamic| -> R<String> {
                s.0.read(needles(&n)?, Some(&t))
            },
        )
        .register_fn(
            "Get",
            |s: &mut Screen, r1: i64, c1: i64, r2: i64, c2: i64| -> String {
                let rows = s.0.host.screen_text();
                let clamp = |v: i64| usize::try_from(v.max(1)).unwrap_or(1);
                let (r1, c1, r2, c2) = (clamp(r1), clamp(c1), clamp(r2), clamp(c2));
                let mut out = Vec::new();
                for r in r1..=r2.min(r1 + 10_000) {
                    let line = rows.get(r - 1).map_or("", String::as_str);
                    out.push(
                        line.chars()
                            .skip(c1 - 1)
                            .take((c2 + 1).saturating_sub(c1))
                            .collect::<String>(),
                    );
                }
                out.join("\n")
            },
        )
        .register_get("CurrentRow", |s: &mut Screen| {
            i64::from(s.0.host.cursor().0)
        })
        .register_get("CurrentColumn", |s: &mut Screen| {
            i64::from(s.0.host.cursor().1)
        });

    // crt.Session
    engine
        .register_fn("Connect", |s: &mut Session, t: &str| -> R<()> {
            s.0.need_connect()?;
            s.0.block(s.0.host.connect(t))?.map_err(rt)
        })
        .register_fn("Disconnect", |s: &mut Session| -> R<()> {
            s.0.need_connect()?;
            s.0.block(s.0.host.disconnect())?.map_err(rt)
        })
        .register_get("Connected", |s: &mut Session| s.0.host.connected());

    // crt.Dialog
    engine.register_fn("MessageBox", |d: &mut Dialog, t: &str| {
        d.0.host.message(t);
    });

    // crt.FileSystem
    engine
        .register_fn("Read", |f: &mut Fs, path: &str| -> R<String> {
            use std::io::Read;
            let file = f.0.open_checked(path, false)?;
            let mut buf = Vec::new();
            file.take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut buf)
                .map_err(|e| rt(format!("{path}: {e}")))?;
            if buf.len() > MAX_BYTES {
                return Err(rt("fichero demasiado grande"));
            }
            String::from_utf8(buf).map_err(|_| rt(format!("{path}: no es UTF-8")))
        })
        .register_fn("Write", |f: &mut Fs, path: &str, text: &str| -> R<()> {
            use std::io::Write;
            if text.len() > MAX_BYTES {
                return Err(rt("texto demasiado grande"));
            }
            let mut file = f.0.open_checked(path, true)?;
            file.set_len(0)
                .and_then(|()| file.write_all(text.as_bytes()))
                .map_err(|e| rt(format!("{path}: {e}")))
        });
}

fn map_err(e: EvalAltResult) -> Result<i64, ScriptError> {
    match e {
        EvalAltResult::ErrorTerminated(t, _) => match t.try_cast::<Signal>() {
            Some(Signal::Quit(n)) => Ok(n),
            Some(Signal::Denied(m)) => Err(ScriptError::Denied(m)),
            Some(Signal::Limit(m)) => Err(ScriptError::Limit(m)),
            Some(Signal::Cancelled) => Err(ScriptError::Runtime("cancelado".into())),
            None => Err(ScriptError::Limit("script terminado".into())),
        },
        EvalAltResult::ErrorRuntime(d, pos) => Err(ScriptError::Runtime(match pos.line() {
            Some(l) => format!("{d} (línea {l})"),
            None => d.to_string(),
        })),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _) => map_err(*inner),
        other => Err(ScriptError::Runtime(other.to_string())),
    }
}

fn run_blocking(
    source: &str,
    args: Vec<String>,
    host: Arc<dyn ScriptHost>,
    perms: &Permissions,
    limits: Limits,
    handle: Handle,
    cancel: CancelToken,
) -> Result<i64, ScriptError> {
    let start = Instant::now();
    let mut engine = Engine::new();
    engine.disable_symbol("eval");
    engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
    engine.set_max_string_size(MAX_BYTES);
    engine.set_max_array_size(1_000_000);
    engine.set_max_map_size(100_000);
    let (max_ops, max_rt) = (limits.max_operations, limits.max_runtime);
    let cancel_p = cancel.clone();
    engine.on_progress(move |ops| {
        if cancel_p.is_cancelled() {
            Some(Dynamic::from(Signal::Cancelled))
        } else if ops > max_ops {
            Some(Dynamic::from(Signal::Limit("operaciones máximas".into())))
        } else if start.elapsed() > max_rt {
            Some(Dynamic::from(Signal::Limit(
                "tiempo máximo de ejecución".into(),
            )))
        } else {
            None
        }
    });
    let h = host.clone();
    engine.on_print(move |s| h.message(s));
    engine.on_debug(|_, _, _| {});
    register(&mut engine);

    let ast = engine
        .compile(source)
        .map_err(|e| ScriptError::Compile(format!("línea {}: {}", e.1.line().unwrap_or(0), e.0)))?;
    let ctx = Arc::new(Ctx {
        host,
        handle,
        read: canon(&perms.read),
        write: canon(&perms.write),
        connect: perms.connect,
        limits,
        start,
        args,
        cancel,
    });
    let mut scope = Scope::new();
    scope.push_constant("crt", Crt(ctx));
    match engine.run_ast_with_scope(&mut scope, &ast) {
        Ok(()) => Ok(0),
        Err(e) => map_err(*e),
    }
}

/// Cancelación cooperativa de un script: la comprueban `crt.Sleep`, las esperas del host y
/// el contador de operaciones; un script cancelado termina con `ScriptError::Runtime("cancelado")`.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// `run` con cancelación. El hilo del motor termina en <200 ms tras `cancel()`.
pub async fn run_cancellable(
    source: &str,
    args: &[String],
    host: Arc<dyn ScriptHost>,
    perms: &Permissions,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<i64, ScriptError> {
    let handle = Handle::current();
    let (source, args, perms, limits, cancel) = (
        source.to_string(),
        args.to_vec(),
        perms.clone(),
        limits.clone(),
        cancel.clone(),
    );
    // Rhai es síncrono: el motor corre en un hilo bloqueante y llama al host con `block_on`.
    tokio::task::spawn_blocking(move || {
        run_blocking(&source, args, host, &perms, limits, handle, cancel)
    })
    .await
    .map_err(|e| ScriptError::Runtime(format!("tarea del script: {e}")))?
}

/// Ejecuta el script con `args` (`crt.Arguments`). Devuelve el valor de `crt.Quit(n)` o 0.
pub async fn run(
    source: &str,
    args: &[String],
    host: Arc<dyn ScriptHost>,
    perms: &Permissions,
    limits: &Limits,
) -> Result<i64, ScriptError> {
    run_cancellable(source, args, host, perms, limits, &CancelToken::new()).await
}
