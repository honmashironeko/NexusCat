use nc_core::{AudioCodec, AudioFrame, NexusError, Result};

const OPUS_SAMPLE_RATE: u32 = 48_000;
#[cfg(any(windows, target_os = "macos"))]
const OPUS_FRAME_SAMPLES: usize = 960;
#[cfg(any(windows, target_os = "macos"))]
const OPUS_MAX_PACKET_BYTES: usize = 4096;
#[cfg(any(windows, target_os = "macos"))]
const OPUS_MAX_FRAME_SAMPLES: usize = 5_760;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioEncoderConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: u32,
}

impl Default for AudioEncoderConfig {
    fn default() -> Self {
        Self {
            sample_rate: OPUS_SAMPLE_RATE,
            channels: 2,
            bitrate: 64_000,
        }
    }
}

impl AudioEncoderConfig {
    pub fn validate(&self) -> Result<()> {
        validate_pcm_format(self.sample_rate, self.channels)?;
        if self.bitrate == 0 {
            return Err(NexusError::invalid("音频码率必须大于 0"));
        }
        Ok(())
    }
}

pub trait AudioEncoder: Send {
    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>>;
}

pub trait AudioDecoder: Send {
    fn decode(&mut self, packet: &[u8], timestamp_us: u64) -> Result<AudioFrame>;
}

pub fn create_audio_encoder(
    sample_rate: u32,
    channels: u16,
    bitrate: u32,
) -> (Box<dyn AudioEncoder>, AudioCodec) {
    #[cfg(any(windows, target_os = "macos"))]
    if sample_rate == OPUS_SAMPLE_RATE && channels == 2 {
        match OpusAudioEncoder::new(sample_rate, channels, bitrate) {
            Ok(encoder) => return (Box::new(encoder), AudioCodec::Opus),
            Err(error) => {
                tracing::warn!(%error, "Opus 编码器初始化失败，回退到 PCM");
            }
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    if sample_rate == OPUS_SAMPLE_RATE && channels == 2 {
        tracing::warn!("当前平台未启用 Opus 后端，回退到 PCM");
    }

    let encoder = PcmAudioEncoder::new_unchecked(sample_rate, channels);
    (Box::new(encoder), AudioCodec::PcmS16Le)
}

pub fn create_audio_decoder(
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
) -> Result<Box<dyn AudioDecoder>> {
    match codec {
        AudioCodec::PcmS16Le => Ok(Box::new(PcmAudioDecoder::new(sample_rate, channels)?)),
        AudioCodec::Opus => create_opus_decoder(sample_rate, channels),
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn create_opus_decoder(sample_rate: u32, channels: u16) -> Result<Box<dyn AudioDecoder>> {
    Ok(Box::new(OpusAudioDecoder::new(sample_rate, channels)?))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn create_opus_decoder(_sample_rate: u32, _channels: u16) -> Result<Box<dyn AudioDecoder>> {
    Err(NexusError::Media(
        "当前平台未启用 Opus 解码后端；请使用 Opus 不可用时的 PCM 回退".to_string(),
    ))
}

#[derive(Debug, Clone, Copy)]
pub struct PcmAudioEncoder {
    sample_rate: u32,
    channels: u16,
}

impl PcmAudioEncoder {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self> {
        validate_pcm_format(sample_rate, channels)?;
        Ok(Self::new_unchecked(sample_rate, channels))
    }

    fn new_unchecked(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }
}

impl AudioEncoder for PcmAudioEncoder {
    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>> {
        validate_frame_format(frame, self.sample_rate, self.channels)?;
        let byte_len = frame
            .samples
            .len()
            .checked_mul(2)
            .ok_or_else(|| NexusError::invalid("PCM 输出长度溢出"))?;
        let mut data = Vec::with_capacity(byte_len);
        for sample in &frame.samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(data)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PcmAudioDecoder {
    sample_rate: u32,
    channels: u16,
}

impl PcmAudioDecoder {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self> {
        validate_pcm_format(sample_rate, channels)?;
        Ok(Self {
            sample_rate,
            channels,
        })
    }
}

impl AudioDecoder for PcmAudioDecoder {
    fn decode(&mut self, packet: &[u8], timestamp_us: u64) -> Result<AudioFrame> {
        if packet.len() % 2 != 0 {
            return Err(NexusError::invalid("PCM 数据长度必须是 2 字节的整数倍"));
        }
        let sample_count = packet.len() / 2;
        if sample_count % self.channels as usize != 0 {
            return Err(NexusError::invalid("PCM 采样数不能被声道数整除"));
        }
        let mut samples = Vec::with_capacity(sample_count);
        for chunk in packet.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(AudioFrame {
            sample_rate: self.sample_rate,
            channels: self.channels,
            timestamp_us,
            samples,
        })
    }
}

#[cfg(any(windows, target_os = "macos"))]
#[derive(Debug)]
pub struct OpusAudioEncoder {
    encoder: opus::Encoder,
    channels: u16,
}

#[cfg(any(windows, target_os = "macos"))]
impl OpusAudioEncoder {
    pub fn new(sample_rate: u32, channels: u16, bitrate: u32) -> Result<Self> {
        validate_opus_format(sample_rate, channels)?;
        let opus_channels = opus_channels(channels)?;
        let bitrate =
            i32::try_from(bitrate).map_err(|_| NexusError::invalid("Opus 码率超出 i32 范围"))?;
        if bitrate <= 0 {
            return Err(NexusError::invalid("Opus 码率必须大于 0"));
        }

        let mut encoder =
            opus::Encoder::new(sample_rate, opus_channels, opus::Application::LowDelay)
                .map_err(|error| opus_error("创建编码器", error))?;
        encoder
            .set_bitrate(opus::Bitrate::Bits(bitrate))
            .map_err(|error| opus_error("设置码率", error))?;
        encoder
            .set_complexity(5)
            .map_err(|error| opus_error("设置复杂度", error))?;
        Ok(Self { encoder, channels })
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl AudioEncoder for OpusAudioEncoder {
    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>> {
        validate_frame_format(frame, OPUS_SAMPLE_RATE, self.channels)?;
        if frame.samples.len() != OPUS_FRAME_SAMPLES * self.channels as usize {
            return Err(NexusError::invalid(format!(
                "Opus 编码要求 20ms（{OPUS_FRAME_SAMPLES} 样本/声道）的整帧"
            )));
        }
        self.encoder
            .encode_vec(&frame.samples, OPUS_MAX_PACKET_BYTES)
            .map_err(|error| opus_error("编码", error))
    }
}

#[cfg(any(windows, target_os = "macos"))]
#[derive(Debug)]
pub struct OpusAudioDecoder {
    decoder: opus::Decoder,
    sample_rate: u32,
    channels: u16,
}

#[cfg(any(windows, target_os = "macos"))]
impl OpusAudioDecoder {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self> {
        validate_opus_format(sample_rate, channels)?;
        let decoder = opus::Decoder::new(sample_rate, opus_channels(channels)?)
            .map_err(|error| opus_error("创建解码器", error))?;
        Ok(Self {
            decoder,
            sample_rate,
            channels,
        })
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl AudioDecoder for OpusAudioDecoder {
    fn decode(&mut self, packet: &[u8], timestamp_us: u64) -> Result<AudioFrame> {
        let mut samples = vec![0_i16; OPUS_MAX_FRAME_SAMPLES * self.channels as usize];
        let samples_per_channel = self
            .decoder
            .decode(packet, &mut samples, false)
            .map_err(|error| opus_error("解码", error))?;
        samples.truncate(samples_per_channel * self.channels as usize);
        Ok(AudioFrame {
            sample_rate: self.sample_rate,
            channels: self.channels,
            timestamp_us,
            samples,
        })
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn opus_channels(channels: u16) -> Result<opus::Channels> {
    match channels {
        1 => Ok(opus::Channels::Mono),
        2 => Ok(opus::Channels::Stereo),
        _ => Err(NexusError::invalid("Opus 仅支持单声道或双声道")),
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn opus_error(stage: &str, error: opus::Error) -> NexusError {
    NexusError::Media(format!(
        "Opus {stage}失败：{}（{:?}）",
        error.description(),
        error.code()
    ))
}

fn validate_pcm_format(sample_rate: u32, channels: u16) -> Result<()> {
    if sample_rate == 0 {
        return Err(NexusError::invalid("采样率必须大于 0"));
    }
    if channels == 0 {
        return Err(NexusError::invalid("声道数必须大于 0"));
    }
    Ok(())
}

fn validate_frame_format(frame: &AudioFrame, sample_rate: u32, channels: u16) -> Result<()> {
    if channels == 0 {
        return Err(NexusError::invalid("声道数必须大于 0"));
    }
    if frame.sample_rate != sample_rate {
        return Err(NexusError::invalid(format!(
            "音频帧采样率 {} 与编码器 {} 不一致",
            frame.sample_rate, sample_rate
        )));
    }
    if frame.channels != channels {
        return Err(NexusError::invalid(format!(
            "音频帧声道数 {} 与编码器 {} 不一致",
            frame.channels, channels
        )));
    }
    if frame.samples.len() % channels as usize != 0 {
        return Err(NexusError::invalid("音频帧采样数不能被声道数整除"));
    }
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
fn validate_opus_format(sample_rate: u32, channels: u16) -> Result<()> {
    if sample_rate != OPUS_SAMPLE_RATE {
        return Err(NexusError::invalid(format!(
            "Opus 后端仅支持 {OPUS_SAMPLE_RATE}Hz 采样率"
        )));
    }
    opus_channels(channels)?;
    Ok(())
}
