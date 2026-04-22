fn encode_planar_f32(channels: &[Vec<f32>]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in channels {
        for sample in ch {
            out.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    out
}

fn encode_planar_f64(channels: &[Vec<f64>]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in channels {
        for sample in ch {
            out.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    out
}

fn encode_planar_i64(channels: &[Vec<i64>]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in channels {
        for sample in ch {
            out.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    out
}

fn decode_planar_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().expect("chunk");
            f32::from_ne_bytes(arr)
        })
        .collect()
}

fn decode_planar_f64(bytes: &[u8]) -> Vec<f64> {
    bytes
        .chunks_exact(std::mem::size_of::<f64>())
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().expect("chunk");
            f64::from_ne_bytes(arr)
        })
        .collect()
}

fn decode_planar_i64(bytes: &[u8]) -> Vec<i64> {
    bytes
        .chunks_exact(std::mem::size_of::<i64>())
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().expect("chunk");
            i64::from_ne_bytes(arr)
        })
        .collect()
}

fn read_wav_mixdown_f32(path: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("wav should open");
    let spec = reader.spec();
    let channels = spec.channels as usize;
    assert!(channels > 0, "wav must contain at least one channel");

    let interleaved = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .expect("float wav samples"),
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|s| s.map(|v| v as f32 / i8::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int8 wav samples"),
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int16 wav samples"),
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()
            .expect("int24 wav samples"),
        (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / i32::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int32 wav samples"),
        _ => panic!(
            "unsupported wav format: {:?} {} bits",
            spec.sample_format, spec.bits_per_sample
        ),
    };

    if channels == 1 {
        return interleaved;
    }

    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct FlatIoDesc {
    elem_ty: PrimitiveType,
    array_len: usize,
    offset: usize,
    elem_bytes: usize,
    entry_bytes: usize,
}

fn process_interleaved(
    instance: &mut onda_runtime::Instance,
    in_interleaved: &[f32],
    out_interleaved: &mut [f32],
    frames: usize,
) -> Result<(), Diagnostic> {
    let in_descs = collect_flat_io_descs(
        instance.input_count(),
        |idx| instance.input_type(idx),
        |idx| instance.input_type_bytes(idx),
    )?;
    let out_descs = collect_flat_io_descs(
        instance.output_count(),
        |idx| instance.output_type(idx),
        |idx| instance.output_type_bytes(idx),
    )?;
    let in_channels: usize = in_descs.iter().map(|d| d.array_len).sum();
    let out_channels: usize = out_descs.iter().map(|d| d.array_len).sum();

    let expected_in = frames.saturating_mul(in_channels);
    if in_interleaved.len() < expected_in {
        return Err(Diagnostic::runtime(
            "input buffer too small for requested frame count",
            0,
            0,
        ));
    }
    let expected_out = frames.saturating_mul(out_channels);
    if out_interleaved.len() < expected_out {
        return Err(Diagnostic::runtime(
            "output buffer too small for requested frame count",
            0,
            0,
        ));
    }

    let mut in_buffers = Vec::with_capacity(in_descs.len());
    for (idx, desc) in in_descs.iter().copied().enumerate() {
        let mut bytes = vec![0_u8; desc.entry_bytes.saturating_mul(frames)];
        for ch in 0..desc.array_len {
            let in_channel = desc.offset + ch;
            for frame in 0..frames {
                let sample = in_interleaved[frame * in_channels + in_channel];
                let byte_idx = (ch * frames + frame) * desc.elem_bytes;
                encode_f32_as_primitive(
                    desc.elem_ty,
                    sample,
                    &mut bytes[byte_idx..byte_idx + desc.elem_bytes],
                )?;
            }
        }
        bind_input(instance, idx, bytes.as_ptr(), bytes.len())?;
        in_buffers.push(bytes);
    }

    let mut out_buffers = Vec::with_capacity(out_descs.len());
    for (idx, desc) in out_descs.iter().copied().enumerate() {
        let mut bytes = vec![0_u8; desc.entry_bytes.saturating_mul(frames)];
        bind_output(instance, idx, bytes.as_mut_ptr(), bytes.len())?;
        out_buffers.push(bytes);
    }

    process_checked(instance, frames)?;

    for (idx, desc) in out_descs.iter().copied().enumerate() {
        let bytes = &out_buffers[idx];
        for ch in 0..desc.array_len {
            let out_channel = desc.offset + ch;
            for frame in 0..frames {
                let byte_idx = (ch * frames + frame) * desc.elem_bytes;
                let sample = decode_primitive_as_f32(
                    desc.elem_ty,
                    &bytes[byte_idx..byte_idx + desc.elem_bytes],
                )?;
                out_interleaved[frame * out_channels + out_channel] = sample;
            }
        }
    }
    Ok(())
}

fn benchmark_process_runtime(
    instance: &mut onda_runtime::Instance,
    in_interleaved: &[f32],
    out_interleaved: &mut [f32],
    frames: usize,
    warmup_iters: usize,
    timed_iters: usize,
) -> f64 {
    for _ in 0..warmup_iters {
        process_interleaved(instance, in_interleaved, out_interleaved, frames)
            .expect("warmup processing should succeed");
    }
    let start = Instant::now();
    for _ in 0..timed_iters {
        process_interleaved(instance, in_interleaved, out_interleaved, frames)
            .expect("timed processing should succeed");
    }
    std::hint::black_box(out_interleaved.first().copied().unwrap_or(0.0));
    start.elapsed().as_secs_f64()
}

fn estimate_positive_zero_cross_frequency(samples: &[f32], sample_rate: f32) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let crossings = samples
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count() as f32;
    crossings * sample_rate / samples.len() as f32
}

fn collect_flat_io_descs<TypeFn, BytesFn>(
    count: usize,
    mut type_of: TypeFn,
    mut bytes_of: BytesFn,
) -> Result<Vec<FlatIoDesc>, Diagnostic>
where
    TypeFn: FnMut(usize) -> Option<String>,
    BytesFn: FnMut(usize) -> Option<usize>,
{
    let mut out = Vec::with_capacity(count);
    let mut offset = 0usize;
    for idx in 0..count {
        let ty_text = type_of(idx).unwrap_or_else(|| "f32".to_owned());
        let (elem_ty, array_len) = parse_declared_type(&ty_text)?;
        let elem_bytes = primitive_type_bytes_local(elem_ty);
        let entry_bytes = bytes_of(idx).unwrap_or_else(|| elem_bytes.saturating_mul(array_len));
        out.push(FlatIoDesc {
            elem_ty,
            array_len,
            offset,
            elem_bytes,
            entry_bytes,
        });
        offset = offset.saturating_add(array_len);
    }
    Ok(out)
}

fn parse_declared_type(text: &str) -> Result<(PrimitiveType, usize), Diagnostic> {
    if let Some(bracket) = text.find('[') {
        if !text.ends_with(']') {
            return Err(Diagnostic::runtime("invalid declared type text", 0, 0));
        }
        let elem = &text[..bracket];
        let len_text = &text[bracket + 1..text.len() - 1];
        let len = len_text
            .parse::<usize>()
            .map_err(|_| Diagnostic::runtime("invalid declared array length", 0, 0))?;
        let ty = primitive_type_from_text(elem)?;
        Ok((ty, len.max(1)))
    } else {
        Ok((primitive_type_from_text(text)?, 1))
    }
}

fn primitive_type_from_text(text: &str) -> Result<PrimitiveType, Diagnostic> {
    match text {
        "f32" => Ok(PrimitiveType::F32),
        "f64" => Ok(PrimitiveType::F64),
        "i32" => Ok(PrimitiveType::I32),
        "i64" => Ok(PrimitiveType::I64),
        "bool" => Ok(PrimitiveType::Bool),
        _ => Err(Diagnostic::runtime(
            "unsupported declared primitive type",
            0,
            0,
        )),
    }
}

fn primitive_type_bytes_local(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn encode_f32_as_primitive(
    ty: PrimitiveType,
    value: f32,
    dst: &mut [u8],
) -> Result<(), Diagnostic> {
    match ty {
        PrimitiveType::F32 => {
            let out: &mut [u8; 4] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f32 destination width", 0, 0))?;
            out.copy_from_slice(&value.to_ne_bytes());
            Ok(())
        }
        PrimitiveType::F64 => {
            let out: &mut [u8; 8] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f64 destination width", 0, 0))?;
            out.copy_from_slice(&(value as f64).to_ne_bytes());
            Ok(())
        }
        PrimitiveType::I32 => {
            let out: &mut [u8; 4] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i32 destination width", 0, 0))?;
            out.copy_from_slice(&(value as i32).to_ne_bytes());
            Ok(())
        }
        PrimitiveType::I64 => {
            let out: &mut [u8; 8] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i64 destination width", 0, 0))?;
            out.copy_from_slice(&(value as i64).to_ne_bytes());
            Ok(())
        }
        PrimitiveType::Bool => {
            let out: &mut [u8; 1] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid bool destination width", 0, 0))?;
            out[0] = if value == 0.0 { 0 } else { 1 };
            Ok(())
        }
    }
}

fn decode_primitive_as_f32(ty: PrimitiveType, src: &[u8]) -> Result<f32, Diagnostic> {
    match ty {
        PrimitiveType::F32 => {
            let arr: [u8; 4] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f32 source width", 0, 0))?;
            Ok(f32::from_ne_bytes(arr))
        }
        PrimitiveType::F64 => {
            let arr: [u8; 8] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f64 source width", 0, 0))?;
            Ok(f64::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::I32 => {
            let arr: [u8; 4] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i32 source width", 0, 0))?;
            Ok(i32::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::I64 => {
            let arr: [u8; 8] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i64 source width", 0, 0))?;
            Ok(i64::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::Bool => {
            let b = *src
                .first()
                .ok_or_else(|| Diagnostic::runtime("invalid bool source width", 0, 0))?;
            Ok(if b == 0 { 0.0 } else { 1.0 })
        }
    }
}
