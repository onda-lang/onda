use std::fmt;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{checked_product, ProjectError, ProjectLimits};

// Synchronized from format-versions.json; do not edit this copy directly.
pub const ONDA_BUFFER_FORMAT_VERSION: u32 = 1;
const ONDA_BUFFER_MAGIC: &[u8; 8] = b"ONDABUF\0";
pub(crate) const ONDA_BUFFER_HEADER_BYTES: usize = 8 + 4 + 4 + 4 + 4 + 4 + 8 + 32;

#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BufferElement {
    Bool,
    I32,
    I64,
    F32,
    F64,
}

impl BufferElement {
    pub fn byte_size(self) -> usize {
        match self {
            Self::Bool => 1,
            Self::I32 | Self::F32 => 4,
            Self::I64 | Self::F64 => 8,
        }
    }

    fn tag(self) -> u8 {
        match self {
            Self::Bool => 0,
            Self::I32 => 1,
            Self::I64 => 2,
            Self::F32 => 3,
            Self::F64 => 4,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, ProjectError> {
        match tag {
            0 => Ok(Self::Bool),
            1 => Ok(Self::I32),
            2 => Ok(Self::I64),
            3 => Ok(Self::F32),
            4 => Ok(Self::F64),
            _ => Err(ProjectError::new(format!(
                "unknown Onda buffer element tag {tag}"
            ))),
        }
    }
}

impl fmt::Display for BufferElement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bool => "bool",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BufferSamples {
    Bool(Vec<u8>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl BufferSamples {
    pub fn element(&self) -> BufferElement {
        match self {
            Self::Bool(_) => BufferElement::Bool,
            Self::I32(_) => BufferElement::I32,
            Self::I64(_) => BufferElement::I64,
            Self::F32(_) => BufferElement::F32,
            Self::F64(_) => BufferElement::F64,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Bool(values) => values.len(),
            Self::I32(values) => values.len(),
            Self::I64(values) => values.len(),
            Self::F32(values) => values.len(),
            Self::F64(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_ptr(&self) -> *const u8 {
        match self {
            Self::Bool(values) => values.as_ptr(),
            Self::I32(values) => values.as_ptr().cast::<u8>(),
            Self::I64(values) => values.as_ptr().cast::<u8>(),
            Self::F32(values) => values.as_ptr().cast::<u8>(),
            Self::F64(values) => values.as_ptr().cast::<u8>(),
        }
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        match self {
            Self::Bool(values) => values.as_mut_ptr(),
            Self::I32(values) => values.as_mut_ptr().cast::<u8>(),
            Self::I64(values) => values.as_mut_ptr().cast::<u8>(),
            Self::F32(values) => values.as_mut_ptr().cast::<u8>(),
            Self::F64(values) => values.as_mut_ptr().cast::<u8>(),
        }
    }

    fn visit_canonical_le_bytes(&self, mut visit: impl FnMut(&[u8])) {
        const CHUNK_BYTES: usize = 16 * 1024;
        macro_rules! visit_numeric {
            ($values:expr, $element_bytes:expr, $to_le_bytes:expr) => {{
                let mut bytes = [0u8; CHUNK_BYTES];
                for values in $values.chunks(CHUNK_BYTES / $element_bytes) {
                    for (value, output) in values.iter().zip(bytes.chunks_exact_mut($element_bytes))
                    {
                        output.copy_from_slice(&($to_le_bytes)(*value));
                    }
                    visit(&bytes[..values.len() * $element_bytes]);
                }
            }};
        }

        match self {
            Self::Bool(values) => visit(values),
            Self::I32(values) => visit_numeric!(values, 4, i32::to_le_bytes),
            Self::I64(values) => visit_numeric!(values, 8, i64::to_le_bytes),
            Self::F32(values) => {
                visit_numeric!(values, 4, |value: f32| value.to_bits().to_le_bytes())
            }
            Self::F64(values) => {
                visit_numeric!(values, 8, |value: f64| value.to_bits().to_le_bytes())
            }
        }
    }

    fn append_canonical_le_bytes(&self, output: &mut Vec<u8>) {
        self.visit_canonical_le_bytes(|bytes| output.extend_from_slice(bytes));
    }

    pub fn from_canonical_le_bytes(
        element: BufferElement,
        bytes: &[u8],
    ) -> Result<Self, ProjectError> {
        let chunks = bytes.chunks_exact(element.byte_size());
        if !chunks.remainder().is_empty() {
            return Err(ProjectError::new(
                "Onda buffer payload is not aligned to its element type",
            ));
        }
        match element {
            BufferElement::Bool => {
                if bytes.iter().any(|value| *value > 1) {
                    return Err(ProjectError::new(
                        "Onda bool buffer payload contains a value other than 0 or 1",
                    ));
                }
                Ok(Self::Bool(bytes.to_vec()))
            }
            BufferElement::I32 => Ok(Self::I32(
                bytes
                    .chunks_exact(4)
                    .map(|chunk| i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect(),
            )),
            BufferElement::I64 => Ok(Self::I64(
                bytes
                    .chunks_exact(8)
                    .map(|chunk| {
                        i64::from_le_bytes([
                            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6],
                            chunk[7],
                        ])
                    })
                    .collect(),
            )),
            BufferElement::F32 => Ok(Self::F32(
                bytes
                    .chunks_exact(4)
                    .map(|chunk| {
                        f32::from_bits(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    })
                    .collect(),
            )),
            BufferElement::F64 => Ok(Self::F64(
                bytes
                    .chunks_exact(8)
                    .map(|chunk| {
                        f64::from_bits(u64::from_le_bytes([
                            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6],
                            chunk[7],
                        ]))
                    })
                    .collect(),
            )),
        }
    }

    pub fn canonical_le_bytes(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(self.len().saturating_mul(self.element().byte_size()));
        self.append_canonical_le_bytes(&mut output);
        output
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BufferAsset {
    pub frames: u32,
    pub channels: u32,
    pub sample_rate: f32,
    pub samples: BufferSamples,
}

/// Shape and storage information available without decoding sample payloads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BufferAssetMetadata {
    pub element: BufferElement,
    pub frames: u32,
    pub channels: u32,
    pub sample_rate: f32,
    pub payload_bytes: usize,
}

impl BufferAsset {
    pub fn new(
        frames: u32,
        channels: u32,
        sample_rate: f32,
        samples: BufferSamples,
    ) -> Result<Self, ProjectError> {
        let asset = Self {
            frames,
            channels,
            sample_rate,
            samples,
        };
        asset.validate(&ProjectLimits::default())?;
        Ok(asset)
    }

    pub fn element(&self) -> BufferElement {
        self.samples.element()
    }

    pub fn payload_bytes(&self) -> usize {
        self.samples
            .len()
            .saturating_mul(self.element().byte_size())
    }

    pub fn metadata(&self) -> BufferAssetMetadata {
        BufferAssetMetadata {
            element: self.element(),
            frames: self.frames,
            channels: self.channels,
            sample_rate: self.sample_rate,
            payload_bytes: self.payload_bytes(),
        }
    }

    pub fn validate(&self, limits: &ProjectLimits) -> Result<(), ProjectError> {
        validate_buffer_shape(
            self.element(),
            self.frames,
            self.channels,
            self.sample_rate,
            self.samples.len(),
            limits,
        )?;
        if let BufferSamples::Bool(values) = &self.samples {
            if values.iter().any(|value| *value > 1) {
                return Err(ProjectError::new(
                    "Onda bool buffer values must be exactly 0 or 1",
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_payload(&self) -> Vec<u8> {
        self.samples.canonical_le_bytes()
    }

    pub(crate) fn append_canonical_payload(&self, output: &mut Vec<u8>) {
        self.samples.append_canonical_le_bytes(output);
    }

    pub(crate) fn content_digest(&self) -> [u8; 32] {
        let mut hasher =
            buffer_content_hasher(self.element(), self.frames, self.channels, self.sample_rate);
        self.samples
            .visit_canonical_le_bytes(|bytes| hasher.update(bytes));
        hasher.finalize().into()
    }
}

/// A fully validated zero-copy view of a canonical `.ondabuffer` file.
///
/// Hosts can inspect the content identity and enforce aggregate budgets before
/// allocating the decoded sample vector.
#[derive(Debug)]
pub struct ValidatedOndabuffer<'a> {
    element: BufferElement,
    frames: u32,
    channels: u32,
    sample_rate: f32,
    payload: &'a [u8],
    content_digest: [u8; 32],
}

impl ValidatedOndabuffer<'_> {
    pub fn content_digest(&self) -> [u8; 32] {
        self.content_digest
    }

    pub fn payload_bytes(&self) -> usize {
        self.payload.len()
    }

    pub fn decode(self) -> Result<BufferAsset, ProjectError> {
        Ok(BufferAsset {
            frames: self.frames,
            channels: self.channels,
            sample_rate: self.sample_rate,
            samples: BufferSamples::from_canonical_le_bytes(self.element, self.payload)?,
        })
    }

    pub fn decode_with_remaining_asset_budget(
        self,
        limits: ProjectLimits,
        allocated_asset_bytes: usize,
    ) -> Result<BufferAsset, ProjectError> {
        let remaining = limits
            .with_remaining_asset_budget(allocated_asset_bytes)
            .max_asset_bytes;
        if self.payload_bytes() > remaining {
            return Err(ProjectError::new(format!(
                "Onda buffer payload has {} bytes, exceeding the {remaining} byte remaining asset budget",
                self.payload_bytes()
            )));
        }
        self.decode()
    }
}

pub fn encode_ondabuffer(asset: &BufferAsset) -> Result<Vec<u8>, ProjectError> {
    asset.validate(&ProjectLimits::default())?;
    let payload_bytes = asset.payload_bytes();
    let payload_len = u64::try_from(payload_bytes)
        .map_err(|_| ProjectError::new("Onda buffer payload length does not fit u64"))?;
    let mut output = Vec::with_capacity(ONDA_BUFFER_HEADER_BYTES.saturating_add(payload_bytes));
    output.extend_from_slice(ONDA_BUFFER_MAGIC);
    output.extend_from_slice(&ONDA_BUFFER_FORMAT_VERSION.to_le_bytes());
    output.push(asset.element().tag());
    output.extend_from_slice(&[0, 0, 0]);
    output.extend_from_slice(&asset.frames.to_le_bytes());
    output.extend_from_slice(&asset.channels.to_le_bytes());
    output.extend_from_slice(&asset.sample_rate.to_bits().to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    output.extend_from_slice(&asset.content_digest());
    asset.append_canonical_payload(&mut output);
    Ok(output)
}

pub fn decode_ondabuffer(bytes: &[u8], limits: ProjectLimits) -> Result<BufferAsset, ProjectError> {
    validate_ondabuffer(bytes, limits)?.decode()
}

/// Validates a canonical `.ondabuffer` without allocating its decoded sample
/// storage.
pub fn validate_ondabuffer(
    bytes: &[u8],
    limits: ProjectLimits,
) -> Result<ValidatedOndabuffer<'_>, ProjectError> {
    let header = parse_ondabuffer_header(bytes, bytes.len(), &limits)?;
    let payload = &bytes[ONDA_BUFFER_HEADER_BYTES..];
    if header.metadata.element == BufferElement::Bool && payload.iter().any(|value| *value > 1) {
        return Err(ProjectError::new(
            "Onda bool buffer payload contains a value other than 0 or 1",
        ));
    }
    let mut hasher = buffer_content_hasher(
        header.metadata.element,
        header.metadata.frames,
        header.metadata.channels,
        header.metadata.sample_rate,
    );
    hasher.update(payload);
    let content_digest: [u8; 32] = hasher.finalize().into();
    if header.expected_digest != content_digest {
        return Err(ProjectError::new("Onda buffer content digest mismatch"));
    }
    Ok(ValidatedOndabuffer {
        element: header.metadata.element,
        frames: header.metadata.frames,
        channels: header.metadata.channels,
        sample_rate: header.metadata.sample_rate,
        payload,
        content_digest,
    })
}

struct OndabufferHeader {
    metadata: BufferAssetMetadata,
    expected_digest: [u8; 32],
}

fn parse_ondabuffer_header(
    bytes: &[u8],
    file_len: usize,
    limits: &ProjectLimits,
) -> Result<OndabufferHeader, ProjectError> {
    if bytes.len() < ONDA_BUFFER_HEADER_BYTES || &bytes[..8] != ONDA_BUFFER_MAGIC {
        return Err(ProjectError::new("file is not an Onda buffer asset"));
    }
    let version = read_u32(bytes, 8)?;
    if version != ONDA_BUFFER_FORMAT_VERSION {
        return Err(ProjectError::new(format!(
            "unsupported Onda buffer format version {version}"
        )));
    }
    if bytes[13..16] != [0, 0, 0] {
        return Err(ProjectError::new(
            "Onda buffer header contains nonzero reserved bytes",
        ));
    }
    let element = BufferElement::from_tag(bytes[12])?;
    let frames = read_u32(bytes, 16)?;
    let channels = read_u32(bytes, 20)?;
    let sample_rate = f32::from_bits(read_u32(bytes, 24)?);
    let payload_len_u64 = read_u64(bytes, 28)?;
    let payload_len = usize::try_from(payload_len_u64)
        .map_err(|_| ProjectError::new("Onda buffer payload length does not fit this host"))?;
    if payload_len > limits.max_asset_bytes {
        return Err(ProjectError::new(format!(
            "Onda buffer payload exceeds the {} byte limit",
            limits.max_asset_bytes
        )));
    }
    let expected_total = ONDA_BUFFER_HEADER_BYTES
        .checked_add(payload_len)
        .ok_or_else(|| ProjectError::new("Onda buffer file length overflows"))?;
    if file_len != expected_total {
        return Err(ProjectError::new(format!(
            "Onda buffer file declares {payload_len} payload bytes but has {}",
            file_len.saturating_sub(ONDA_BUFFER_HEADER_BYTES)
        )));
    }
    if !payload_len.is_multiple_of(element.byte_size()) {
        return Err(ProjectError::new(
            "Onda buffer payload is not aligned to its element type",
        ));
    }
    let element_count = payload_len / element.byte_size();
    validate_buffer_shape(
        element,
        frames,
        channels,
        sample_rate,
        element_count,
        limits,
    )?;
    let mut expected_digest = [0_u8; 32];
    expected_digest.copy_from_slice(&bytes[36..68]);
    Ok(OndabufferHeader {
        metadata: BufferAssetMetadata {
            element,
            frames,
            channels,
            sample_rate,
            payload_bytes: payload_len,
        },
        expected_digest,
    })
}

/// Inspects a supported buffer file without decoding or hashing its samples.
///
/// This validates the encoded container, declared shape, supported sample
/// representation, and project resource limits. Full payload integrity remains
/// the responsibility of [`load_buffer_file`] when the asset is used.
pub fn inspect_buffer_file(
    path: impl AsRef<Path>,
    limits: ProjectLimits,
) -> Result<BufferAssetMetadata, ProjectError> {
    let path = path.as_ref();
    let file_metadata = fs::metadata(path).map_err(|error| {
        ProjectError::new(format!(
            "failed to inspect buffer asset '{}': {error}",
            path.display()
        ))
    })?;
    if !file_metadata.is_file() {
        return Err(ProjectError::new(format!(
            "buffer asset '{}' is not a file",
            path.display()
        )));
    }
    let file_len = usize::try_from(file_metadata.len())
        .map_err(|_| ProjectError::new("buffer asset file length does not fit this host"))?;
    let encoded_limit = limits
        .max_asset_bytes
        .saturating_add(ONDA_BUFFER_HEADER_BYTES);
    if file_len > encoded_limit {
        return Err(ProjectError::new(format!(
            "buffer asset '{}' has {file_len} encoded bytes, exceeding the {encoded_limit} byte limit",
            path.display()
        )));
    }

    let mut file = fs::File::open(path).map_err(|error| {
        ProjectError::new(format!(
            "failed to open buffer asset '{}': {error}",
            path.display()
        ))
    })?;
    let mut prefix = [0_u8; ONDA_BUFFER_HEADER_BYTES];
    file.read_exact(&mut prefix[..ONDA_BUFFER_MAGIC.len()])
        .map_err(|error| {
            ProjectError::new(format!(
                "failed to read buffer asset '{}': {error}",
                path.display()
            ))
        })?;
    if prefix[..ONDA_BUFFER_MAGIC.len()] == *ONDA_BUFFER_MAGIC {
        file.read_exact(&mut prefix[ONDA_BUFFER_MAGIC.len()..])
            .map_err(|error| {
                ProjectError::new(format!(
                    "failed to read Onda buffer header '{}': {error}",
                    path.display()
                ))
            })?;
        return Ok(parse_ondabuffer_header(&prefix, file_len, &limits)?.metadata);
    }

    inspect_wav(path, limits)
}

fn inspect_wav(path: &Path, limits: ProjectLimits) -> Result<BufferAssetMetadata, ProjectError> {
    let reader = hound::WavReader::open(path).map_err(|error| {
        ProjectError::new(format!(
            "unsupported buffer file '{}': expected .ondabuffer or WAV ({error})",
            path.display()
        ))
    })?;
    let spec = reader.spec();
    let channels = u32::from(spec.channels);
    if channels == 0 {
        return Err(ProjectError::new(format!(
            "WAV '{}' has zero channels",
            path.display()
        )));
    }
    if !matches!(
        (spec.sample_format, spec.bits_per_sample),
        (hound::SampleFormat::Float, 32) | (hound::SampleFormat::Int, 8 | 16 | 24 | 32)
    ) {
        return Err(ProjectError::new(format!(
            "unsupported WAV encoding in '{}': {:?} {} bits",
            path.display(),
            spec.sample_format,
            spec.bits_per_sample
        )));
    }
    let sample_count = usize::try_from(reader.len())
        .map_err(|_| ProjectError::new("WAV sample count does not fit this host"))?;
    if sample_count == 0 || !sample_count.is_multiple_of(channels as usize) {
        return Err(ProjectError::new(format!(
            "WAV '{}' contains invalid interleaved sample data",
            path.display()
        )));
    }
    let frames = u32::try_from(sample_count / channels as usize)
        .map_err(|_| ProjectError::new("WAV frame count exceeds u32"))?;
    validate_buffer_shape(
        BufferElement::F32,
        frames,
        channels,
        spec.sample_rate as f32,
        sample_count,
        &limits,
    )?;
    Ok(BufferAssetMetadata {
        element: BufferElement::F32,
        frames,
        channels,
        sample_rate: spec.sample_rate as f32,
        payload_bytes: sample_count * std::mem::size_of::<f32>(),
    })
}

fn validate_buffer_shape(
    element: BufferElement,
    frames: u32,
    channels: u32,
    sample_rate: f32,
    element_count: usize,
    limits: &ProjectLimits,
) -> Result<(), ProjectError> {
    if frames == 0 || frames > i32::MAX as u32 {
        return Err(ProjectError::new(
            "Onda buffer frames must be between 1 and i32::MAX",
        ));
    }
    if channels == 0 || channels > i32::MAX as u32 {
        return Err(ProjectError::new(
            "Onda buffer channels must be between 1 and i32::MAX",
        ));
    }
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(ProjectError::new(
            "Onda buffer sample rate must be finite and greater than zero",
        ));
    }
    let expected = checked_product(&[frames as usize, channels as usize], "Onda buffer element")?;
    if element_count != expected {
        return Err(ProjectError::new(format!(
            "Onda buffer shape requires {expected} elements, got {element_count}"
        )));
    }
    let payload_bytes = checked_product(&[expected, element.byte_size()], "Onda buffer payload")?;
    if payload_bytes > limits.max_asset_bytes {
        return Err(ProjectError::new(format!(
            "Onda buffer payload has {payload_bytes} bytes, exceeding the {} byte limit",
            limits.max_asset_bytes
        )));
    }
    Ok(())
}

fn buffer_content_hasher(
    element: BufferElement,
    frames: u32,
    channels: u32,
    sample_rate: f32,
) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(b"onda-buffer-asset-v1\0");
    hasher.update([element.tag()]);
    hasher.update(frames.to_le_bytes());
    hasher.update(channels.to_le_bytes());
    hasher.update(sample_rate.to_bits().to_le_bytes());
    hasher
}

pub fn load_buffer_file(
    path: impl AsRef<Path>,
    limits: ProjectLimits,
) -> Result<BufferAsset, ProjectError> {
    let path = path.as_ref();
    let encoded_limit = limits
        .max_asset_bytes
        .saturating_add(ONDA_BUFFER_HEADER_BYTES);
    let bytes = crate::read_bounded_file(path, encoded_limit, "buffer asset", "encoded-file")?;
    decode_buffer_bytes(&bytes, path, limits)
}

pub fn is_ondabuffer(bytes: &[u8]) -> bool {
    bytes.starts_with(ONDA_BUFFER_MAGIC)
}

pub fn decode_buffer_bytes(
    bytes: &[u8],
    path: impl AsRef<Path>,
    limits: ProjectLimits,
) -> Result<BufferAsset, ProjectError> {
    if is_ondabuffer(bytes) {
        return decode_ondabuffer(bytes, limits);
    }
    decode_wav(bytes, path.as_ref(), limits)
}

pub fn encode_wav_f32(asset: &BufferAsset) -> Result<Vec<u8>, ProjectError> {
    let BufferSamples::F32(samples) = &asset.samples else {
        return Err(ProjectError::new(
            "canonical WAV materialization requires an f32 buffer asset",
        ));
    };
    asset.validate(&ProjectLimits::default())?;
    let channels = u16::try_from(asset.channels)
        .map_err(|_| ProjectError::new("WAV channel count exceeds u16"))?;
    if asset.sample_rate.fract() != 0.0 || f64::from(asset.sample_rate) > f64::from(u32::MAX) {
        return Err(ProjectError::new(
            "WAV materialization requires an integer sample rate representable as u32",
        ));
    }
    let sample_rate = asset.sample_rate as u32;
    let data_bytes = samples
        .len()
        .checked_mul(4)
        .ok_or_else(|| ProjectError::new("WAV data size overflows"))?;
    let riff_size = 36usize
        .checked_add(data_bytes)
        .ok_or_else(|| ProjectError::new("WAV RIFF size overflows"))?;
    let data_bytes_u32 = u32::try_from(data_bytes)
        .map_err(|_| ProjectError::new("WAV data exceeds the RIFF 4 GiB limit"))?;
    let riff_size_u32 = u32::try_from(riff_size)
        .map_err(|_| ProjectError::new("WAV file exceeds the RIFF 4 GiB limit"))?;
    let block_align = channels
        .checked_mul(4)
        .ok_or_else(|| ProjectError::new("WAV block alignment overflows"))?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| ProjectError::new("WAV byte rate overflows"))?;

    let mut output = Vec::with_capacity(44usize.saturating_add(data_bytes));
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&riff_size_u32.to_le_bytes());
    output.extend_from_slice(b"WAVEfmt ");
    output.extend_from_slice(&16u32.to_le_bytes());
    output.extend_from_slice(&3u16.to_le_bytes());
    output.extend_from_slice(&channels.to_le_bytes());
    output.extend_from_slice(&sample_rate.to_le_bytes());
    output.extend_from_slice(&byte_rate.to_le_bytes());
    output.extend_from_slice(&block_align.to_le_bytes());
    output.extend_from_slice(&32u16.to_le_bytes());
    output.extend_from_slice(b"data");
    output.extend_from_slice(&data_bytes_u32.to_le_bytes());
    for sample in samples {
        output.extend_from_slice(&sample.to_bits().to_le_bytes());
    }
    Ok(output)
}

fn decode_wav(
    bytes: &[u8],
    path: &Path,
    limits: ProjectLimits,
) -> Result<BufferAsset, ProjectError> {
    let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(|error| {
        ProjectError::new(format!(
            "unsupported buffer file '{}': expected .ondabuffer or WAV ({error})",
            path.display()
        ))
    })?;
    let spec = reader.spec();
    let channels = u32::from(spec.channels);
    if channels == 0 {
        return Err(ProjectError::new(format!(
            "WAV '{}' has zero channels",
            path.display()
        )));
    }
    let sample_count = usize::try_from(reader.len())
        .map_err(|_| ProjectError::new("WAV sample count does not fit this host"))?;
    let decoded_bytes = sample_count
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| ProjectError::new("WAV decoded sample size overflows"))?;
    if decoded_bytes > limits.max_asset_bytes {
        return Err(ProjectError::new(format!(
            "WAV '{}' decodes to {decoded_bytes} bytes, exceeding the {} byte limit",
            path.display(),
            limits.max_asset_bytes
        )));
    }
    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ProjectError::new(format!("failed to read WAV samples: {error}")))?,
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|sample| sample.map(|value| value as f32 / 128.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ProjectError::new(format!("failed to read WAV samples: {error}")))?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| value as f32 / 32_768.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ProjectError::new(format!("failed to read WAV samples: {error}")))?,
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|sample| sample.map(|value| value as f32 / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ProjectError::new(format!("failed to read WAV samples: {error}")))?,
        (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|sample| sample.map(|value| value as f32 / 2_147_483_648.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ProjectError::new(format!("failed to read WAV samples: {error}")))?,
        _ => {
            return Err(ProjectError::new(format!(
                "unsupported WAV encoding in '{}': {:?} {} bits",
                path.display(),
                spec.sample_format,
                spec.bits_per_sample
            )))
        }
    };
    if samples.is_empty() || !samples.len().is_multiple_of(channels as usize) {
        return Err(ProjectError::new(format!(
            "WAV '{}' contains invalid interleaved sample data",
            path.display()
        )));
    }
    let frames = u32::try_from(samples.len() / channels as usize)
        .map_err(|_| ProjectError::new("WAV frame count exceeds u32"))?;
    let asset = BufferAsset {
        frames,
        channels,
        sample_rate: spec.sample_rate as f32,
        samples: BufferSamples::F32(samples),
    };
    asset.validate(&limits)?;
    Ok(asset)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ProjectError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| ProjectError::new("Onda buffer header offset overflows"))?;
    let Some(slice) = bytes.get(offset..end) else {
        return Err(ProjectError::new("Onda buffer header is truncated"));
    };
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ProjectError> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| ProjectError::new("Onda buffer header offset overflows"))?;
    let Some(slice) = bytes.get(offset..end) else {
        return Err(ProjectError::new("Onda buffer header is truncated"));
    };
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}
