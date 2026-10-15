//! Rewrites the ternary weights of Tencent's 1.25-bit Hy-MT2 GGUF for the bundled llama.cpp.
//!
//! The file stores its weights as STQ1_0, a type whose kernels exist only in an unmerged ARM
//! branch of llama.cpp. Each STQ1_0 block holds 256 weights of -1, 0 or +1 times one fp16
//! scale, which is what llama.cpp's TQ2_0 stores too, with x86 kernels. The rewrite is exact:
//! every weight keeps its value. Everything else in the file is copied unchanged.

use lt_core::error::{Error, Result};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// The type id that the published file uses for STQ1_0.
const STQ1_0: u32 = 42;
const TQ2_0: u32 = 35;
/// `general.file_type` of a file whose weights are TQ2_0.
const MOSTLY_TQ2_0: u32 = 37;
const BLOCK_WEIGHTS: usize = 256;
/// qs[32] (a 4-bit pattern code per group of 4 weights), sign[8] (a flip bit per group), d.
const STQ_BLOCK: usize = 42;
/// qs[64] (2 bits per weight), d.
const TQ2_BLOCK: usize = 66;
/// Each code is a group of 4 lanes with exactly one zero; 2 bits per lane, -1 -> 0, 0 -> 1,
/// +1 -> 2. The second half is the first with every non-zero lane negated.
const CODEBOOK: [u8; 32] = [
    0xA9, 0x89, 0x29, 0x09, 0xA6, 0x86, 0x26, 0x06, 0x9A, 0x92, 0x1A, 0x12, 0x6A, 0x62, 0x4A, 0x42,
    0x01, 0x21, 0x81, 0xA1, 0x04, 0x24, 0x84, 0xA4, 0x10, 0x18, 0x90, 0x98, 0x40, 0x48, 0x60, 0x68,
];

struct Tensor {
    name: Vec<u8>,
    dims: Vec<u64>,
    kind: u32,
    offset: u64,
}

/// Rewrites `source` into `output`. Checks `cancelled` between tensors.
pub(crate) fn convert(source: &Path, output: &Path, cancelled: &AtomicBool) -> Result<()> {
    let invalid = |what: &str| Error::Engine(format!("Ternary model file is not valid: {what}"));
    let mut input = BufReader::new(File::open(source)?);
    let file_len = input.get_ref().metadata()?.len();
    let mut header = Vec::new();
    let mut magic = [0_u8; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(invalid("no GGUF header"));
    }
    header.extend_from_slice(&magic);
    let version = read_u32(&mut input)?;
    let tensor_count = read_u64(&mut input)?;
    let kv_count = read_u64(&mut input)?;
    header.extend_from_slice(&version.to_le_bytes());
    header.extend_from_slice(&tensor_count.to_le_bytes());
    header.extend_from_slice(&kv_count.to_le_bytes());
    let mut alignment = 32_u64;
    let mut file_type_seen = false;
    for _ in 0..kv_count {
        let key = read_string(&mut input)?;
        let kind = read_u32(&mut input)?;
        let mut value = Vec::new();
        copy_value(&mut input, kind, &mut value)?;
        header.extend_from_slice(&(key.len() as u64).to_le_bytes());
        header.extend_from_slice(&key);
        header.extend_from_slice(&kind.to_le_bytes());
        match (key.as_slice(), kind) {
            (b"general.file_type", 4) => {
                header.extend_from_slice(&MOSTLY_TQ2_0.to_le_bytes());
                file_type_seen = true;
            }
            (b"general.alignment", 4) => {
                let bytes = value[..4].try_into().map_err(|_| invalid("alignment"))?;
                alignment = u64::from(u32::from_le_bytes(bytes));
                header.extend_from_slice(&value);
            }
            _ => header.extend_from_slice(&value),
        }
    }
    if !file_type_seen || alignment == 0 {
        return Err(invalid("no file type or alignment"));
    }
    let tensors = (0..tensor_count)
        .map(|_| {
            let name = read_string(&mut input)?;
            let dims = (0..read_u32(&mut input)?)
                .map(|_| read_u64(&mut input))
                .collect::<Result<Vec<_>>>()?;
            let kind = read_u32(&mut input)?;
            let offset = read_u64(&mut input)?;
            Ok(Tensor {
                name,
                dims,
                kind,
                offset,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let position = input.stream_position()?;
    let data_start = position.div_ceil(alignment) * alignment;
    // The bytes each tensor takes in the source (its data and padding) and in the output.
    let mut sizes = Vec::with_capacity(tensors.len());
    for (index, tensor) in tensors.iter().enumerate() {
        let end = tensors
            .get(index + 1)
            .map_or(file_len - data_start, |next| next.offset);
        let span = end
            .checked_sub(tensor.offset)
            .ok_or_else(|| invalid("tensor offsets out of order"))?;
        let size = if tensor.kind == STQ1_0 {
            let weights: u64 = tensor.dims.iter().product();
            if !weights.is_multiple_of(BLOCK_WEIGHTS as u64)
                || span < weights / BLOCK_WEIGHTS as u64 * STQ_BLOCK as u64
            {
                return Err(invalid("ternary tensor size"));
            }
            weights / BLOCK_WEIGHTS as u64 * TQ2_BLOCK as u64
        } else {
            span
        };
        sizes.push((span, size));
    }
    let mut offset = 0_u64;
    for (tensor, (_, size)) in tensors.iter().zip(&sizes) {
        header.extend_from_slice(&(tensor.name.len() as u64).to_le_bytes());
        header.extend_from_slice(&tensor.name);
        header.extend_from_slice(&(tensor.dims.len() as u32).to_le_bytes());
        for dim in &tensor.dims {
            header.extend_from_slice(&dim.to_le_bytes());
        }
        let kind = if tensor.kind == STQ1_0 {
            TQ2_0
        } else {
            tensor.kind
        };
        header.extend_from_slice(&kind.to_le_bytes());
        header.extend_from_slice(&offset.to_le_bytes());
        offset = (offset + size).div_ceil(alignment) * alignment;
    }
    let mut writer = BufWriter::new(File::create(output)?);
    write_padded(&mut writer, &header, alignment)?;
    input.seek(SeekFrom::Start(data_start))?;
    for (tensor, (span, size)) in tensors.iter().zip(&sizes) {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        if tensor.kind == STQ1_0 {
            let mut data = vec![0_u8; *span as usize];
            input.read_exact(&mut data)?;
            let mut converted = Vec::with_capacity(*size as usize);
            for block in data
                .as_chunks::<STQ_BLOCK>()
                .0
                .iter()
                .take(*size as usize / TQ2_BLOCK)
            {
                converted.extend_from_slice(&tq2_block(block));
            }
            write_padded(&mut writer, &converted, alignment)?;
        } else {
            // The embedding table alone is about 200 MB: copy it without holding it.
            let copied = std::io::copy(&mut (&mut input).take(*span), &mut writer)?;
            if copied != *span {
                return Err(invalid("file ends early"));
            }
            pad(&mut writer, *span, alignment)?;
        }
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

/// One STQ1_0 block as a TQ2_0 block with the same weights.
fn tq2_block(stq: &[u8; STQ_BLOCK]) -> [u8; TQ2_BLOCK] {
    let d = u16::from_le_bytes([stq[40], stq[41]]);
    let magnitude = d & 0x7FFF;
    // TQ2_0's scale is the largest magnitude, so a negative STQ1_0 scale flips each weight.
    let negative = d & 0x8000 != 0 && magnitude != 0;
    let mut tq = [0_u8; TQ2_BLOCK];
    for group in 0..BLOCK_WEIGHTS / 4 {
        let code = (stq[group / 2] >> (4 * (group & 1))) & 0x0F;
        let sign = (stq[32 + group / 8] >> (group % 8)) & 1;
        let pattern = CODEBOOK[usize::from(sign << 4 | code)];
        let (chunk, slot) = (group / 16, group % 16);
        for lane in 0..4 {
            let value = (pattern >> (2 * lane)) & 3;
            let value = match (magnitude, negative) {
                (0, _) => 1,
                (_, true) => 2 - value,
                (_, false) => value,
            };
            // Weight w of TQ2_0 sits at byte (w / 128) * 32 + w % 32, bits 2 * ((w % 128) / 32).
            let weight = chunk * 64 + slot + lane * 16;
            tq[(weight / 128) * 32 + weight % 32] |= value << (2 * ((weight % 128) / 32));
        }
    }
    tq[64..].copy_from_slice(&magnitude.to_le_bytes());
    tq
}

fn write_padded(writer: &mut impl Write, data: &[u8], alignment: u64) -> Result<()> {
    writer.write_all(data)?;
    pad(writer, data.len() as u64, alignment)
}

/// Zeros after `len` bytes up to the next multiple of `alignment`.
fn pad(writer: &mut impl Write, len: u64, alignment: u64) -> Result<()> {
    let padding = (alignment - len % alignment) % alignment;
    writer.write_all(&vec![0; padding as usize])?;
    Ok(())
}

fn read_u32(input: &mut impl Read) -> Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut impl Read) -> Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_string(input: &mut impl Read) -> Result<Vec<u8>> {
    let length = read_u64(input)?;
    let mut bytes =
        vec![0; usize::try_from(length).map_err(|_| Error::Engine("GGUF string too long".into()))?];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Copies one metadata value of GGUF type `kind` into `out`, as stored.
fn copy_value(input: &mut impl Read, kind: u32, out: &mut Vec<u8>) -> Result<()> {
    let fixed = match kind {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        8 => {
            let length = read_u64(input)?;
            out.extend_from_slice(&length.to_le_bytes());
            return copy_bytes(input, length, out);
        }
        9 => {
            let inner = read_u32(input)?;
            let count = read_u64(input)?;
            out.extend_from_slice(&inner.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            for _ in 0..count {
                copy_value(input, inner, out)?;
            }
            return Ok(());
        }
        _ => {
            return Err(Error::Engine(format!(
                "GGUF metadata type {kind} is unknown"
            )))
        }
    };
    copy_bytes(input, fixed, out)
}

fn copy_bytes(input: &mut impl Read, count: u64, out: &mut Vec<u8>) -> Result<()> {
    let start = out.len();
    out.resize(
        start + usize::try_from(count).map_err(|_| Error::Engine("GGUF value too long".into()))?,
        0,
    );
    input.read_exact(&mut out[start..])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The weights STQ1_0 stores, as llama.cpp's reference decoder reads them.
    fn stq_weights(stq: &[u8; STQ_BLOCK]) -> [i8; BLOCK_WEIGHTS] {
        let mut weights = [0; BLOCK_WEIGHTS];
        for group in 0..64 {
            let code = (stq[group / 2] >> (4 * (group & 1))) & 0x0F;
            let sign = (stq[32 + group / 8] >> (group % 8)) & 1;
            let pattern = CODEBOOK[usize::from(sign << 4 | code)];
            for lane in 0..4 {
                weights[(group / 16) * 64 + group % 16 + lane * 16] =
                    ((pattern >> (2 * lane)) & 3) as i8 - 1;
            }
        }
        weights
    }

    /// The weights TQ2_0 stores, as llama.cpp's reference decoder reads them.
    fn tq2_weights(tq: &[u8; TQ2_BLOCK]) -> [i8; BLOCK_WEIGHTS] {
        let mut weights = [0; BLOCK_WEIGHTS];
        for chunk in 0..2 {
            for shift in 0..4 {
                for m in 0..32 {
                    weights[chunk * 128 + shift * 32 + m] =
                        ((tq[chunk * 32 + m] >> (2 * shift)) & 3) as i8 - 1;
                }
            }
        }
        weights
    }

    /// A silent mistake in the bit layout would give a model that loads and writes nonsense.
    #[test]
    fn every_weight_and_the_scale_keep_their_value() {
        let mut seed = 0x2545_f491_u32;
        for scale in [0x2E66_u16, 0xAE66, 0x0000] {
            let mut stq = [0_u8; STQ_BLOCK];
            for byte in &mut stq[..40] {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            stq[40..].copy_from_slice(&scale.to_le_bytes());
            let tq = tq2_block(&stq);
            let sign: i8 = if scale & 0x8000 != 0 { -1 } else { 1 };
            let expected =
                stq_weights(&stq).map(|w| if scale & 0x7FFF == 0 { 0 } else { w * sign });
            assert_eq!(tq2_weights(&tq), expected, "scale {scale:#06x}");
            assert_eq!(u16::from_le_bytes([tq[64], tq[65]]), scale & 0x7FFF);
        }
    }
}
