//! Caché de listados para la sincronización (SQLite, tablas `config` y `direntries`, como
//! hace SecureFX según docs/13). Guarda el estado de la última sincronización para detectar
//! conflictos en modo `Both`.
//!
//! Esquema: `config(id,name,value)` y `direntries(id,parentid,name,mtime,size,attrs)` de
//! SecureFX, más `roots(id,key)`. `parentid` apunta a `roots.id` (lista plana por clave),
//! `name` guarda la ruta completa y `attrs` es `"<kind>;<mode>"`.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, params};

use crate::{Entry, Error, Kind, Result};

pub struct DirCache {
    conn: Mutex<Connection>,
}

fn db(e: rusqlite::Error) -> Error {
    Error::Io(std::io::Error::other(e))
}

fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::File => "f",
        Kind::Dir => "d",
        Kind::Symlink => "l",
        Kind::Other => "o",
    }
}

fn kind_of(s: &str) -> Kind {
    match s {
        "f" => Kind::File,
        "d" => Kind::Dir,
        "l" => Kind::Symlink,
        _ => Kind::Other,
    }
}

impl DirCache {
    /// Crea o abre la base de datos (permisos 0600 en Unix).
    pub fn open(path: &Path) -> Result<Self> {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        let conn = Connection::open(path).map_err(db)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS config(id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, value TEXT);
             CREATE TABLE IF NOT EXISTS roots(id INTEGER PRIMARY KEY, key TEXT UNIQUE NOT NULL);
             CREATE TABLE IF NOT EXISTS direntries(
                 id INTEGER PRIMARY KEY, parentid INTEGER NOT NULL, name TEXT NOT NULL,
                 mtime INTEGER, size INTEGER NOT NULL, attrs TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS direntries_parent ON direntries(parentid);",
        )
        .map_err(db)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| Error::Protocol("caché envenenada".into()))
    }

    /// Estado guardado de `root` (clave: etiqueta del fs + raíz).
    pub fn load(&self, key: &str) -> Result<Vec<Entry>> {
        let conn = self.conn()?;
        let mut st = conn
            .prepare(
                "SELECT d.name, d.mtime, d.size, d.attrs FROM direntries d
                 JOIN roots r ON r.id = d.parentid WHERE r.key = ?1 ORDER BY d.name",
            )
            .map_err(db)?;
        let rows = st
            .query_map(params![key], |r| {
                let path: String = r.get(0)?;
                let attrs: String = r.get(3)?;
                let (k, mode) = attrs.split_once(';').unwrap_or((&attrs, ""));
                Ok(Entry {
                    name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                    path,
                    kind: kind_of(k),
                    size: r.get::<_, i64>(2)? as u64,
                    mtime: r.get(1)?,
                    mode: mode.parse().ok(),
                })
            })
            .map_err(db)?;
        rows.collect::<std::result::Result<_, _>>().map_err(db)
    }

    /// Sustituye el estado guardado de `key` (en una transacción).
    pub fn store(&self, key: &str, entries: &[Entry]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(db)?;
        tx.execute("INSERT OR IGNORE INTO roots(key) VALUES(?1)", params![key])
            .map_err(db)?;
        let id: i64 = tx
            .query_row("SELECT id FROM roots WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .map_err(db)?;
        tx.execute("DELETE FROM direntries WHERE parentid = ?1", params![id])
            .map_err(db)?;
        {
            let mut ins = tx
                .prepare(
                    "INSERT INTO direntries(parentid,name,mtime,size,attrs) VALUES(?1,?2,?3,?4,?5)",
                )
                .map_err(db)?;
            for e in entries {
                let attrs = format!(
                    "{};{}",
                    kind_str(e.kind),
                    e.mode.map(|m| m.to_string()).unwrap_or_default()
                );
                ins.execute(params![id, e.path, e.mtime, e.size as i64, attrs])
                    .map_err(db)?;
            }
        }
        tx.commit().map_err(db)
    }
}
