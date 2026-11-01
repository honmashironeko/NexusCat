#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PcmSampleFormat {
    U8,
    I16,
    I24,
    I32,
    F32,
}

impl PcmSampleFormat {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) const fn bytes_per_sample(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::I16 => 2,
            Self::I24 => 3,
            Self::I32 | Self::F32 => 4,
        }
    }
}

pub(crate) fn decode_to_i16(bytes: &[u8], format: PcmSampleFormat) -> Vec<i16> {
    match format {
        PcmSampleFormat::U8 => bytes
            .iter()
            .map(|byte| (i16::from(*byte) - 128) << 8)
            .collect(),
        PcmSampleFormat::I16 => bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect(),
        PcmSampleFormat::I24 => bytes
            .chunks_exact(3)
            .map(|chunk| {
                let value =
                    i32::from(chunk[0]) | (i32::from(chunk[1]) << 8) | (i32::from(chunk[2]) << 16);
                let signed = (value << 8) >> 8;
                (signed >> 8) as i16
            })
            .collect(),
        PcmSampleFormat::I32 => bytes
            .chunks_exact(4)
            .map(|chunk| {
                let value = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                (value >> 16) as i16
            })
            .collect(),
        PcmSampleFormat::F32 => bytes
            .chunks_exact(4)
            .map(|chunk| {
                let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                f32_to_i16(value)
            })
            .collect(),
    }
}

pub(crate) fn map_channels(
    samples: &[i16],
    source_channels: u16,
    target_channels: u16,
) -> Vec<i16> {
    let source_count = source_channels as usize;
    let target_count = target_channels as usize;
    if source_count == 0 || target_count == 0 {
        return Vec::new();
    }

    let frame_count = samples.len() / source_count;
    if source_count == target_count {
        return samples[..frame_count * source_count].to_vec();
    }

    let mut mapped = Vec::with_capacity(frame_count * target_count);
    for frame in 0..frame_count {
        let base = frame * source_count;
        for channel in 0..target_count {
            let value = if source_count == 1 {
                samples[base]
            } else if target_count == 1 {
                ((i32::from(samples[base]) + i32::from(samples[base + 1])) / 2) as i16
            } else if channel < source_count {
                samples[base + channel]
            } else {
                samples[base + channel % source_count]
            };
            mapped.push(value);
        }
    }
    mapped
}

pub(crate) fn resample_linear(
    samples: &[i16],
    channels: u16,
    source_rate: u32,
    target_rate: u32,
) -> Vec<i16> {
    let channel_count = channels as usize;
    if channel_count == 0 || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }

    let source_frames = samples.len() / channel_count;
    if source_frames == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return samples[..source_frames * channel_count].to_vec();
    }

    let target_frames = ((source_frames as u64 * u64::from(target_rate)
        + u64::from(source_rate) / 2)
        / u64::from(source_rate)) as usize;
    let ratio = f64::from(source_rate) / f64::from(target_rate);
    let mut output = Vec::with_capacity(target_frames * channel_count);

    for frame in 0..target_frames {
        let position = frame as f64 * ratio;
        let lower = (position.floor() as usize).min(source_frames - 1);
        let upper = (lower + 1).min(source_frames - 1);
        let fraction = position - lower as f64;
        for channel in 0..channel_count {
            let first = f64::from(samples[lower * channel_count + channel]);
            let second = f64::from(samples[upper * channel_count + channel]);
            let interpolated = first + (second - first) * fraction;
            output.push(interpolated.round().clamp(-32768.0, 32767.0) as i16);
        }
    }
    output
}

pub(crate) fn convert_to_target(
    bytes: &[u8],
    format: PcmSampleFormat,
    source_rate: u32,
    source_channels: u16,
    target_rate: u32,
    target_channels: u16,
) -> Vec<i16> {
    let decoded = decode_to_i16(bytes, format);
    let mapped = map_channels(&decoded, source_channels, target_channels);
    resample_linear(&mapped, target_channels, source_rate, target_rate)
}

fn f32_to_i16(value: f32) -> i16 {
    if !value.is_finite() {
        return 0;
    }
    (value.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

