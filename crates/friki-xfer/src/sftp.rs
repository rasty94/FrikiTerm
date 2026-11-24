//! SFTP sobre `friki_ssh::Connection::open_subsystem("sftp")`.

use crate::{RemoteFs, Result};

pub async fn open(conn: &friki_ssh::Connection) -> Result<Box<dyn RemoteFs>> {
    let _ = conn;
    todo!("friki-xfer: sftp::open")
}
