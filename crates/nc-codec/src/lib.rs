mod audio;
mod color;
#[cfg(windows)]
mod mf_video;
#[cfg(windows)]
mod openh264_video;
mod sustain_probe;
mod video;
#[cfg(target_os = "macos")]
mod vt_video;

pub use audio::{
    create_audio_decoder, create_audio_encoder, AudioDecoder, AudioEncoder, AudioEncoderConfig,
    PcmAudioDecoder, PcmAudioEncoder,
};
#[cfg(any(windows, target_os = "macos"))]
pub use audio::{OpusAudioDecoder, OpusAudioEncoder};
pub use color::{bgra_to_i420, i420_to_bgra};
#[cfg(windows)]
pub use mf_video::MfH264Encoder;
#[cfg(windows)]
pub use openh264_video::OpenH264Encoder;
pub use sustain_probe::{probe_video_encoder_sustained, SustainedEncoderProbe};
pub use video::{
    create_software_video_encoder, create_video_decoder, create_video_encoder,
    create_video_encoder_tuned, create_video_encoder_with_preference,
    probe_video_encoder_with_preference, EncoderConfig, EncoderPreset, EncoderProbe, EncoderVendor,
    VideoDecoder, VideoEncoder,
};
#[cfg(target_os = "macos")]
pub use vt_video::VtH264Encoder;
