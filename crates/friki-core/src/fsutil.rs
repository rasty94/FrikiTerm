//! Escritura atómica (fichero temporal en el mismo directorio + rename) y
//! bloqueo exclusivo entre procesos mediante un fichero `.lock` aparte.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Result;

/// Crea el directorio (y padres); en Unix con modo 0700.
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)?;
    Ok(())
}

fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    }
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let dir = parent_of(path);
    create_private_dir(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Ejecuta `f` con bloqueo exclusivo sobre `<path>.lock`. El bloqueo es sobre
/// un fichero aparte porque el rename sustituye el inodo del fichero de datos.
pub(crate) fn with_lock<T>(path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    create_private_dir(parent_of(path))?;
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(name))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write()?;
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn dirs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a").join("b").join("f");
        write_atomic(&p, b"x").unwrap();
        for dir in [d.path().join("a"), d.path().join("a").join("b")] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }
}
