//! Sincronización de carpetas: comparación y plan de acciones con filtros (TODO.md §5).

use std::collections::{BTreeMap, BTreeSet};

use crate::{Entry, Error, Kind, RemoteFs, Result, join, safe_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Local → remoto: sube lo nuevo o distinto.
    Upload,
    /// Remoto → local.
    Download,
    /// Ambos sentidos: gana la fecha más reciente; con caché y `delete` propaga borrados.
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filters {
    /// Patrones glob (`*.log`, `node_modules/`, `**/tmp/*`) relativos a la raíz.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOptions {
    pub mode: SyncMode,
    /// Borrar en el destino lo que no existe en el origen; en `Both` solo propaga los
    /// borrados detectados con la caché.
    pub delete: bool,
    pub filters: Filters,
    /// Comparar también por tamaño (siempre) y fecha con esta tolerancia en segundos.
    pub mtime_tolerance: i64,
    /// Máximo de borrados que puede contener el plan; si lo supera, `plan` falla.
    pub max_deletes: Option<usize>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            mode: SyncMode::Upload,
            delete: false,
            filters: Filters::default(),
            mtime_tolerance: 2,
            max_deletes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Upload {
        path: String,
    },
    Download {
        path: String,
    },
    MkdirRemote {
        path: String,
    },
    MkdirLocal {
        path: String,
    },
    DeleteRemote {
        path: String,
    },
    DeleteLocal {
        path: String,
    },
    /// Ambos cambiaron desde la última sincronización (solo con `Both` y caché).
    Conflict {
        path: String,
    },
}

/// Clave de la caché para un par de raíces; el llamador guarda con ella el estado tras
/// sincronizar (`DirCache::store`), p. ej. las entradas de ficheros de un lado.
pub fn cache_key(
    local: &dyn RemoteFs,
    local_root: &str,
    remote: &dyn RemoteFs,
    remote_root: &str,
) -> String {
    format!(
        "{}{}|{}{}",
        local.label(),
        local_root,
        remote.label(),
        remote_root
    )
}

fn action_path(a: &Action) -> &str {
    match a {
        Action::Upload { path }
        | Action::Download { path }
        | Action::MkdirRemote { path }
        | Action::MkdirLocal { path }
        | Action::DeleteRemote { path }
        | Action::DeleteLocal { path }
        | Action::Conflict { path } => path,
    }
}

/// Estado actual de `root` (ficheros y directorios, sin filtros) para guardarlo en la caché
/// tras una sincronización `Both` correcta. Una raíz inexistente da un estado vacío.
pub async fn snapshot(fs: &dyn RemoteFs, root: &str) -> Result<Vec<Entry>> {
    let f = Filter {
        inc: vec![],
        exc: vec![],
    };
    Ok(walk(fs, root, &f, true).await?.into_values().collect())
}

struct Pat {
    glob: globset::GlobMatcher,
    dir_only: bool,
    base_only: bool,
}

/// Patrones: `x/` solo directorios; sin `/` interior se compara también con el nombre base.
fn compile(pats: &[String]) -> Result<Vec<Pat>> {
    pats.iter()
        .map(|p| {
            let dir_only = p.ends_with('/');
            let t = p.trim_end_matches('/').trim_start_matches('/');
            let glob = globset::Glob::new(t)
                .map_err(|e| Error::Protocol(format!("patrón `{p}`: {e}")))?
                .compile_matcher();
            Ok(Pat {
                glob,
                dir_only,
                base_only: !t.contains('/'),
            })
        })
        .collect()
}

fn hit(pats: &[Pat], rel: &str, is_dir: bool) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    pats.iter().any(|p| {
        (!p.dir_only || is_dir) && (p.glob.is_match(rel) || p.base_only && p.glob.is_match(base))
    })
}

struct Filter {
    inc: Vec<Pat>,
    exc: Vec<Pat>,
}

impl Filter {
    fn excluded(&self, rel: &str, is_dir: bool) -> bool {
        hit(&self.exc, rel, is_dir)
    }
    /// `include` solo filtra ficheros; también vale que un directorio ascendente coincida.
    fn included(&self, rel: &str) -> bool {
        if self.inc.is_empty() || hit(&self.inc, rel, false) {
            return true;
        }
        let mut p = rel;
        while let Some((parent, _)) = p.rsplit_once('/') {
            if hit(&self.inc, parent, true) {
                return true;
            }
            p = parent;
        }
        false
    }
}

type Tree = BTreeMap<String, Entry>;

/// Recorre `root` (solo ficheros y directorios; nombres inseguros y excluidos se ignoran).
/// Una raíz inexistente solo se tolera con `missing_ok` (la de destino): la de origen
/// inexistente no es un árbol vacío, y con `delete` borraría todo el destino.
async fn walk(fs: &dyn RemoteFs, root: &str, f: &Filter, missing_ok: bool) -> Result<Tree> {
    let mut out = Tree::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let dir = if rel.is_empty() {
            root.to_string()
        } else {
            join(root, &rel)
        };
        let list = match fs.list(&dir).await {
            Err(Error::NotFound(_)) if rel.is_empty() && missing_ok => continue,
            r => r?,
        };
        for mut e in list {
            if !safe_name(&e.name) || !matches!(e.kind, Kind::File | Kind::Dir) {
                continue;
            }
            let p = if rel.is_empty() {
                e.name.clone()
            } else {
                format!("{rel}/{}", e.name)
            };
            let is_dir = e.kind == Kind::Dir;
            if f.excluded(&p, is_dir) || (!is_dir && !f.included(&p)) {
                continue;
            }
            if is_dir {
                stack.push(p.clone());
            }
            e.path = p.clone();
            out.insert(p, e);
        }
    }
    Ok(out)
}

fn differs(a: &Entry, b: &Entry, tol: i64) -> bool {
    a.size != b.size || matches!((a.mtime, b.mtime), (Some(x), Some(y)) if (x - y).abs() > tol)
}

fn is_dir(e: &Entry) -> bool {
    e.kind == Kind::Dir
}

/// Compara `local_root` y `remote_root` y devuelve el plan sin ejecutar nada.
/// Rutas del plan relativas a las raíces. Nombres remotos no seguros se ignoran.
///
/// Orden aplicable: creaciones y copias en orden de ruta (un directorio antes que su
/// contenido), y al final los borrados (ficheros, luego directorios del más profundo al
/// más superficial). Un mismo nombre que es fichero en un lado y directorio en el otro
/// se informa como `Conflict`. En `Both` con caché, la caché debe contener el estado de
/// ficheros de la última sincronización (clave: [`cache_key`]).
pub async fn plan(
    local: &dyn RemoteFs,
    local_root: &str,
    remote: &dyn RemoteFs,
    remote_root: &str,
    opts: &SyncOptions,
    cache: Option<&crate::cache::DirCache>,
) -> Result<Vec<Action>> {
    let f = Filter {
        inc: compile(&opts.filters.include)?,
        exc: compile(&opts.filters.exclude)?,
    };
    // En `Both` no hay origen único ni borrados: se toleran las dos.
    let l = walk(local, local_root, &f, opts.mode != SyncMode::Upload).await?;
    let r = walk(remote, remote_root, &f, opts.mode != SyncMode::Download).await?;
    let tol = opts.mtime_tolerance;
    let mut acts = Vec::new();
    let mut dels: Vec<(bool, Action)> = Vec::new(); // (es directorio, acción)

    // `up`: el origen es el lado local.
    let up_act = |up: bool, path: String, dir: bool| match (up, dir) {
        (true, false) => Action::Upload { path },
        (false, false) => Action::Download { path },
        (true, true) => Action::MkdirRemote { path },
        (false, true) => Action::MkdirLocal { path },
    };

    if opts.mode == SyncMode::Both {
        let last: Tree = match cache {
            Some(c) => c
                .load(&cache_key(local, local_root, remote, remote_root))?
                .into_iter()
                .map(|e| (e.path.clone(), e))
                .collect(),
            None => Tree::new(),
        };
        // ponytail: como unison, se aborta antes que propagar borrados de un árbol vacío (raíz
        // desmontada o inexistente); sin caché no hay borrados que temer.
        if !last.is_empty() && (l.is_empty() || r.is_empty()) {
            let lado = if l.is_empty() { "local" } else { "remota" };
            return Err(Error::UnsafeSync(format!(
                "la raíz {lado} no existe o está vacía y la última sincronización no lo estaba"
            )));
        }
        let paths: BTreeSet<&String> = l.keys().chain(r.keys()).collect();
        for p in paths {
            let path = p.clone();
            match (l.get(p), r.get(p)) {
                // Con caché y `delete`: lo que estaba en la última sincronización y sigue
                // sin cambios en un lado se borró en el otro; si cambió, se conserva.
                (Some(a), None) => match last.get(p) {
                    Some(c) if opts.delete && (is_dir(a) || !differs(a, c, tol)) => {
                        dels.push((is_dir(a), Action::DeleteLocal { path }))
                    }
                    _ => acts.push(up_act(true, path, is_dir(a))),
                },
                (None, Some(b)) => match last.get(p) {
                    Some(c) if opts.delete && (is_dir(b) || !differs(b, c, tol)) => {
                        dels.push((is_dir(b), Action::DeleteRemote { path }))
                    }
                    _ => acts.push(up_act(false, path, is_dir(b))),
                },
                (Some(a), Some(b)) if is_dir(a) != is_dir(b) => {
                    acts.push(Action::Conflict { path })
                }
                (Some(a), Some(_)) if is_dir(a) => {}
                (Some(a), Some(b)) => {
                    if !differs(a, b, tol) {
                        continue;
                    }
                    let (lc, rc) = match last.get(p) {
                        Some(c) => (differs(a, c, tol), differs(b, c, tol)),
                        None => (true, true),
                    };
                    acts.push(match (lc, rc) {
                        (true, false) => Action::Upload { path },
                        (false, true) => Action::Download { path },
                        _ if last.contains_key(p) && lc && rc => Action::Conflict { path },
                        _ => match (a.mtime, b.mtime) {
                            (Some(x), Some(y)) if x > y => Action::Upload { path },
                            (Some(x), Some(y)) if y > x => Action::Download { path },
                            _ => Action::Conflict { path },
                        },
                    });
                }
                (None, None) => {}
            }
        }
        // Un directorio borrado que aún tiene contenido modificado se recrea en vez de borrarse.
        let mut i = 0;
        while i < dels.len() {
            let (d, a) = &dels[i];
            let (Action::DeleteLocal { path } | Action::DeleteRemote { path }) = a else {
                unreachable!()
            };
            let pre = format!("{path}/");
            let at = acts.iter().position(|x| action_path(x).starts_with(&pre));
            match at {
                Some(at) if *d => {
                    let mk = up_act(matches!(a, Action::DeleteLocal { .. }), path.clone(), true);
                    acts.insert(at, mk);
                    dels.remove(i);
                }
                _ => i += 1,
            }
        }
    } else {
        let up = opts.mode == SyncMode::Upload;
        let (src, dst) = if up { (&l, &r) } else { (&r, &l) };
        for (p, s) in src {
            let path = p.clone();
            match dst.get(p) {
                None => acts.push(up_act(up, path, is_dir(s))),
                Some(d) if is_dir(d) != is_dir(s) => acts.push(Action::Conflict { path }),
                Some(d) if !is_dir(s) && differs(s, d, tol) => acts.push(up_act(up, path, false)),
                Some(_) => {}
            }
        }
        if opts.delete {
            for (p, d) in dst.iter().filter(|(p, _)| !src.contains_key(*p)) {
                let path = p.clone();
                let a = match up {
                    true => Action::DeleteRemote { path },
                    false => Action::DeleteLocal { path },
                };
                dels.push((is_dir(d), a));
            }
        }
    }
    if let Some(max) = opts.max_deletes
        && dels.len() > max
    {
        return Err(Error::Protocol(format!(
            "el plan borra {} elementos, por encima del máximo permitido ({max})",
            dels.len()
        )));
    }
    // Ficheros primero; directorios después, del más profundo al más superficial.
    acts.extend(dels.iter().filter(|(d, _)| !d).map(|(_, a)| a.clone()));
    acts.extend(dels.into_iter().rev().filter(|(d, _)| *d).map(|(_, a)| a));
    Ok(acts)
}
