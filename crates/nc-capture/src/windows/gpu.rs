use std::fmt;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Interface, Result as WindowsResult};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

static GPU_CAPTURE_DISABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn gpu_capture_enabled() -> bool {
    if gpu_capture_suppressed() {
        return false;
    }
    if !gpu_capture_requested(std::env::var("NEXUSCAT_GPU_CAPTURE").ok().as_deref()) {
        return false;
    }
    let gpu = nc_platform::gpu_info();
    if gpu.name.to_ascii_lowercase().contains("basic render") {
        tracing::info!(
            gpu = %gpu.name,
            "检测到 Microsoft Basic Render Driver，GPU 采集不可用，保持 CPU 采集"
        );
        return false;
    }
    true
}

fn gpu_capture_requested(value: Option<&str>) -> bool {
    !value.is_some_and(|value| value == "0" || value.eq_ignore_ascii_case("false"))
}

pub(crate) fn gpu_capture_suppressed() -> bool {
    GPU_CAPTURE_DISABLED.load(Ordering::Relaxed)
}

pub fn disable_gpu_capture() {
    GPU_CAPTURE_DISABLED.store(true, Ordering::SeqCst);
}

pub struct GpuFrame {
    pub(crate) device: ID3D11Device,
    pub(crate) texture: ID3D11Texture2D,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl GpuFrame {
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

unsafe impl Send for GpuFrame {}

impl fmt::Debug for GpuFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

pub(crate) struct GpuScaler {
    device: ID3D11Device,
    video: ID3D11VideoDevice,
    context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    target_width: u32,
    target_height: u32,
}

impl GpuScaler {
    pub(crate) fn new(
        device: &ID3D11Device,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        fps: u32,
    ) -> WindowsResult<Self> {
        if let Ok(multithread) = device.cast::<ID3D10Multithread>() {
            let _ = unsafe { multithread.SetMultithreadProtected(true) };
        }
        let video: ID3D11VideoDevice = device.cast()?;
        let immediate = unsafe { device.GetImmediateContext() }?;
        let context: ID3D11VideoContext = immediate.cast()?;
        let rate = DXGI_RATIONAL {
            Numerator: fps.max(1),
            Denominator: 1,
        };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: source_width,
            InputHeight: source_height,
            OutputFrameRate: rate,
            OutputWidth: target_width,
            OutputHeight: target_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { video.CreateVideoProcessorEnumerator(&desc) }?;
        for (format, required) in [
            (
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT,
            ),
            (
                DXGI_FORMAT_NV12,
                D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
            ),
        ] {
            let support = unsafe { enumerator.CheckVideoProcessorFormat(format) }?;
            if support & required.0 as u32 == 0 {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    format!("VideoProcessor 不支持格式 {format:?}"),
                ));
            }
        }
        let processor = unsafe { video.CreateVideoProcessor(&enumerator, 0) }?;
        let full = RECT {
            left: 0,
            top: 0,
            right: source_width as i32,
            bottom: source_height as i32,
        };
        let scaled = RECT {
            left: 0,
            top: 0,
            right: target_width as i32,
            bottom: target_height as i32,
        };
        unsafe {
            context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&full));
            context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&scaled));
            context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let input = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 4 };
            let output = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 20 };
            context.VideoProcessorSetStreamColorSpace(&processor, 0, &input);
            context.VideoProcessorSetOutputColorSpace(&processor, &output);
            context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&scaled));
        }
        Ok(Self {
            device: device.clone(),
            video,
            context,
            enumerator,
            processor,
            target_width,
            target_height,
        })
    }

    pub(crate) fn scale(&self, source: &ID3D11Texture2D) -> WindowsResult<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.target_width,
            Height: self.target_height,
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
        let mut output: Option<ID3D11Texture2D> = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut output)) }?;
        let output = output.ok_or_else(|| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "NV12 纹理创建失败")
        })?;

        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            self.video.CreateVideoProcessorInputView(
                source,
                &self.enumerator,
                &input_desc,
                Some(&mut input),
            )
        }?;
        let input = input.ok_or_else(|| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "输入视图为空")
        })?;

        let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut view: Option<ID3D11VideoProcessorOutputView> = None;
        unsafe {
            self.video.CreateVideoProcessorOutputView(
                &output,
                &self.enumerator,
                &output_desc,
                Some(&mut view),
            )
        }?;
        let view = view.ok_or_else(|| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "输出视图为空")
        })?;

        let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: ManuallyDrop::new(Some(input)),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        }];
        let result = unsafe {
            self.context
                .VideoProcessorBlt(&self.processor, &view, 0, &streams)
        };
        drop(unsafe { ManuallyDrop::into_inner(std::ptr::read(&streams[0].pInputSurface)) });
        result?;
        Ok(output)
    }
}

