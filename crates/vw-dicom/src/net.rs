//! 他の機器（PACS など）への送信。C-ECHO で接続を確かめ、C-STORE でファイルを送る。
//!
//! 1 回の送信では、送るファイルの SOP Class と転送構文の組をすべて集めて、
//! 1 つのアソシエーションでまとめて提案する。ファイルごとにつなぎ直すと、
//! 数千ファイルのシリーズで接続の手間が積み重なり、相手のログも埋まるため。
//!
//! 送るデータは書き換えない。転送構文はファイルのままを第一候補にし、相手がそれを
//! 受けないときは、画素が圧縮されていなければ非圧縮の転送構文で符号化し直して送る。
//! 圧縮された画素を展開して送る変換はしない（画質や大きさが変わるのを送る側で決めないため）。

use std::io::Read;
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_encoding::transfer_syntax::{TransferSyntax, TransferSyntaxIndex};
use dicom_object::file::ReadPreamble;
use dicom_object::{DefaultDicomObject, InMemDicomObject, OpenFileOptions};
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use dicom_ul::association::client::{ClientAssociation, ClientAssociationOptions};
use dicom_ul::pdu::{PDataValue, PDataValueType, Pdu};

pub mod receiver;

const VERIFICATION: &str = "1.2.840.10008.1.1";
const IMPLICIT_VR_LE: &str = "1.2.840.10008.1.2";
const EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";

const C_STORE_RQ: u16 = 0x0001;
const C_ECHO_RQ: u16 = 0x0030;
/// Command Data Set Type: データセットが続かない。
const NO_DATA_SET: u16 = 0x0101;
/// Command Data Set Type: データセットが続く（0x0101 以外なら何でもよい）。
const DATA_SET_FOLLOWS: u16 = 0x0000;
/// AE タイトルの長さの上限（DICOM の規格）。
pub const AE_TITLE_MAX: usize = 16;
/// 1 つのアソシエーションで提案できる組の上限（提案の ID は 1〜255 の奇数）。
const MAX_CONTEXTS: usize = 128;

/// 読み込んだ DICOM ファイル。
pub type Object = DefaultDicomObject;

/// 送り手と相手。
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    /// 自分の AE タイトル（Calling AE）。
    pub calling_ae: String,
    pub host: String,
    pub port: u16,
    /// 相手の AE タイトル（Called AE）。
    pub called_ae: String,
    pub timeout: Duration,
}

impl Endpoint {
    fn options(&self) -> ClientAssociationOptions<'_> {
        ClientAssociationOptions::new()
            .calling_ae_title(self.calling_ae.as_str())
            .called_ae_title(self.called_ae.as_str())
            .connection_timeout(self.timeout)
            .read_timeout(self.timeout)
            .write_timeout(self.timeout)
    }

    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// AE タイトルとして使えるか。1〜16 文字の英大文字・数字・空白・`_`・`-` で、空白だけではない。
///
/// 規格は制御文字と `\` 以外を許すが、小文字や記号は相手の機器によって扱いがまちまちで、
/// 大文字に直されて一致しなくなることがあるので、ここで狭めておく。
pub fn valid_ae_title(title: &str) -> bool {
    let allowed = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || " _-".contains(c);
    (1..=AE_TITLE_MAX).contains(&title.len())
        && title.chars().all(allowed)
        && !title.trim().is_empty()
}

/// 送れなかった理由。
#[derive(Debug, Clone, PartialEq)]
pub enum SendError {
    /// このファイルは送れないが、残りは送り続けられる（相手がこの種類を受けない、
    /// 相手が失敗を返した、ファイルが読めない）。
    File(String),
    /// 接続が切れたなど、残りも送れない。
    Link(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::File(m) | SendError::Link(m) => f.write_str(m),
        }
    }
}

/// 相手が受け取ったときの結果。警告（B000 など）でも保存はされている。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Success,
    Warning(u16),
}

/// 送るファイルの、提案に使う属性。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Header {
    pub sop_class: String,
    pub sop_instance: String,
    pub transfer_syntax: String,
}

impl Header {
    pub fn of(obj: &DefaultDicomObject) -> Self {
        let meta = obj.meta();
        Self {
            sop_class: trim_uid(meta.media_storage_sop_class_uid()).to_owned(),
            sop_instance: trim_uid(meta.media_storage_sop_instance_uid()).to_owned(),
            transfer_syntax: trim_uid(meta.transfer_syntax()).to_owned(),
        }
    }
}

fn trim_uid(uid: &str) -> &str {
    uid.trim_end_matches(['\0', ' '])
}

/// DICOM ファイル（128 バイトのプリアンブルつき）を読む。`header_only` なら画素の手前まで。
pub fn open(data: &[u8], header_only: bool) -> Result<DefaultDicomObject, String> {
    crate::structure::check(data, data.len() as u64, header_only).map_err(|e| e.to_string())?;
    from_reader(data, header_only)
}

fn from_reader(source: impl Read, header_only: bool) -> Result<DefaultDicomObject, String> {
    let mut options = OpenFileOptions::new().read_preamble(ReadPreamble::Always);
    if header_only {
        options = options.read_until(tags::PIXEL_DATA);
    }
    options.from_reader(source).map_err(|e| e.to_string())
}

/// 提案に使う属性を読むときに、長さを確かめる頭の大きさ。画素の手前まではふつう数 KB。
const HEAD_BYTES: u64 = 1 << 20;

/// ファイルの、提案に使う属性を読む。たくさんのファイルを順に見るので、全体は読まない。
pub fn read_header(path: &Path) -> Result<Header, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let mut head = Vec::new();
    (&mut file)
        .take(HEAD_BYTES)
        .read_to_end(&mut head)
        .map_err(|e| e.to_string())?;
    crate::structure::check(&head, total, true).map_err(|e| e.to_string())?;
    let rest = std::io::BufReader::new(file);
    from_reader(std::io::Cursor::new(head).chain(rest), true).map(|obj| Header::of(&obj))
}

fn transfer_syntax(uid: &str) -> Option<&'static TransferSyntax> {
    TransferSyntaxRegistry.get(uid)
}

/// 画素が圧縮されていない（符号化し直しても画素が変わらない）転送構文か。
fn is_native(uid: &str) -> bool {
    transfer_syntax(uid).is_some_and(|ts| !ts.is_encapsulated_pixel_data())
}

/// 提案する（SOP Class, 転送構文の候補）の並び。ファイルの転送構文を第一候補にし、
/// 非圧縮の 2 つを後ろに足す。同じ組は 1 つにまとめる。
fn proposals<'a>(headers: impl IntoIterator<Item = &'a Header>) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for h in headers {
        let first = (h.sop_class.clone(), h.transfer_syntax.clone());
        if out.iter().any(|(c, ts)| c == &first.0 && ts[0] == first.1) {
            continue;
        }
        let mut syntaxes = vec![first.1];
        for fallback in [EXPLICIT_VR_LE, IMPLICIT_VR_LE] {
            if !syntaxes.iter().any(|t| t == fallback) {
                syntaxes.push(fallback.to_owned());
            }
        }
        out.push((first.0, syntaxes));
    }
    out
}

/// C-ECHO で、相手が DICOM の通信を受け付けるか確かめる。
pub fn echo(endpoint: &Endpoint) -> Result<(), String> {
    let mut assoc = endpoint
        .options()
        .with_presentation_context(VERIFICATION, vec![EXPLICIT_VR_LE, IMPLICIT_VR_LE])
        .establish_with(&format!("{}@{}", endpoint.called_ae, endpoint.address()))
        .map_err(|e| e.to_string())?;
    let pc = assoc
        .presentation_contexts()
        .first()
        .map(|pc| pc.id)
        .ok_or("the peer does not accept verification (C-ECHO)")?;
    let command = command(&[
        uid_element(tags::AFFECTED_SOP_CLASS_UID, VERIFICATION),
        us_element(tags::COMMAND_FIELD, C_ECHO_RQ),
        us_element(tags::MESSAGE_ID, 1),
        us_element(tags::COMMAND_DATA_SET_TYPE, NO_DATA_SET),
    ]);
    let result = send_command(&mut assoc, pc, &command).and_then(|()| receive_status(&mut assoc));
    let _ = assoc.release();
    match result? {
        0 => Ok(()),
        status => Err(format!("the peer answered status {status:04X}H")),
    }
}

/// C-STORE のためにつないだアソシエーション。
pub struct Store {
    assoc: ClientAssociation<TcpStream>,
    next_message: u16,
}

impl Store {
    /// `headers` のファイルを送るための組をまとめて提案してつなぐ。
    pub fn connect<'a>(
        endpoint: &Endpoint,
        headers: impl IntoIterator<Item = &'a Header>,
    ) -> Result<Self, String> {
        let mut options = endpoint.options();
        let proposed = proposals(headers);
        if proposed.is_empty() {
            return Err("nothing to send".into());
        }
        for (class, syntaxes) in proposed.into_iter().take(MAX_CONTEXTS) {
            options = options.with_presentation_context(class, syntaxes);
        }
        let mut assoc = options
            .establish_with(&format!("{}@{}", endpoint.called_ae, endpoint.address()))
            .map_err(|e| e.to_string())?;
        // 送って応えを待つ往復が続くので、Nagle で小さな PDU を溜めると 1 ファイルごとに
        // 遅延 ACK の待ちが入り、桁違いに遅くなる。
        let _ = assoc.inner_stream().set_nodelay(true);
        Ok(Self {
            assoc,
            next_message: 1,
        })
    }

    /// 1 ファイルを送る。
    pub fn send(&mut self, obj: &DefaultDicomObject) -> Result<Stored, SendError> {
        let header = Header::of(obj);
        let (pc, ts) = self.context_for(&header)?;
        let ts = transfer_syntax(&ts)
            .ok_or_else(|| SendError::File(format!("unknown transfer syntax {ts}")))?;
        let mut data = Vec::new();
        obj.write_dataset_with_ts(&mut data, ts)
            .map_err(|e| SendError::File(e.to_string()))?;

        let message = self.next_message;
        self.next_message = self.next_message.wrapping_add(1).max(1);
        let command = command(&[
            uid_element(tags::AFFECTED_SOP_CLASS_UID, &header.sop_class),
            us_element(tags::COMMAND_FIELD, C_STORE_RQ),
            us_element(tags::MESSAGE_ID, message),
            us_element(tags::PRIORITY, 0),
            us_element(tags::COMMAND_DATA_SET_TYPE, DATA_SET_FOLLOWS),
            uid_element(tags::AFFECTED_SOP_INSTANCE_UID, &header.sop_instance),
        ]);
        let link = |e: String| SendError::Link(e);
        send_command(&mut self.assoc, pc, &command).map_err(link)?;
        {
            let mut writer = self.assoc.send_pdata(pc);
            std::io::Write::write_all(&mut writer, &data).map_err(|e| link(e.to_string()))?;
            writer.finish().map_err(|e| link(e.to_string()))?;
        }
        match receive_status(&mut self.assoc).map_err(link)? {
            0 => Ok(Stored::Success),
            s @ (0xB000 | 0xB006 | 0xB007) => Ok(Stored::Warning(s)),
            s => Err(SendError::File(format!(
                "the peer answered status {s:04X}H"
            ))),
        }
    }

    /// 相手が受け入れた組から、このファイルを送る組（提案の ID と転送構文）を選ぶ。
    fn context_for(&self, header: &Header) -> Result<(u8, String), SendError> {
        let accepted: Vec<_> = self
            .assoc
            .presentation_contexts()
            .iter()
            .filter(|pc| trim_uid(&pc.abstract_syntax) == header.sop_class)
            .collect();
        if accepted.is_empty() {
            return Err(SendError::File(format!(
                "the peer does not accept SOP class {}",
                header.sop_class
            )));
        }
        if let Some(pc) = accepted
            .iter()
            .find(|pc| trim_uid(&pc.transfer_syntax) == header.transfer_syntax)
        {
            return Ok((pc.id, header.transfer_syntax.clone()));
        }
        if is_native(&header.transfer_syntax)
            && let Some(pc) = accepted
                .iter()
                .find(|pc| is_native(trim_uid(&pc.transfer_syntax)))
            {
                return Ok((pc.id, trim_uid(&pc.transfer_syntax).to_owned()));
            }
        let name = transfer_syntax(&header.transfer_syntax)
            .map_or(String::new(), |ts| format!("{} ", ts.name()));
        Err(SendError::File(format!(
            "the peer does not accept the compressed transfer syntax {name}({})",
            header.transfer_syntax
        )))
    }

    /// 送り終えて、アソシエーションを閉じる。
    pub fn release(self) {
        let _ = self.assoc.release();
    }

    /// 途中で取り消して、アソシエーションを切る。
    pub fn abort(self) {
        let _ = self.assoc.abort();
    }
}

fn us_element(tag: dicom_core::Tag, value: u16) -> DataElement<InMemDicomObject> {
    DataElement::new(tag, VR::US, PrimitiveValue::from(value))
}

/// UID の値。奇数の長さなら規格どおり NUL で偶数にする。
fn uid_element(tag: dicom_core::Tag, uid: &str) -> DataElement<InMemDicomObject> {
    let mut value = uid.to_owned();
    if value.len() % 2 == 1 {
        value.push('\0');
    }
    DataElement::new(tag, VR::UI, PrimitiveValue::from(value))
}

/// コマンドを Implicit VR Little Endian で符号化する（コマンドは常にこの転送構文）。
fn command(elements: &[DataElement<InMemDicomObject>]) -> Vec<u8> {
    let obj = InMemDicomObject::command_from_element_iter(elements.iter().cloned());
    let ts = transfer_syntax(IMPLICIT_VR_LE).expect("implicit VR LE is always registered");
    let mut out = Vec::new();
    obj.write_dataset_with_ts(&mut out, ts)
        .expect("a command set always encodes");
    out
}

fn send_command(
    assoc: &mut ClientAssociation<TcpStream>,
    pc: u8,
    data: &[u8],
) -> Result<(), String> {
    assoc
        .send(&Pdu::PData {
            data: vec![PDataValue {
                presentation_context_id: pc,
                value_type: PDataValueType::Command,
                is_last: true,
                data: data.to_vec(),
            }],
        })
        .map_err(|e| e.to_string())
}

/// 応答のコマンドを受け取り、Status (0000,0900) を返す。
fn receive_status(assoc: &mut ClientAssociation<TcpStream>) -> Result<u16, String> {
    let mut bytes = Vec::new();
    loop {
        match assoc.receive().map_err(|e| e.to_string())? {
            Pdu::PData { data } => {
                let mut last = false;
                for value in data {
                    if value.value_type == PDataValueType::Command {
                        bytes.extend_from_slice(&value.data);
                        last |= value.is_last;
                    }
                }
                if last {
                    break;
                }
            }
            Pdu::AbortRQ { .. } => return Err("the peer aborted the association".into()),
            Pdu::ReleaseRQ => return Err("the peer closed the association".into()),
            other => return Err(format!("unexpected message from the peer: {other:?}")),
        }
    }
    let ts = transfer_syntax(IMPLICIT_VR_LE).expect("implicit VR LE is always registered");
    let response =
        InMemDicomObject::read_dataset_with_ts(&bytes[..], ts).map_err(|e| e.to_string())?;
    response
        .element(tags::STATUS)
        .ok()
        .and_then(|e| e.to_int::<u16>().ok())
        .ok_or_else(|| "the peer's answer has no status".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(class: &str, ts: &str) -> Header {
        Header {
            sop_class: class.into(),
            sop_instance: "1.2.3".into(),
            transfer_syntax: ts.into(),
        }
    }

    #[test]
    fn ae_title_allows_upper_case_digits_space_underscore_and_hyphen() {
        assert!(valid_ae_title("VW"));
        assert!(valid_ae_title("TEST_PACS-1 A"));
        assert!(valid_ae_title("ABCDEFGHIJKLMNOP"));
    }

    #[test]
    fn ae_title_rejects_empty_long_lower_case_and_blank() {
        assert!(!valid_ae_title(""));
        assert!(!valid_ae_title("ABCDEFGHIJKLMNOPQ"));
        assert!(!valid_ae_title("pacs"));
        assert!(!valid_ae_title("   "));
        assert!(!valid_ae_title("A\\B"));
    }

    #[test]
    fn file_transfer_syntax_is_proposed_first_with_uncompressed_fallbacks() {
        let jpeg_ls = "1.2.840.10008.1.2.4.80";
        let p = proposals([&header("1.2.840.10008.5.1.4.1.1.2", jpeg_ls)]);
        assert_eq!(
            p,
            vec![(
                "1.2.840.10008.5.1.4.1.1.2".to_string(),
                vec![jpeg_ls.into(), EXPLICIT_VR_LE.into(), IMPLICIT_VR_LE.into()]
            )]
        );
    }

    #[test]
    fn same_class_and_syntax_is_proposed_once() {
        let ct = "1.2.840.10008.5.1.4.1.1.2";
        let a = header(ct, EXPLICIT_VR_LE);
        let b = header(ct, IMPLICIT_VR_LE);
        let p = proposals([&a, &a.clone(), &b]);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].1, vec![EXPLICIT_VR_LE, IMPLICIT_VR_LE]);
        assert_eq!(p[1].1, vec![IMPLICIT_VR_LE, EXPLICIT_VR_LE]);
    }

    use crate::synth::{Synth, CT_IMAGE, JPEG_LS_LOSSLESS};
    use receiver::{Received, Receiver};

    /// 受け手を別のスレッドで動かし、そこへ送る相手を返す。
    fn spawn_receiver(
        accept_compressed: bool,
        store_status: u16,
    ) -> (
        Endpoint,
        std::thread::JoinHandle<Result<Received, String>>,
        tempdir::Dir,
    ) {
        let dir = tempdir::Dir::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let receiver = Receiver {
            ae_title: "TESTSCP".into(),
            accept_compressed,
            dir: dir.0.clone(),
            store_status,
        };
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().map_err(|e| e.to_string())?;
            receiver.serve(stream)
        });
        let endpoint = Endpoint {
            calling_ae: "VW".into(),
            host: "127.0.0.1".into(),
            port,
            called_ae: "TESTSCP".into(),
            timeout: Duration::from_secs(10),
        };
        (endpoint, handle, dir)
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static N: AtomicUsize = AtomicUsize::new(0);
                let p = std::env::temp_dir().join(format!(
                    "vw-net-test-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn ct_file(instance: &str) -> DefaultDicomObject {
        let bytes = Synth::ct(4, 4).instance(instance, 1).bytes();
        open(&bytes[..], false).unwrap()
    }

    fn jpeg_ls_file() -> DefaultDicomObject {
        let bytes = Synth::ct(4, 4)
            .instance("1.2.826.0.1.3680043.9.77", 1)
            .encapsulated(JPEG_LS_LOSSLESS, vec![vec![0xFF, 0xD8, 0xFF, 0xD9]])
            .bytes();
        open(&bytes[..], false).unwrap()
    }

    #[test]
    fn echo_succeeds_against_a_listening_peer() {
        let (endpoint, handle, _dir) = spawn_receiver(false, 0);
        echo(&endpoint).unwrap();
        assert_eq!(handle.join().unwrap().unwrap().echoes, 1);
    }

    #[test]
    fn echo_reports_a_refused_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let endpoint = Endpoint {
            calling_ae: "VW".into(),
            host: "127.0.0.1".into(),
            port,
            called_ae: "NOBODY".into(),
            timeout: Duration::from_secs(2),
        };
        assert!(echo(&endpoint).is_err());
    }

    #[test]
    fn files_are_stored_in_one_association() {
        let (endpoint, handle, _dir) = spawn_receiver(false, 0);
        let files = [
            ct_file("1.2.826.0.1.3680043.9.11"),
            ct_file("1.2.826.0.1.3680043.9.12"),
        ];
        let headers: Vec<Header> = files.iter().map(Header::of).collect();
        let mut store = Store::connect(&endpoint, &headers).unwrap();
        for f in &files {
            assert_eq!(store.send(f).unwrap(), Stored::Success);
        }
        store.release();
        let received = handle.join().unwrap().unwrap();
        assert_eq!(received.files.len(), 2);
        let back = OpenFileOptions::new()
            .open_file(&received.files[1])
            .unwrap();
        assert_eq!(
            trim_uid(back.meta().media_storage_sop_instance_uid()),
            "1.2.826.0.1.3680043.9.12"
        );
    }

    #[test]
    fn compressed_file_is_not_sent_to_a_peer_that_only_takes_uncompressed() {
        let (endpoint, handle, _dir) = spawn_receiver(false, 0);
        let files = [jpeg_ls_file(), ct_file("1.2.826.0.1.3680043.9.13")];
        let headers: Vec<Header> = files.iter().map(Header::of).collect();
        let mut store = Store::connect(&endpoint, &headers).unwrap();
        assert!(matches!(store.send(&files[0]), Err(SendError::File(_))));
        assert_eq!(store.send(&files[1]).unwrap(), Stored::Success);
        store.release();
        assert_eq!(handle.join().unwrap().unwrap().files.len(), 1);
    }

    #[test]
    fn compressed_file_keeps_its_transfer_syntax_when_the_peer_accepts_it() {
        let (endpoint, handle, _dir) = spawn_receiver(true, 0);
        let file = jpeg_ls_file();
        let mut store = Store::connect(&endpoint, [&Header::of(&file)]).unwrap();
        assert_eq!(store.send(&file).unwrap(), Stored::Success);
        store.release();
        let received = handle.join().unwrap().unwrap();
        assert_eq!(received.transfer_syntaxes, [JPEG_LS_LOSSLESS]);
    }

    #[test]
    fn failure_status_from_the_peer_fails_only_that_file() {
        let (endpoint, handle, _dir) = spawn_receiver(false, 0xA700);
        let file = ct_file("1.2.826.0.1.3680043.9.14");
        let mut store = Store::connect(&endpoint, [&Header::of(&file)]).unwrap();
        let err = store.send(&file).unwrap_err();
        assert_eq!(
            err,
            SendError::File("the peer answered status A700H".into())
        );
        store.release();
        assert!(handle.join().unwrap().unwrap().files.is_empty());
    }

    #[test]
    fn sop_class_is_ct() {
        assert_eq!(Header::of(&ct_file("1.2.3.4")).sop_class, CT_IMAGE);
    }

    #[test]
    fn only_uncompressed_syntaxes_can_be_re_encoded() {
        assert!(is_native(EXPLICIT_VR_LE));
        assert!(is_native(IMPLICIT_VR_LE));
        assert!(!is_native("1.2.840.10008.1.2.4.80"));
        assert!(!is_native("1.2.840.10008.1.2.4.50"));
    }
}
