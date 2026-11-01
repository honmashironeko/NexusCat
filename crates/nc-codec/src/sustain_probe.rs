use crate::video::{EncoderConfig, EncoderVendor};

#[derive(Debug, Clone, PartialEq)]
pub struct SustainedEncoderProbe {
    pub backend: String,
    pub fps: f64,
}

pub fn probe_video_encoder_sustained(
    config: EncoderConfig,
    preferred_vendor: Option<EncoderVendor>,
) -> Option<SustainedEncoderProbe> {
    config.validate().ok()?;
    #[cfg(windows)]
    {
        windows_probe(config, preferred_vendor)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = preferred_vendor;
        macos_probe(config)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (config, preferred_vendor);
        None
    }
}

#[cfg(windows)]
fn windows_probe(
    config: EncoderConfig,
    preferred_vendor: Option<EncoderVendor>,
) -> Option<SustainedEncoderProbe> {
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
        D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device,
        ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

    use crate::mf_video::MfH264Encoder;
    use crate::video::VideoEncoder;

    let mut device: Option<ID3D11Device> = None;
    let mut context = None;
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            flags,
            None::<&[D3D_FEATURE_LEVEL]>,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None::<*mut D3D_FEATURE_LEVEL>,
            Some(&mut context),
        )
        .ok()?;
    }
    let device = device?;
    let _context = context;

    let width = config.width;
    let height = config.height;
    let plane_len = usize::try_from(width.checked_mul(height)?).ok()?;
    let uv_len = plane_len / 2;
    let mut pixels = vec![16u8; plane_len + uv_len];
    pixels[plane_len..].fill(128);
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: width,
        SysMemSlicePitch: 0,
    };
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, Some(&initial), Some(&mut texture)).ok()? };
    let texture = texture?;

    let mut encoder = MfH264Encoder::new_with_device(config, preferred_vendor, &device).ok()?;
    if !encoder.is_hardware() {
        return None;
    }
    let backend = encoder.backend_name().to_string();
    let frames = (config.fps / 4).max(8);
    let interval = Duration::from_nanos(1_000_000_000 / u64::from(config.fps.max(1)));
    let started = Instant::now();
    let mut produced = 0usize;
    for tick in 0..frames {
        let timestamp_us = u64::from(tick) * 1_000_000 / u64::from(config.fps.max(1));
        let mut outputs: Vec<_> = encoder
            .encode_texture(&texture, timestamp_us)
            .ok()?
            .into_iter()
            .collect();
        outputs.extend(encoder.poll().ok()?);
        produced += outputs.len();
        let deadline = started + interval * (tick + 1);
        if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
    }
    produced += encoder.flush().ok()?.len();
    let elapsed = started.elapsed().as_secs_f64();
    Some(SustainedEncoderProbe {
        backend,
        fps: if elapsed > 0.0 {
            produced as f64 / elapsed
        } else {
            0.0
        },
    })
}

#[cfg(target_os = "macos")]
fn macos_probe(config: EncoderConfig) -> Option<SustainedEncoderProbe> {
    use std::time::{Duration, Instant};

    use nc_core::{PixelFormat, VideoFrame};

    use crate::video::VideoEncoder;
    use crate::vt_video::VtH264Encoder;

    let mut encoder = VtH264Encoder::new(config).ok()?;
    let backend = encoder.backend_name().to_string();
    let width = config.width;
    let height = config.height;
    let stride = usize::try_from(width).ok()?.checked_mul(4)?;
    let mut frame = VideoFrame {
        width,
        height,
        stride,
        format: PixelFormat::Bgra8,
        timestamp_us: 0,
        data: vec![128u8; stride.checked_mul(usize::try_from(height).ok()?)?],
    };
    let frames = (config.fps / 4).max(8);
    let interval = Duration::from_nanos(1_000_000_000 / u64::from(config.fps.max(1)));
    let started = Instant::now();
    let mut produced = 0usize;
    for tick in 0..frames {
        frame.timestamp_us = u64::from(tick) * 1_000_000 / u64::from(config.fps.max(1));
        if encoder.encode(&frame).ok()?.is_some() {
            produced += 1;
        }
        let deadline = started + interval * (tick + 1);
        if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
    }
    produced += encoder.flush().ok()?.len();
    let elapsed = started.elapsed().as_secs_f64();
    Some(SustainedEncoderProbe {
        backend,
        fps: if elapsed > 0.0 {
            produced as f64 / elapsed
        } else {
            0.0
        },
    })
}
