//! DTO del contrato Rust <-> UI. Espejo exacto de `src/lib/types.ts` (solo T0 lo toca).
//! Los tipos de friki-core (`SessionConfig`, `Forward`) conservan su serde nativo.

use friki_core::config::{Forward, SessionConfig};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{CmdError, ErrorKind};

pub type TermId = u64;
pub type ConnId = u64;
pub type SftpId = u64;
pub type PromptId = u64;
pub type JobId = u64;
pub type ForwardId = u64;

// ---- Prompts y eventos globales ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KbdPrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Prompt {
    HostKeyUnknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        previous_fingerprint: String,
    },
    Password {
        user: String,
        host: String,
        port: u16,
    },
    Passphrase {
        key_path: String,
    },
    KbdInteractive {
        host: String,
        name: String,
        instructions: String,
        prompts: Vec<KbdPrompt>,
    },
}

/// Tipo de respuesta que espera un prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    HostKey,
    Secret,
    Multi,
}

impl Prompt {
    pub fn kind(&self) -> PromptKind {
        match self {
            Prompt::HostKeyUnknown { .. } | Prompt::HostKeyChanged { .. } => PromptKind::HostKey,
            Prompt::Password { .. } | Prompt::Passphrase { .. } => PromptKind::Secret,
            Prompt::KbdInteractive { .. } => PromptKind::Multi,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKeyDecision {
    Reject,
    AcceptOnce,
    AcceptAndSave,
}

/// Los secretos se deserializan directamente a `Zeroizing`.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PromptAnswer {
    HostKey {
        decision: HostKeyDecision,
    },
    Secret {
        value: Option<Zeroizing<String>>,
        remember: bool,
    },
    Multi {
        values: Option<Vec<Zeroizing<String>>>,
    },
}

impl PromptAnswer {
    pub fn kind(&self) -> PromptKind {
        match self {
            PromptAnswer::HostKey { .. } => PromptKind::HostKey,
            PromptAnswer::Secret { .. } => PromptKind::Secret,
            PromptAnswer::Multi { .. } => PromptKind::Multi,
        }
    }
}

/// Refleja `friki_xfer::queue::QueueEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TransferEvent {
    Started {
        id: JobId,
        file: String,
        total: Option<u64>,
    },
    Progress {
        id: JobId,
        file: String,
        done: u64,
    },
    Retrying {
        id: JobId,
        attempt: u32,
        error: String,
    },
    Finished {
        id: JobId,
        bytes: u64,
    },
    Failed {
        id: JobId,
        error: String,
        error_kind: ErrorKind,
    },
    Cancelled {
        id: JobId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardStatus {
    pub id: ForwardId,
    pub spec: Forward,
    pub listen: String,
    pub active: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AppEvent {
    Prompt {
        id: PromptId,
        prompt: Prompt,
    },
    Transfer {
        sftp: SftpId,
        ev: TransferEvent,
    },
    Forward {
        conn: ConnId,
        status: ForwardStatus,
    },
    ConnClosed {
        conn: ConnId,
        reason: Option<String>,
    },
    /// El prompt ya no espera respuesta (plazo vencido o cancelado): la UI debe cerrar su modal.
    PromptDone {
        id: PromptId,
    },
}

// ---- Terminal ----

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TermTarget {
    Saved { name: String },
    Quick { spec: Box<SessionConfig> },
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TermEvent {
    Opened {
        conn: Option<ConnId>,
        warnings: Vec<String>,
        negotiated: Option<String>,
        forwards: Vec<ForwardStatus>,
    },
    Failed {
        error: CmdError,
    },
    Exit {
        code: Option<i32>,
    },
    Closed,
    Lost {
        reason: Option<String>,
    },
}

// ---- Hosts ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub added: u32,
    pub warnings: Vec<String>,
}

// ---- SFTP y transferencias ----

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FsRef {
    Local { root: String },
    Remote { sftp: SftpId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryDto {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime: Option<i64>,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XferJob {
    pub direction: Direction,
    pub src: String,
    pub dst: String,
    pub recursive: bool,
}

// ---- Llavero y snippets ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInfo {
    pub path: String,
    pub algorithm: String,
    pub comment: String,
    pub fingerprint: String,
    pub encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub name: String,
    pub command: String,
}

// ---- Puertos serie ----

/// Puerto serie detectado (`serial_ports`). `description` es orientativa (USB, Bluetooth...) y puede ir vacía.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialPortInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RightClick {
    Paste,
    Menu,
}

/// Preferencias de la app de escritorio (`config_dir/desktop.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSettings {
    pub theme: ThemePref,
    pub font_family: String,
    pub font_size: u8,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub scrollback: u32,
    pub copy_on_select: bool,
    pub right_click: RightClick,
    pub confirm_close: bool,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            font_family: String::new(),
            font_size: 13,
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            scrollback: 10_000,
            copy_on_select: false,
            right_click: RightClick::Menu,
            confirm_close: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formas_json_del_contrato() {
        let ev = AppEvent::Prompt {
            id: 7,
            prompt: Prompt::HostKeyChanged {
                host: "h".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint: "a".into(),
                previous_fingerprint: "b".into(),
            },
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"type":"prompt","id":7,"prompt":{"kind":"hostKeyChanged","host":"h","port":22,"algorithm":"ssh-ed25519","fingerprint":"a","previousFingerprint":"b"}}"#
        );
        let t = TransferEvent::Failed {
            id: 1,
            error: "x".into(),
            error_kind: ErrorKind::Network,
        };
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"type":"failed","id":1,"error":"x","errorKind":"network"}"#
        );
        let a: PromptAnswer =
            serde_json::from_str(r#"{"kind":"secret","value":"pw","remember":true}"#).unwrap();
        assert_eq!(a.kind(), PromptKind::Secret);
        let a: PromptAnswer =
            serde_json::from_str(r#"{"kind":"hostKey","decision":"acceptAndSave"}"#).unwrap();
        assert_eq!(a.kind(), PromptKind::HostKey);
        let t: TermTarget = serde_json::from_str(r#"{"kind":"local"}"#).unwrap();
        assert_eq!(t, TermTarget::Local);
        let f: FsRef = serde_json::from_str(r#"{"kind":"local","root":"/"}"#).unwrap();
        assert!(matches!(f, FsRef::Local { .. }));
    }
}
