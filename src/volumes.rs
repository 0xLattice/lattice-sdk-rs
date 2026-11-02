//! Verified complete-Volume content transport, following @adalin/lattice-volumes.
//! Identity hashes are allowed here; consensus block links have stricter rules.
use data_encoding::BASE32_NOPAD;
use lattice_cashew::cbor::CborValue;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::RwLock,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

pub const VOLUME_ARCHIVE_MEDIA_TYPE: &str = "application/vnd.lattice.volume";
pub const MAXIMUM_VOLUME_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
pub const MAXIMUM_VOLUME_ARCHIVE_ENTRIES: usize = 0xffff;
pub const MAXIMUM_VOLUME_CID_BYTES: usize = 8192;
#[derive(Debug, Clone, Copy)]
pub struct VolumeLimits {
    pub maximum_entries: usize,
    pub maximum_bytes: usize,
}
impl Default for VolumeLimits {
    fn default() -> Self {
        Self {
            maximum_entries: 4096,
            maximum_bytes: MAXIMUM_VOLUME_ARCHIVE_BYTES,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VolumeEntry {
    pub cid: String,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SerializedVolume {
    pub root_cid: String,
    pub entries: Vec<VolumeEntry>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeErrorCode {
    NotFound,
    WrongRoot,
    MissingRoot,
    DuplicateEntry,
    TooManyEntries,
    TooLarge,
    InvalidEntry,
}
impl VolumeErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::WrongRoot => "wrong-root",
            Self::MissingRoot => "missing-root",
            Self::DuplicateEntry => "duplicate-entry",
            Self::TooManyEntries => "too-many-entries",
            Self::TooLarge => "too-large",
            Self::InvalidEntry => "invalid-entry",
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum VolumeError {
    #[error("{code:?}: {message}")]
    Verification {
        code: VolumeErrorCode,
        message: String,
    },
    #[error("invalid Volume archive: {0}")]
    Archive(&'static str),
    #[error("invalid content URL or options: {0}")]
    Configuration(&'static str),
    #[error("content gateway HTTP {status}: {message}")]
    Http { status: u16, message: &'static str },
    #[error(transparent)]
    Request(#[from] crate::transport::Error),
    #[error("request cancelled")]
    Cancelled,
    #[error("request deadline exceeded")]
    Timeout,
    #[error("invalid SDK DAG-CBOR: {0}")]
    Cbor(String),
}
fn failure(code: VolumeErrorCode, message: impl Into<String>) -> VolumeError {
    VolumeError::Verification {
        code,
        message: message.into(),
    }
}
/// An authenticated content collection. Construction is only possible through verification.
/// This authenticates bytes and root membership, not DAG reachability/completeness.
#[derive(Debug, Clone)]
pub struct VerifiedVolume {
    root_cid: String,
    entries: Vec<VolumeEntry>,
}
impl VerifiedVolume {
    /// A view of already authenticated entries, not a new storage boundary.
    pub(crate) fn with_root(mut self, root: &str) -> Result<Self, VolumeError> {
        if !self.has(root) {
            return Err(failure(VolumeErrorCode::MissingRoot, root));
        }
        self.root_cid = root.into();
        Ok(self)
    }

    pub fn root_cid(&self) -> &str {
        &self.root_cid
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn has(&self, cid: &str) -> bool {
        self.bytes(cid).is_some()
    }
    pub fn bytes(&self, cid: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.cid == cid)
            .map(|e| e.bytes.as_slice())
    }
    pub fn root_bytes(&self) -> &[u8] {
        self.bytes(&self.root_cid)
            .expect("verified root membership")
    }
    pub fn entries(&self) -> &[VolumeEntry] {
        &self.entries
    }
    /// SDK canonical decoding with explicit default resource limits.
    /// Byte verification itself does not parse or constrain CBOR.
    pub fn decode_root(&self) -> Result<CborValue, VolumeError> {
        decode_sdk_cbor(self.root_bytes())
    }
    pub fn decode_root_with_limits(&self, limits: DagCborLimits) -> Result<CborValue, VolumeError> {
        decode_dag_cbor(self.root_bytes(), limits)
    }
}
fn varint(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let start = *offset;
    let mut value = 0u64;
    loop {
        let b = *bytes.get(*offset)?;
        let shift = 7 * (*offset - start);
        if shift >= 63 {
            return None;
        }
        value = value.checked_add(((b & 127) as u64).checked_shl(shift as u32)?)?;
        *offset += 1;
        if value > 9007199254740991 {
            return None;
        }
        if b & 128 == 0 {
            if *offset - start > 1 && b == 0 {
                return None;
            }
            return Some(value);
        }
    }
}
/// SDK assertCID semantics: canonical base32 CIDv1, any codec, identity or full SHA-256.
pub fn verify_cid(cid: &str, content: &[u8]) -> bool {
    let Some(encoded) = cid.strip_prefix('b') else {
        return false;
    };
    if encoded.is_empty()
        || !encoded
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
    {
        return false;
    }
    let Ok(bytes) = BASE32_NOPAD.decode(encoded.to_ascii_uppercase().as_bytes()) else {
        return false;
    };
    if BASE32_NOPAD.encode(&bytes).to_ascii_lowercase() != encoded {
        return false;
    }
    let mut offset = 0;
    if varint(&bytes, &mut offset) != Some(1) {
        return false;
    }
    if varint(&bytes, &mut offset).is_none() {
        return false;
    }
    let algorithm = varint(&bytes, &mut offset);
    let Some(length) = varint(&bytes, &mut offset) else {
        return false;
    };
    let digest = &bytes[offset..];
    if digest.is_empty() || length != digest.len() as u64 {
        return false;
    }
    match algorithm {
        Some(0) => digest == content,
        Some(0x12) if digest.len() == 32 => digest == Sha256::digest(content).as_slice(),
        _ => false,
    }
}
pub fn verify_volume(
    volume: &SerializedVolume,
    expected_root: &str,
    limits: VolumeLimits,
) -> Result<VerifiedVolume, VolumeError> {
    if volume.root_cid != expected_root {
        return Err(failure(VolumeErrorCode::WrongRoot, "unexpected root"));
    }
    if volume.entries.len() > limits.maximum_entries {
        return Err(failure(VolumeErrorCode::TooManyEntries, "entry limit"));
    }
    let mut seen = BTreeSet::new();
    let mut size = 0usize;
    for e in &volume.entries {
        if !seen.insert(&e.cid) {
            return Err(failure(VolumeErrorCode::DuplicateEntry, e.cid.clone()));
        }
        size = size
            .checked_add(e.bytes.len())
            .filter(|s| *s <= limits.maximum_bytes)
            .ok_or_else(|| failure(VolumeErrorCode::TooLarge, "payload limit"))?;
        if !verify_cid(&e.cid, &e.bytes) {
            return Err(failure(VolumeErrorCode::InvalidEntry, e.cid.clone()));
        }
    }
    if !seen.contains(&expected_root.to_string()) {
        return Err(failure(VolumeErrorCode::MissingRoot, expected_root));
    }
    Ok(VerifiedVolume {
        root_cid: expected_root.into(),
        entries: volume.entries.clone(),
    })
}
pub fn encode_volume_archive(volume: &SerializedVolume) -> Result<Vec<u8>, VolumeError> {
    let verified = verify_volume(
        volume,
        &volume.root_cid,
        VolumeLimits {
            maximum_entries: MAXIMUM_VOLUME_ARCHIVE_ENTRIES,
            maximum_bytes: MAXIMUM_VOLUME_ARCHIVE_BYTES,
        },
    )?;
    let mut entries: Vec<_> = verified.entries.iter().collect();
    entries.sort_by(|a, b| a.cid.cmp(&b.cid));
    let mut length = 2usize;
    for e in &entries {
        if e.cid.len() > MAXIMUM_VOLUME_CID_BYTES {
            return Err(VolumeError::Archive("CID is too long"));
        }
        length = length
            .checked_add(6 + e.cid.len() + e.bytes.len())
            .filter(|s| *s <= MAXIMUM_VOLUME_ARCHIVE_BYTES)
            .ok_or(VolumeError::Archive("archive is too large"))?;
    }
    let mut out = Vec::with_capacity(length);
    out.extend((entries.len() as u16).to_be_bytes());
    for e in entries {
        out.extend((e.cid.len() as u16).to_be_bytes());
        out.extend(e.cid.as_bytes());
        out.extend((e.bytes.len() as u32).to_be_bytes());
        out.extend(&e.bytes);
    }
    Ok(out)
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], VolumeError> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|e| *e <= self.bytes.len())
            .ok_or(VolumeError::Archive("truncated field"))?;
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }
    fn u16(&mut self) -> Result<usize, VolumeError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as usize)
    }
    fn u32(&mut self) -> Result<usize, VolumeError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }
}
pub fn decode_volume_archive(
    root_cid: &str,
    archive: &[u8],
    limits: VolumeLimits,
) -> Result<SerializedVolume, VolumeError> {
    if archive.is_empty() || archive.len() > MAXIMUM_VOLUME_ARCHIVE_BYTES {
        return Err(VolumeError::Archive("archive is empty or too large"));
    }
    let mut r = Reader {
        bytes: archive,
        offset: 0,
    };
    let count = r.u16()?;
    if count == 0 {
        return Err(VolumeError::Archive("archive has no entries"));
    }
    if count > limits.maximum_entries {
        return Err(VolumeError::Archive("archive exceeds entry limit"));
    }
    let mut entries = Vec::<VolumeEntry>::with_capacity(count);
    let mut payload = 0usize;
    for _ in 0..count {
        let length = r.u16()?;
        if length > MAXIMUM_VOLUME_CID_BYTES {
            return Err(VolumeError::Archive("CID is too long"));
        }
        let raw = std::str::from_utf8(r.take(length)?)
            .map_err(|_| VolumeError::Archive("CID is not UTF-8"))?;
        // TextDecoder without ignoreBOM strips an initial BOM, as in the pinned archive decoder.
        let cid = raw.strip_prefix('\u{feff}').unwrap_or(raw).to_owned();
        if entries
            .last()
            .is_some_and(|e| e.cid.encode_utf16().cmp(cid.encode_utf16()).is_ge())
        {
            return Err(VolumeError::Archive("entries duplicated or out of order"));
        }
        let length = r.u32()?;
        let bytes = r.take(length)?;
        payload = payload
            .checked_add(length)
            .filter(|s| *s <= limits.maximum_bytes)
            .ok_or(VolumeError::Archive("archive exceeds byte limit"))?;
        entries.push(VolumeEntry {
            cid,
            bytes: bytes.to_vec(),
        });
    }
    if r.offset != archive.len() {
        return Err(VolumeError::Archive("trailing bytes"));
    }
    let volume = SerializedVolume {
        root_cid: root_cid.into(),
        entries,
    };
    verify_volume(&volume, root_cid, limits)?;
    Ok(volume)
}

#[cfg(not(target_arch = "wasm32"))]
pub type VolumeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<SerializedVolume>, VolumeError>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type VolumeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<SerializedVolume>, VolumeError>> + 'a>>;
pub trait VolumeTransport: Send + Sync {
    fn get_volume<'a>(
        &'a self,
        root_cid: &'a str,
        cancel: &'a CancellationToken,
    ) -> VolumeFuture<'a>;
}
#[derive(Default)]
pub struct MemoryVolumeTransport {
    volumes: RwLock<BTreeMap<String, SerializedVolume>>,
}
impl MemoryVolumeTransport {
    pub fn new(volumes: impl IntoIterator<Item = SerializedVolume>) -> Self {
        Self {
            volumes: RwLock::new(
                volumes
                    .into_iter()
                    .map(|v| (v.root_cid.clone(), v))
                    .collect(),
            ),
        }
    }
    pub fn put(&self, volume: SerializedVolume) {
        self.volumes
            .write()
            .expect("memory volume lock")
            .insert(volume.root_cid.clone(), volume);
    }
}
impl VolumeTransport for MemoryVolumeTransport {
    fn get_volume<'a>(&'a self, root: &'a str, _cancel: &'a CancellationToken) -> VolumeFuture<'a> {
        Box::pin(async move {
            Ok(self
                .volumes
                .read()
                .expect("memory volume lock")
                .get(root)
                .cloned())
        })
    }
}
pub struct VolumeClient<T> {
    transport: T,
    limits: VolumeLimits,
}
impl<T: VolumeTransport> VolumeClient<T> {
    pub fn new(transport: T, limits: VolumeLimits) -> Self {
        Self { transport, limits }
    }
    pub async fn get(
        &self,
        root: &str,
        cancel: &CancellationToken,
    ) -> Result<VerifiedVolume, VolumeError> {
        let raw = self
            .transport
            .get_volume(root, cancel)
            .await?
            .ok_or_else(|| failure(VolumeErrorCode::NotFound, root))?;
        verify_volume(&raw, root, self.limits)
    }
}
#[derive(Debug, Clone)]
pub struct HttpVolumeOptions {
    pub timeout: Duration,
    pub limits: VolumeLimits,
    pub authorization: Option<String>,
    pub chain_path: Option<Vec<String>>,
}
impl Default for HttpVolumeOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            limits: VolumeLimits::default(),
            authorization: None,
            chain_path: None,
        }
    }
}
pub struct HttpVolumeTransport {
    endpoint: Url,
    http: crate::transport::Client,
    options: HttpVolumeOptions,
}
impl HttpVolumeTransport {
    pub fn new(endpoint: &str, options: HttpVolumeOptions) -> Result<Self, VolumeError> {
        let mut url =
            Url::parse(endpoint).map_err(|_| VolumeError::Configuration("invalid URL"))?;
        if !url.username().is_empty()
            || url.password().is_some_and(|s| !s.is_empty())
            || url.query().is_some_and(|s| !s.is_empty())
            || url.fragment().is_some_and(|s| !s.is_empty())
        {
            return Err(VolumeError::Configuration(
                "credentials/query/fragment forbidden",
            ));
        }
        let loopback = match url.host() {
            Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(a)) => a.is_loopback(),
            Some(Host::Ipv6(a)) => a.is_loopback(),
            None => false,
        };
        if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
            return Err(VolumeError::Configuration(
                "HTTPS required except loopback HTTP",
            ));
        }
        if options
            .chain_path
            .as_ref()
            .is_some_and(|p| p.is_empty() || p.iter().any(String::is_empty))
        {
            return Err(VolumeError::Configuration(
                "nonempty chain path components required",
            ));
        }
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let http = crate::transport::client()?;
        Ok(Self {
            endpoint: url,
            http,
            options,
        })
    }
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }
    async fn request(&self, root: &str) -> Result<Option<SerializedVolume>, VolumeError> {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|_| VolumeError::Configuration("URL has no path"))?
            .pop_if_empty()
            .push(root);
        if let Some(path) = &self.options.chain_path {
            url.query_pairs_mut()
                .append_pair("chainPath", &path.join("/"));
        }
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, VOLUME_ARCHIVE_MEDIA_TYPE);
        if let Some(auth) = &self.options.authorization {
            request = request.header(reqwest::header::AUTHORIZATION, auth);
        }
        let mut response = crate::transport::response_limit(request, MAXIMUM_VOLUME_ARCHIVE_BYTES)
            .send()
            .await?;
        let status = response.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(VolumeError::Http {
                status,
                message: "non-success response (redirects forbidden)",
            });
        }
        let media = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .and_then(|v| v.split(';').next())
            .unwrap_or("")
            .trim();
        if !media.eq_ignore_ascii_case(VOLUME_ARCHIVE_MEDIA_TYPE) {
            return Err(VolumeError::Http {
                status,
                message: "unexpected content type",
            });
        }
        if let Some(length) = response.headers().get(reqwest::header::CONTENT_LENGTH) {
            let size = length
                .to_str()
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|n| *n <= MAXIMUM_VOLUME_ARCHIVE_BYTES as u64);
            if size.is_none() {
                return Err(VolumeError::Http {
                    status,
                    message: "response is too large",
                });
            }
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|n| n > MAXIMUM_VOLUME_ARCHIVE_BYTES)
            {
                return Err(VolumeError::Http {
                    status,
                    message: "response is too large",
                });
            }
            bytes.extend(chunk);
        }
        Ok(Some(decode_volume_archive(
            root,
            &bytes,
            self.options.limits,
        )?))
    }
}
impl VolumeTransport for HttpVolumeTransport {
    fn get_volume<'a>(&'a self, root: &'a str, cancel: &'a CancellationToken) -> VolumeFuture<'a> {
        Box::pin(async move {
            tokio::select! {biased;_=cancel.cancelled()=>Err(VolumeError::Cancelled),result=crate::runtime::timeout(self.options.timeout,self.request(root))=>result.map_err(|_|VolumeError::Timeout)?}
        })
    }
}
/// Resource policy for SDK DAG-CBOR decoding, separate from byte authentication.
/// TypeScript has no declared depth cap. Rust explicitly caps nesting by default
/// to keep subsequent traversal/destruction of the recursive value stack-safe.
#[derive(Debug, Clone, Copy)]
pub struct DagCborLimits {
    pub maximum_depth: usize,
    pub maximum_values: usize,
}
impl Default for DagCborLimits {
    fn default() -> Self {
        Self {
            maximum_depth: 512,
            maximum_values: MAXIMUM_VOLUME_ARCHIVE_BYTES,
        }
    }
}
enum Frame {
    Array {
        left: usize,
        values: Vec<CborValue>,
    },
    Map {
        left: usize,
        entries: Vec<(CborValue, CborValue)>,
        key: Option<String>,
        previous: Option<String>,
    },
}
/// SDK canonical integer/bytes/text/array/map/bool/null decoder. Floats and tags
/// are forbidden; text preserves BOMs. Frames are parsed iteratively.
pub fn decode_dag_cbor(bytes: &[u8], limits: DagCborLimits) -> Result<CborValue, VolumeError> {
    fn error(msg: &str) -> VolumeError {
        VolumeError::Cbor(msg.into())
    }
    let mut reader = Reader { bytes, offset: 0 };
    let mut frames = Vec::<Frame>::new();
    let mut count = 0usize;
    loop {
        count = count
            .checked_add(1)
            .filter(|n| *n <= limits.maximum_values)
            .ok_or_else(|| error("value limit exceeded"))?;
        let initial = reader.take(1).map_err(|_| error("truncated value"))?[0];
        let major = initial >> 5;
        let additional = initial & 31;
        let mut value = if major == 7 {
            match additional {
                20 => CborValue::Bool(false),
                21 => CborValue::Bool(true),
                22 => CborValue::Null,
                _ => return Err(error("floats and unsupported simple values forbidden")),
            }
        } else {
            let n = match additional {
                0..=23 => additional as u64,
                24..=27 => {
                    let length = 1usize << (additional - 24);
                    let raw = reader
                        .take(length)
                        .map_err(|_| error("truncated argument"))?;
                    let mut n = 0u64;
                    for b in raw {
                        n = (n << 8) | u64::from(*b);
                    }
                    let minimum = if length == 1 {
                        24
                    } else {
                        1u64 << (8 * (length / 2))
                    };
                    if n < minimum {
                        return Err(error("noncanonical integer or length"));
                    }
                    n
                }
                _ => return Err(error("indefinite/reserved length")),
            };
            match major {
                0 => CborValue::Integer(n.into()),
                1 => CborValue::Integer(
                    (-1i128 - i128::from(n))
                        .try_into()
                        .map_err(|_| error("integer out of range"))?,
                ),
                2 | 3 => {
                    let length = usize::try_from(n).map_err(|_| error("length overflow"))?;
                    let raw = reader
                        .take(length)
                        .map_err(|_| error("truncated bytes/string"))?;
                    if major == 2 {
                        CborValue::Bytes(raw.to_vec())
                    } else {
                        CborValue::Text(
                            std::str::from_utf8(raw)
                                .map_err(|_| error("invalid UTF-8"))?
                                .to_owned(),
                        )
                    }
                }
                4 | 5 => {
                    let length = usize::try_from(n).map_err(|_| error("length overflow"))?;
                    if length > bytes.len().saturating_sub(reader.offset) {
                        return Err(error("truncated collection"));
                    }
                    if frames.len() >= limits.maximum_depth {
                        return Err(error("depth limit exceeded"));
                    }
                    if length == 0 {
                        if major == 4 {
                            CborValue::Array(vec![])
                        } else {
                            CborValue::Map(vec![])
                        }
                    } else {
                        frames.push(if major == 4 {
                            Frame::Array {
                                left: length,
                                values: Vec::new(),
                            }
                        } else {
                            Frame::Map {
                                left: length,
                                entries: Vec::new(),
                                key: None,
                                previous: None,
                            }
                        });
                        continue;
                    }
                }
                _ => return Err(error("unsupported major type")),
            }
        };
        loop {
            let Some(frame) = frames.last_mut() else {
                if reader.offset != bytes.len() {
                    return Err(error("trailing bytes"));
                }
                return Ok(value);
            };
            match frame {
                Frame::Array { left, values } => {
                    values.push(value);
                    *left -= 1;
                    if *left > 0 {
                        break;
                    }
                    let Frame::Array { values, .. } = frames.pop().unwrap() else {
                        unreachable!()
                    };
                    value = CborValue::Array(values);
                }
                Frame::Map {
                    left,
                    entries,
                    key,
                    previous,
                } => {
                    if let Some(k) = key.take() {
                        entries.push((CborValue::Text(k), value));
                        *left -= 1;
                        if *left > 0 {
                            break;
                        }
                        let Frame::Map { entries, .. } = frames.pop().unwrap() else {
                            unreachable!()
                        };
                        value = CborValue::Map(entries);
                    } else {
                        let CborValue::Text(k) = value else {
                            return Err(error("map key is not text"));
                        };
                        if previous.as_ref().is_some_and(|p| {
                            p.len()
                                .cmp(&k.len())
                                .then(p.as_bytes().cmp(k.as_bytes()))
                                .is_ge()
                        }) {
                            return Err(error("duplicate/unordered map keys"));
                        }
                        *previous = Some(k.clone());
                        *key = Some(k);
                        break;
                    }
                }
            }
        }
    }
}
fn decode_sdk_cbor(bytes: &[u8]) -> Result<CborValue, VolumeError> {
    decode_dag_cbor(bytes, DagCborLimits::default())
}
#[derive(Debug, Clone)]
pub struct Uint64SparseProof {
    pub root_cid: String,
    pub key: String,
    pub value: Option<u64>,
    pub entries: Vec<VolumeEntry>,
}
fn property<'a>(v: &'a CborValue, key: &str) -> Option<&'a CborValue> {
    let CborValue::Map(entries) = v else {
        return None;
    };
    entries
        .iter()
        .find_map(|(k, v)| matches!(k,CborValue::Text(s) if s==key).then_some(v))
}
pub fn verify_uint64_sparse_proof(proof: &Uint64SparseProof) -> bool {
    let Ok(volume) = verify_volume(
        &SerializedVolume {
            root_cid: proof.root_cid.clone(),
            entries: proof.entries.clone(),
        },
        &proof.root_cid,
        VolumeLimits::default(),
    ) else {
        return false;
    };
    let mut cid = proof.root_cid.as_str();
    let mut remaining: Vec<u16> = proof.key.encode_utf16().collect();
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(cid.to_owned()) {
            return false;
        }
        let Some(bytes) = volume.bytes(cid) else {
            return false;
        };
        let Ok(node) = decode_sdk_cbor(bytes) else {
            return false;
        };
        if !matches!(node, CborValue::Map(_)) {
            return false;
        }
        if let Some(prefix) = property(&node, "prefix") {
            let CborValue::Text(prefix) = prefix else {
                return false;
            };
            let prefix: Vec<_> = prefix.encode_utf16().collect();
            if !remaining.starts_with(&prefix) {
                return proof.value.is_none();
            }
            remaining.drain(..prefix.len());
        }
        if remaining.is_empty() {
            return match property(&node, "value") {
                None => proof.value.is_none(),
                Some(CborValue::Integer(n)) => {
                    proof.value.is_some_and(|v| i128::from(*n) == i128::from(v))
                }
                _ => false,
            };
        }
        let Some(CborValue::Array(children)) = property(&node, "children") else {
            return false;
        };
        let mut next = None;
        for child in children {
            let Some(CborValue::Text(key)) = property(child, "key") else {
                return false;
            };
            let chars: Vec<_> = key.encode_utf16().collect();
            if chars.len() != 1 {
                return false;
            }
            let Some(value) = property(child, "value") else {
                return false;
            };
            let Some(CborValue::Text(child_cid)) = property(value, "rawCID") else {
                return false;
            };
            if chars[0] == remaining[0] {
                if next.is_some() {
                    return false;
                }
                next = Some(child_cid.clone());
            }
        }
        let Some(next) = next else {
            return proof.value.is_none();
        };
        let Some(entry) = volume.entries.iter().find(|e| e.cid == next) else {
            return false;
        };
        cid = &entry.cid;
    }
}
