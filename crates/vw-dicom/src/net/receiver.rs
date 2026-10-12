//! 確認用の小さな受け手（Storage SCP）。送信のテストと、手元で送信を試すための
//! `examples/store_scp.rs` が使う。病院の PACS の代わりになるものではない。

use std::net::TcpStream;
use std::path::{Path, PathBuf};

use dicom_dictionary_std::tags;
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use dicom_ul::association::server::{ServerAssociation, ServerAssociationOptions};
use dicom_ul::pdu::{PDataValueType, Pdu};

use super::{
    command, transfer_syntax, trim_uid, uid_element, us_element, C_ECHO_RQ, C_STORE_RQ,
    EXPLICIT_VR_LE, IMPLICIT_VR_LE, NO_DATA_SET,
};

const C_STORE_RSP: u16 = 0x8001;
const C_ECHO_RSP: u16 = 0x8030;

/// 受け手の振る舞い。
#[derive(Debug, Clone)]
pub struct Receiver {
    pub ae_title: String,
    /// 圧縮の転送構文も受ける。`false` なら非圧縮の 2 つだけを受ける。
    pub accept_compressed: bool,
    /// 受け取ったファイルを置くフォルダ。
    pub dir: PathBuf,
    /// C-STORE に返すステータス。0 なら成功。
    pub store_status: u16,
}

/// 1 つのアソシエーションで受け取ったもの。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Received {
    pub echoes: usize,
    pub files: Vec<PathBuf>,
    /// 受け取ったファイルの転送構文（`files` と同じ順）。
    pub transfer_syntaxes: Vec<String>,
}

impl Receiver {
    /// 1 つのアソシエーションを受けて、相手が閉じるまで処理する。
    pub fn serve(&self, stream: TcpStream) -> Result<Received, String> {
        let mut options = ServerAssociationOptions::new()
            .accept_called_ae_title()
            .ae_title(self.ae_title.as_str())
            .promiscuous(true);
        if !self.accept_compressed {
            options = options
                .with_transfer_syntax(EXPLICIT_VR_LE)
                .with_transfer_syntax(IMPLICIT_VR_LE);
        }
        let _ = stream.set_nodelay(true);
        let mut assoc = options.establish(stream).map_err(|e| e.to_string())?;
        let mut out = Received::default();
        loop {
            let (pc, request) = match read_message(&mut assoc, PDataValueType::Command)? {
                Message::Data(pc, bytes) => (pc, parse_command(&bytes)?),
                Message::Released => return Ok(out),
            };
            let field = int(&request, tags::COMMAND_FIELD)?;
            let message = int(&request, tags::MESSAGE_ID)?;
            let class = text(&request, tags::AFFECTED_SOP_CLASS_UID)?;
            match field {
                C_ECHO_RQ => {
                    out.echoes += 1;
                    let rsp = response(C_ECHO_RSP, message, &class, None, 0);
                    reply(&mut assoc, pc, &rsp)?;
                }
                C_STORE_RQ => {
                    let instance = text(&request, tags::AFFECTED_SOP_INSTANCE_UID)?;
                    let Message::Data(_, data) = read_message(&mut assoc, PDataValueType::Data)?
                    else {
                        return Err("released in the middle of a C-STORE".into());
                    };
                    let ts = assoc
                        .presentation_contexts()
                        .iter()
                        .find(|c| c.id == pc)
                        .map(|c| trim_uid(&c.transfer_syntax).to_owned())
                        .ok_or("unknown presentation context")?;
                    if self.store_status == 0 {
                        let path = self.dir.join(format!("{instance}.dcm"));
                        save(&path, &data, &class, &instance, &ts)?;
                        out.files.push(path);
                        out.transfer_syntaxes.push(ts);
                    }
                    let rsp = response(
                        C_STORE_RSP,
                        message,
                        &class,
                        Some(&instance),
                        self.store_status,
                    );
                    reply(&mut assoc, pc, &rsp)?;
                }
                other => return Err(format!("unsupported command {other:04X}H")),
            }
        }
    }
}

enum Message {
    Data(u8, Vec<u8>),
    Released,
}

fn read_message(
    assoc: &mut ServerAssociation<TcpStream>,
    kind: PDataValueType,
) -> Result<Message, String> {
    let mut bytes = Vec::new();
    loop {
        match assoc.receive().map_err(|e| e.to_string())? {
            Pdu::PData { data } => {
                for value in data {
                    if value.value_type != kind {
                        continue;
                    }
                    bytes.extend_from_slice(&value.data);
                    if value.is_last {
                        return Ok(Message::Data(value.presentation_context_id, bytes));
                    }
                }
            }
            Pdu::ReleaseRQ => {
                assoc.send(&Pdu::ReleaseRP).map_err(|e| e.to_string())?;
                return Ok(Message::Released);
            }
            other => return Err(format!("unexpected message: {other:?}")),
        }
    }
}

fn parse_command(bytes: &[u8]) -> Result<InMemDicomObject, String> {
    let ts = transfer_syntax(IMPLICIT_VR_LE).expect("implicit VR LE is always registered");
    InMemDicomObject::read_dataset_with_ts(bytes, ts).map_err(|e| e.to_string())
}

fn int(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Result<u16, String> {
    obj.element(tag)
        .map_err(|e| e.to_string())?
        .to_int::<u16>()
        .map_err(|e| e.to_string())
}

fn text(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Result<String, String> {
    let value = obj
        .element(tag)
        .map_err(|e| e.to_string())?
        .to_str()
        .map_err(|e| e.to_string())?;
    Ok(trim_uid(&value).to_owned())
}

fn response(field: u16, message: u16, class: &str, instance: Option<&str>, status: u16) -> Vec<u8> {
    let mut elements = vec![
        uid_element(tags::AFFECTED_SOP_CLASS_UID, class),
        us_element(tags::COMMAND_FIELD, field),
        us_element(tags::MESSAGE_ID_BEING_RESPONDED_TO, message),
        us_element(tags::COMMAND_DATA_SET_TYPE, NO_DATA_SET),
        us_element(tags::STATUS, status),
    ];
    if let Some(instance) = instance {
        elements.push(uid_element(tags::AFFECTED_SOP_INSTANCE_UID, instance));
    }
    command(&elements)
}

fn reply(assoc: &mut ServerAssociation<TcpStream>, pc: u8, data: &[u8]) -> Result<(), String> {
    assoc
        .send(&Pdu::PData {
            data: vec![dicom_ul::pdu::PDataValue {
                presentation_context_id: pc,
                value_type: PDataValueType::Command,
                is_last: true,
                data: data.to_vec(),
            }],
        })
        .map_err(|e| e.to_string())
}

fn save(path: &Path, data: &[u8], class: &str, instance: &str, ts: &str) -> Result<(), String> {
    let syntax = transfer_syntax(ts).ok_or_else(|| format!("unknown transfer syntax {ts}"))?;
    let obj = InMemDicomObject::read_dataset_with_ts(data, syntax).map_err(|e| e.to_string())?;
    obj.with_meta(
        FileMetaTableBuilder::new()
            .transfer_syntax(ts)
            .media_storage_sop_class_uid(class)
            .media_storage_sop_instance_uid(instance),
    )
    .map_err(|e| e.to_string())?
    .write_to_file(path)
    .map_err(|e| e.to_string())
}
