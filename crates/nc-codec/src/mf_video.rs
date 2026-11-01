use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use nc_core::{EncodedVideoFrame, NexusError, Result, VideoFrame};
use windows::core::{Error as WindowsError, Interface, GUID};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, VARIANT_TRUE};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_Base, eAVEncH264VProfile_High,
    eAVEncH264VProfile_Main, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQualityVsSpeed,
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncCommonRealTime,
    CODECAPI_AVEncH264CABACEnable, CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncMaxFrameRate, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
    ICodecAPI, IMF2DBuffer, IMFActivate, IMFAttributes, IMFDXGIDeviceManager,
    IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, METransformDrainComplete,
    METransformHaveOutput, METransformNeedInput, MFCreateDXGIDeviceManager,
    MFCreateDXGISurfaceBuffer, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFMediaType_Video, MFSampleExtension_CleanPoint, MFShutdown, MFStartup, MFTEnumEx,
    MFT_FRIENDLY_NAME_Attribute, MFVideoFormat_H264, MFVideoFormat_I420, MFVideoFormat_IYUV,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFSTARTUP_LITE, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_COMMAND_FLUSH,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
    MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_PROCESS_OUTPUT_STATUS_NEW_STREAMS, MFT_REGISTER_TYPE_INFO, MF_EVENT_FLAG_NO_WAIT,
    MF_E_NOTACCEPTING, MF_E_NO_EVENTS_AVAILABLE, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_LEVEL,
    MF_MT_MPEG2_PROFILE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4, VT_UI8};

use crate::bgra_to_i420;
use crate::color::bgra_to_nv12;
use crate::video::{contains_keyframe_nal, contains_nal_type, EncoderConfig, EncoderVendor};

const EVENT_POLL_INTERVAL: Duration = Duration::from_micros(200);
const FIRST_INPUT_TIMEOUT: Duration = Duration::from_millis(50);
const PENDING_INPUT_TIMEOUT: Duration = Duration::from_millis(2);
const DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

static MF_SESSION_COUNT: OnceLock<Mutex<usize>> = OnceLock::new();

thread_local! {
    static COM_GUARD: RefCell<Option<ComGuard>> = const { RefCell::new(None) };
}

pub struct MfH264Encoder {
    transform: Option<IMFTransform>,
    event_generator: Option<IMFMediaEventGenerator>,
    codec_api: Option<ICodecAPI>,
    config: EncoderConfig,
    input_kind: InputKind,
    is_async: bool,
    provides_samples: bool,
    output_buffer_size: u32,
    pending_inputs: VecDeque<IMFSample>,
    pending_timestamps: VecDeque<u64>,
    pending_keyframes: VecDeque<bool>,
    outputs: VecDeque<EncodedVideoFrame>,
    need_input: bool,
    drain_complete: bool,
    force_keyframe_pending: bool,
    has_submitted_input: bool,
    sequence_header: Option<Vec<u8>>,
    backend: BackendInfo,
    _d3d_manager: Option<IMFDXGIDeviceManager>,
    _d3d_device: Option<ID3D11Device>,
    _session: MfSession,
}

unsafe impl Send for MfH264Encoder {}

impl MfH264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        Self::new_with_preference(config, None)
    }

    pub fn new_with_preference(
        config: EncoderConfig,
        preferred_vendor: Option<EncoderVendor>,
    ) -> Result<Self> {
        Self::new_with_preference_and_speed(config, preferred_vendor, false)
    }

    pub fn new_with_preference_and_speed(
        config: EncoderConfig,
        preferred_vendor: Option<EncoderVendor>,
        prefer_speed: bool,
    ) -> Result<Self> {
        Self::create(config, preferred_vendor, None, prefer_speed)
    }

    pub fn new_with_device(
        config: EncoderConfig,
        preferred_vendor: Option<EncoderVendor>,
        device: &ID3D11Device,
    ) -> Result<Self> {
        Self::create(config, preferred_vendor, Some(device), false)
    }

    fn create(
        config: EncoderConfig,
        preferred_vendor: Option<EncoderVendor>,
        device: Option<&ID3D11Device>,
        prefer_speed: bool,
    ) -> Result<Self> {
        config.validate()?;
        ensure_com_initialized()?;
        let mut session = Some(MfSession::acquire()?);
        let mut attempts = Vec::new();
        for hardware in [true, false] {
            let activations = enumerate_encoders(hardware);
            let activations = if hardware {
                order_by_vendor(activations, preferred_vendor)
            } else {
                activations
            };
            for activation in activations {
                let name = activation_name(&activation);
                tracing::debug!(encoder = %name, hardware, "发现 Media Foundation H.264 编码器候选");
                match configure_transform(
                    config,
                    &activation,
                    &name,
                    hardware,
                    device,
                    prefer_speed,
                ) {
                    Ok(configured) => {
                        if let Some(preferred) = preferred_vendor {
                            if EncoderVendor::from_name(&name) != Some(preferred) {
                                tracing::warn!(
                                    encoder = %name,
                                    preferred = ?preferred,
                                    "首选厂商的硬件编码器不可用，已回退到其他后端"
                                );
                            }
                        }
                        tracing::info!(
                            encoder = %name,
                            hardware,
                            is_async = configured.is_async,
                            provides_samples = configured.provides_samples,
                            input = ?configured.input_kind,
                            "已创建 Media Foundation H.264 编码器"
                        );
                        return Ok(Self {
                            transform: Some(configured.transform),
                            event_generator: configured.event_generator,
                            codec_api: configured.codec_api,
                            config,
                            input_kind: configured.input_kind,
                            is_async: configured.is_async,
                            provides_samples: configured.provides_samples,
                            output_buffer_size: configured.output_buffer_size,
                            pending_inputs: VecDeque::new(),
                            pending_timestamps: VecDeque::new(),
                            pending_keyframes: VecDeque::new(),
                            outputs: VecDeque::new(),
                            need_input: false,
                            drain_complete: false,
                            force_keyframe_pending: true,
                            has_submitted_input: false,
                            sequence_header: configured.sequence_header,
                            backend: BackendInfo { name, hardware },
                            _d3d_manager: configured.d3d_manager,
                            _d3d_device: device.cloned(),
                            _session: session.take().expect("Media Foundation 会话已创建"),
                        });
                    }
                    Err(error) => {
                        tracing::debug!(
                            %error,
                            hardware,
                            encoder = %name,
                            "Media Foundation 编码器配置失败，尝试下一个"
                        );
                        attempts.push(format!("{name}: {error}"));
                    }
                }
            }
        }

        let detail = if attempts.is_empty() {
            String::new()
        } else {
            format!("（尝试结果：{}）", attempts.join("；"))
        };
        Err(NexusError::Media(format!(
            "未找到可用的 H.264 Media Foundation 编码器{detail}"
        )))
    }

    pub fn is_hardware(&self) -> bool {
        self.backend.hardware
    }

    pub fn backend_name(&self) -> &str {
        &self.backend.name
    }

    fn convert_frame(&self, frame: &VideoFrame) -> Result<Vec<u8>> {
        match self.input_kind {
            InputKind::Nv12 => bgra_to_nv12(frame, self.config.width, self.config.height),
            InputKind::I420 => {
                let yuv = bgra_to_i420(frame, self.config.width, self.config.height)?;
                let mut data = Vec::with_capacity(yuv.y.len() + 2 * yuv.u.len());
                data.extend_from_slice(&yuv.y);
                data.extend_from_slice(&yuv.u);
                data.extend_from_slice(&yuv.v);
                Ok(data)
            }
        }
    }

    fn create_sample(
        &self,
        bytes: &[u8],
        timestamp_us: u64,
        force_keyframe: bool,
    ) -> Result<IMFSample> {
        let length = u32::try_from(bytes.len())
            .map_err(|_| NexusError::Media("单帧输入超过 Media Foundation 缓冲区上限".into()))?;
        let buffer = unsafe { MFCreateMemoryBuffer(length) }
            .map_err(|error| mf_error("创建输入缓冲区", error))?;
        let mut pointer: *mut u8 = null_mut();
        unsafe { buffer.Lock(&mut pointer, None, None) }
            .map_err(|error| mf_error("锁定输入缓冲区", error))?;
        if pointer.is_null() {
            let _ = unsafe { buffer.Unlock() };
            return Err(NexusError::Media("输入缓冲区指针为空".into()));
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len()) };
        unsafe { buffer.Unlock() }.map_err(|error| mf_error("解锁输入缓冲区", error))?;
        unsafe { buffer.SetCurrentLength(length) }
            .map_err(|error| mf_error("设置输入缓冲区长度", error))?;

        let sample =
            unsafe { MFCreateSample() }.map_err(|error| mf_error("创建输入 sample", error))?;
        unsafe { sample.AddBuffer(&buffer) }.map_err(|error| mf_error("绑定输入缓冲区", error))?;
        self.stamp_sample(&sample, timestamp_us, force_keyframe)?;
        Ok(sample)
    }

    fn create_texture_sample(
        &self,
        texture: &ID3D11Texture2D,
        timestamp_us: u64,
        force_keyframe: bool,
    ) -> Result<IMFSample> {
        let buffer = unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false) }
            .map_err(|error| mf_error("包装 D3D11 纹理", error))?;
        let length = unsafe {
            buffer
                .cast::<IMF2DBuffer>()
                .map_err(|error| mf_error("读取 DXGI 纹理接口", error))?
                .GetContiguousLength()
        }
        .map_err(|error| mf_error("读取 DXGI 纹理长度", error))?;
        unsafe { buffer.SetCurrentLength(length) }
            .map_err(|error| mf_error("设置 DXGI 纹理长度", error))?;
        let sample =
            unsafe { MFCreateSample() }.map_err(|error| mf_error("创建纹理 sample", error))?;
        unsafe { sample.AddBuffer(&buffer) }.map_err(|error| mf_error("绑定纹理缓冲区", error))?;
        self.stamp_sample(&sample, timestamp_us, force_keyframe)?;
        Ok(sample)
    }

    fn stamp_sample(
        &self,
        sample: &IMFSample,
        timestamp_us: u64,
        force_keyframe: bool,
    ) -> Result<()> {
        let hundred_nanoseconds = timestamp_us.min((i64::MAX / 10) as u64) * 10;
        unsafe { sample.SetSampleTime(hundred_nanoseconds as i64) }
            .map_err(|error| mf_error("设置 sample 时间戳", error))?;
        let duration = 10_000_000 / u64::from(self.config.fps.max(1));
        unsafe { sample.SetSampleDuration(duration as i64) }
            .map_err(|error| mf_error("设置 sample 时长", error))?;
        if force_keyframe {
            if let Err(error) = unsafe { sample.SetUINT32(&MFSampleExtension_CleanPoint, 1) } {
                tracing::warn!(%error, "设置关键帧 sample 标记失败");
            }
        }
        Ok(())
    }

    fn create_output_sample(&self) -> Result<IMFSample> {
        let capacity = self
            .output_buffer_size
            .max(self.raw_frame_size())
            .max(64 * 1024);
        let buffer = unsafe { MFCreateMemoryBuffer(capacity) }
            .map_err(|error| mf_error("创建输出缓冲区", error))?;
        let sample =
            unsafe { MFCreateSample() }.map_err(|error| mf_error("创建输出 sample", error))?;
        unsafe { sample.AddBuffer(&buffer) }.map_err(|error| mf_error("绑定输出缓冲区", error))?;
        Ok(sample)
    }

    fn raw_frame_size(&self) -> u32 {
        self.config
            .width
            .saturating_mul(self.config.height)
            .saturating_mul(3)
            / 2
    }

    fn force_keyframe_via_codec(&self) {
        if let Some(codec_api) = &self.codec_api {
            set_codec_u32(codec_api, &CODECAPI_AVEncVideoForceKeyFrame, 1);
        }
    }

    fn try_send_pending_input(&mut self) -> Result<()> {
        if !self.need_input {
            return Ok(());
        }
        let Some(sample) = self.pending_inputs.front().cloned() else {
            return Ok(());
        };
        if self.pending_keyframes.front().copied().unwrap_or(false) {
            self.force_keyframe_via_codec();
        }
        let transform = self
            .transform
            .as_ref()
            .ok_or_else(|| NexusError::Media("编码器已释放".into()))?;
        match unsafe { transform.ProcessInput(0, &sample, 0) } {
            Ok(()) => {
                self.pending_inputs.pop_front();
                self.pending_keyframes.pop_front();
                self.need_input = false;
                self.has_submitted_input = true;
                Ok(())
            }
            Err(error) if error.code() == MF_E_NOTACCEPTING => Ok(()),
            Err(error) => Err(mf_error("提交编码帧", error)),
        }
    }

    fn pump_sync(&mut self) -> Result<()> {
        loop {
            let mut progressed = false;
            if let Some(sample) = self.pending_inputs.front().cloned() {
                if self.pending_keyframes.front().copied().unwrap_or(false) {
                    self.force_keyframe_via_codec();
                }
                let transform = self
                    .transform
                    .as_ref()
                    .ok_or_else(|| NexusError::Media("编码器已释放".into()))?;
                match unsafe { transform.ProcessInput(0, &sample, 0) } {
                    Ok(()) => {
                        self.pending_inputs.pop_front();
                        self.pending_keyframes.pop_front();
                        progressed = true;
                    }
                    Err(error) if error.code() == MF_E_NOTACCEPTING => {}
                    Err(error) => return Err(mf_error("提交编码帧", error)),
                }
            }

            while let Some(frame) = self.receive_output()? {
                self.outputs.push_back(frame);
                progressed = true;
            }

            if !progressed || self.pending_inputs.is_empty() {
                break;
            }
        }
        Ok(())
    }

    fn submit_pending_input(&mut self) -> Result<()> {
        let timeout = if self.has_submitted_input {
            PENDING_INPUT_TIMEOUT
        } else {
            FIRST_INPUT_TIMEOUT
        };
        let deadline = Instant::now() + timeout;
        loop {
            self.drain_async_events()?;
            self.try_send_pending_input()?;
            if self.pending_inputs.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(EVENT_POLL_INTERVAL);
        }
        self.drain_async_events()?;
        Ok(())
    }

    fn discard_pending_input(&mut self) {
        if self.pending_inputs.is_empty() {
            return;
        }
        if self.pending_keyframes.iter().any(|force| *force) {
            self.force_keyframe_pending = true;
        }
        self.pending_inputs.clear();
        self.pending_timestamps.clear();
        self.pending_keyframes.clear();
    }

    fn drain_async_events(&mut self) -> Result<()> {
        for _ in 0..256 {
            let event = {
                let generator = self
                    .event_generator
                    .as_ref()
                    .ok_or_else(|| NexusError::Media("编码器未提供事件接口".into()))?;
                match unsafe { generator.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                    Ok(event) => event,
                    Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                    Err(error) => return Err(mf_error("获取编码器事件", error)),
                }
            };
            let event_type = unsafe { event.GetType() }
                .map_err(|error| mf_error("读取编码器事件类型", error))?;
            let status = unsafe { event.GetStatus() }
                .map_err(|error| mf_error("读取编码器事件状态", error))?;
            if status.is_err() {
                return Err(NexusError::Media(format!(
                    "编码器事件返回失败 HRESULT 0x{:08X}",
                    status.0 as u32
                )));
            }

            if event_type == METransformNeedInput.0 as u32 {
                self.need_input = true;
                self.try_send_pending_input()?;
            } else if event_type == METransformHaveOutput.0 as u32 {
                self.need_input = false;
                if let Some(frame) = self.receive_output()? {
                    self.outputs.push_back(frame);
                }
            } else if event_type == METransformDrainComplete.0 as u32 {
                self.drain_complete = true;
            }
        }
        Ok(())
    }

    fn receive_output(&mut self) -> Result<Option<EncodedVideoFrame>> {
        for _ in 0..32 {
            let (result, sample, events, status) = {
                let transform = self
                    .transform
                    .as_ref()
                    .ok_or_else(|| NexusError::Media("编码器已释放".into()))?;
                let supplied_sample = if self.provides_samples {
                    None
                } else {
                    Some(self.create_output_sample()?)
                };
                let mut output = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(supplied_sample),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                };
                let mut status = 0u32;
                let result = unsafe {
                    transform.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status)
                };
                let sample = unsafe { ManuallyDrop::take(&mut output.pSample) };
                let events = unsafe { ManuallyDrop::take(&mut output.pEvents) };
                (result, sample, events, status)
            };
            drop(events);

            match result {
                Ok(()) => {
                    let sample = sample
                        .ok_or_else(|| NexusError::Media("编码器未返回输出 sample".into()))?;
                    let bytes = read_sample(&sample)?;
                    if status & MFT_PROCESS_OUTPUT_STATUS_NEW_STREAMS.0 as u32 != 0 {
                        self.renegotiate_output()?;
                    }
                    let data = ensure_annex_b(bytes);
                    if data.is_empty() {
                        tracing::warn!("编码器返回空输出，跳过该访问单元");
                        continue;
                    }
                    if !contains_nal_type(&data, 1) && !contains_nal_type(&data, 5) {
                        if contains_nal_type(&data, 7) || contains_nal_type(&data, 8) {
                            tracing::debug!("编码器单独输出参数集 NAL，缓存为序列头");
                            self.sequence_header = Some(data);
                        } else {
                            tracing::warn!("编码器返回不含 VCL NAL 的输出，跳过");
                        }
                        continue;
                    }
                    let timestamp_us = self.pending_timestamps.pop_front().unwrap_or(0);
                    return Ok(Some(self.build_output(data, timestamp_us)));
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.renegotiate_output()?;
                    continue;
                }
                Err(error) => return Err(mf_error("读取 H.264 输出", error)),
            }
        }
        Err(NexusError::Media(
            "编码器连续返回参数集或流变更，未能产出访问单元".into(),
        ))
    }

    fn renegotiate_output(&mut self) -> Result<()> {
        let (sequence_header, provides_samples, output_buffer_size) = {
            let transform = self
                .transform
                .as_ref()
                .ok_or_else(|| NexusError::Media("编码器已释放".into()))?;
            let mut selected = None;
            for index in 0..16 {
                let Ok(available) = (unsafe { transform.GetOutputAvailableType(0, index) }) else {
                    break;
                };
                let subtype = unsafe { available.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
                if subtype == MFVideoFormat_H264 {
                    selected = Some(available);
                    break;
                }
            }
            let output_type = match selected {
                Some(media_type) => media_type,
                None => unsafe { transform.GetOutputCurrentType(0) }
                    .map_err(|error| mf_error("读取当前输出类型", error))?,
            };
            unsafe { transform.SetOutputType(0, &output_type, 0) }
                .map_err(|error| mf_error("重新协商输出类型", error))?;
            let actual = unsafe { transform.GetOutputCurrentType(0) }.unwrap_or(output_type);
            let header = get_blob(&actual, &MF_MT_MPEG_SEQUENCE_HEADER);
            let info = unsafe { transform.GetOutputStreamInfo(0) }
                .map_err(|error| mf_error("读取输出流信息", error))?;
            (
                header,
                info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0,
                info.cbSize,
            )
        };

        if sequence_header.is_some() {
            self.sequence_header = sequence_header;
        }
        self.provides_samples = provides_samples;
        self.output_buffer_size = output_buffer_size;
        Ok(())
    }

    fn build_output(&self, bytes: Vec<u8>, timestamp_us: u64) -> EncodedVideoFrame {
        let mut data = ensure_annex_b(bytes);
        let has_idr = contains_nal_type(&data, 5);
        let has_sps = contains_nal_type(&data, 7);
        let has_pps = contains_nal_type(&data, 8);
        if has_idr && (!has_sps || !has_pps) {
            if let Some(header) = &self.sequence_header {
                let mut prefixed = ensure_annex_b(header.clone());
                prefixed.extend_from_slice(&data);
                data = prefixed;
            }
        }
        EncodedVideoFrame {
            keyframe: contains_keyframe_nal(&data),
            data,
            timestamp_us,
            width: self.config.width,
            height: self.config.height,
        }
    }

    fn wait_for_drain(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        self.drain_complete = false;
        loop {
            self.drain_async_events()?;
            if self.drain_complete {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!("等待编码器 DRAIN 超时，返回已产生的输出");
                break;
            }
            std::thread::sleep(EVENT_POLL_INTERVAL);
        }
        self.drain_async_events()?;
        Ok(())
    }

    fn restart_streaming(&mut self) {
        let Some(transform) = self.transform.as_ref() else {
            return;
        };
        let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0) };
        let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0) };
        let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0) };
        let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0) };
        let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0) };
        self.need_input = false;
        self.drain_complete = false;
        self.force_keyframe_pending = true;
        self.has_submitted_input = false;
    }
}

impl MfH264Encoder {
    fn wait_for_pending_inputs(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(100);
        while !self.pending_inputs.is_empty() {
            self.drain_async_events()?;
            self.try_send_pending_input()?;
            if self.pending_inputs.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(EVENT_POLL_INTERVAL);
        }
        if !self.pending_inputs.is_empty() {
            tracing::warn!(
                pending = self.pending_inputs.len(),
                "冲刷前仍有未提交的输入帧，放弃这些帧"
            );
        }
        Ok(())
    }
}

impl crate::video::VideoEncoder for MfH264Encoder {
    fn encode(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoFrame>> {
        ensure_com_initialized()?;
        if self.is_async {
            self.discard_pending_input();
        }
        let bytes = self.convert_frame(frame)?;
        let force_keyframe = self.force_keyframe_pending;
        let sample = self.create_sample(&bytes, frame.timestamp_us, force_keyframe)?;
        self.force_keyframe_pending = false;
        self.push_pending(sample, frame.timestamp_us, force_keyframe);
        self.pump_after_push()
    }

    fn poll(&mut self) -> Result<Vec<EncodedVideoFrame>> {
        if self.is_async {
            self.drain_async_events()?;
            self.try_send_pending_input()?;
            self.drain_async_events()?;
        }
        Ok(self.outputs.drain(..).collect())
    }

    fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>> {
        ensure_com_initialized()?;
        if self.transform.is_none() {
            return Ok(Vec::new());
        }

        if self.is_async {
            self.wait_for_pending_inputs()?;
        } else {
            self.pump_sync()?;
        }

        {
            let transform = self
                .transform
                .as_ref()
                .ok_or_else(|| NexusError::Media("编码器已释放".into()))?;
            if let Err(error) = unsafe { transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) } {
                tracing::warn!(%error, "编码器不支持 DRAIN，收集当前可用输出");
            }
        }

        if self.is_async {
            self.wait_for_drain(DRAIN_TIMEOUT)?;
        } else {
            while let Some(frame) = self.receive_output()? {
                self.outputs.push_back(frame);
            }
        }

        let frames = self.outputs.drain(..).collect();
        self.pending_inputs.clear();
        self.pending_timestamps.clear();
        self.pending_keyframes.clear();
        self.restart_streaming();
        Ok(frames)
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe_pending = true;
    }

    fn config(&self) -> EncoderConfig {
        self.config
    }

    fn backend_name(&self) -> &str {
        &self.backend.name
    }

    fn is_hardware(&self) -> bool {
        self.backend.hardware
    }
}

impl MfH264Encoder {
    pub fn encode_texture(
        &mut self,
        texture: &ID3D11Texture2D,
        timestamp_us: u64,
    ) -> Result<Option<EncodedVideoFrame>> {
        ensure_com_initialized()?;
        if self.is_async {
            self.discard_pending_input();
        }
        let force_keyframe = self.force_keyframe_pending;
        let sample = self.create_texture_sample(texture, timestamp_us, force_keyframe)?;
        self.force_keyframe_pending = false;
        self.push_pending(sample, timestamp_us, force_keyframe);
        self.pump_after_push()
    }

    fn push_pending(&mut self, sample: IMFSample, timestamp_us: u64, force_keyframe: bool) {
        self.pending_timestamps.push_back(timestamp_us);
        self.pending_keyframes.push_back(force_keyframe);
        self.pending_inputs.push_back(sample);
    }

    fn pump_after_push(&mut self) -> Result<Option<EncodedVideoFrame>> {
        if self.is_async {
            self.submit_pending_input()?;
        } else {
            self.pump_sync()?;
        }
        Ok(self.outputs.pop_front())
    }
}

impl Drop for MfH264Encoder {
    fn drop(&mut self) {
        let _ = ensure_com_initialized();
        if let Some(transform) = self.transform.take() {
            let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0) };
            let _ = unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0) };
            drop(transform);
        }
        self.codec_api = None;
        self.pending_inputs.clear();
        self.pending_timestamps.clear();
        self.pending_keyframes.clear();
        self.outputs.clear();
    }
}

#[derive(Clone)]
struct BackendInfo {
    name: String,
    hardware: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    Nv12,
    I420,
}

struct ConfiguredTransform {
    transform: IMFTransform,
    event_generator: Option<IMFMediaEventGenerator>,
    codec_api: Option<ICodecAPI>,
    input_kind: InputKind,
    is_async: bool,
    provides_samples: bool,
    output_buffer_size: u32,
    sequence_header: Option<Vec<u8>>,
    d3d_manager: Option<IMFDXGIDeviceManager>,
}

fn configure_transform(
    config: EncoderConfig,
    activation: &IMFActivate,
    name: &str,
    hardware: bool,
    device: Option<&ID3D11Device>,
    prefer_speed: bool,
) -> Result<ConfiguredTransform> {
    let transform: IMFTransform = unsafe { activation.ActivateObject() }
        .map_err(|error| mf_error(format!("激活编码器 {name}"), error))?;

    let attributes =
        unsafe { transform.GetAttributes() }.map_err(|error| mf_error("读取编码器属性", error))?;
    let is_async = unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) != 0;
    if is_async {
        unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }
            .map_err(|error| mf_error("解锁异步编码器", error))?;
    }
    if let Err(error) = unsafe { attributes.SetUINT32(&MF_LOW_LATENCY, 1) } {
        tracing::warn!(%error, hardware, "编码器不支持 MF_LOW_LATENCY，使用默认延迟设置");
    }

    let d3d_manager = match device {
        Some(device) => {
            let mut token = 0u32;
            let mut manager: Option<IMFDXGIDeviceManager> = None;
            unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }
                .map_err(|error| mf_error("创建 DXGI 设备管理器", error))?;
            let manager = manager.ok_or_else(|| NexusError::Media("DXGI 设备管理器为空".into()))?;
            unsafe { manager.ResetDevice(device, token) }
                .map_err(|error| mf_error("绑定编码器 D3D11 设备", error))?;
            unsafe {
                transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
            }
            .map_err(|error| mf_error("设置编码器 D3D 管理器", error))?;
            Some(manager)
        }
        None => None,
    };

    let (mut input_count, mut output_count) = (0u32, 0u32);
    unsafe { transform.GetStreamCount(&mut input_count, &mut output_count) }
        .map_err(|error| mf_error("读取编码器流数量", error))?;
    if input_count == 0 || output_count == 0 {
        return Err(NexusError::Media(format!(
            "编码器 {name} 没有可用的输入/输出流"
        )));
    }

    set_output_type(&transform, config)?;
    let output_type = unsafe { transform.GetOutputCurrentType(0) }
        .map_err(|error| mf_error("读取编码器输出类型", error))?;
    let codec_api = transform.cast::<ICodecAPI>().ok();
    configure_codec_api(codec_api.as_ref(), config, prefer_speed);
    let input_kind = set_input_type(&transform, config)?;
    let (provides_samples, output_buffer_size) = output_stream_info(&transform)?;
    let sequence_header = get_blob(&output_type, &MF_MT_MPEG_SEQUENCE_HEADER);
    let event_generator = if is_async {
        Some(
            transform
                .cast::<IMFMediaEventGenerator>()
                .map_err(|error| mf_error("获取编码器事件接口", error))?,
        )
    } else {
        None
    };

    unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0) }
        .map_err(|error| mf_error("启动编码器流", error))?;
    unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0) }
        .map_err(|error| mf_error("开始编码器流", error))?;

    Ok(ConfiguredTransform {
        transform,
        event_generator,
        codec_api,
        input_kind,
        is_async,
        provides_samples,
        output_buffer_size,
        sequence_header,
        d3d_manager,
    })
}

fn order_by_vendor(
    activations: Vec<IMFActivate>,
    preferred_vendor: Option<EncoderVendor>,
) -> Vec<IMFActivate> {
    let Some(preferred_vendor) = preferred_vendor else {
        return activations;
    };
    let names: Vec<String> = activations.iter().map(activation_name).collect();
    let order = vendor_order(&names, preferred_vendor);
    let mut slots: Vec<Option<IMFActivate>> = activations.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| slots.get_mut(index).and_then(Option::take))
        .collect()
}

fn vendor_order(names: &[String], preferred_vendor: EncoderVendor) -> Vec<usize> {
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by_key(|&index| EncoderVendor::from_name(&names[index]) != Some(preferred_vendor));
    order
}

fn enumerate_encoders(hardware: bool) -> Vec<IMFActivate> {
    let mut flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER;
    if hardware {
        flags |= MFT_ENUM_FLAG_HARDWARE;
    }
    let output_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activations: *mut Option<IMFActivate> = null_mut();
    let mut count = 0u32;
    let result = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&output_type as *const MFT_REGISTER_TYPE_INFO),
            &mut activations,
            &mut count,
        )
    };
    if let Err(error) = result {
        if !activations.is_null() {
            unsafe { CoTaskMemFree(Some(activations as *const c_void)) };
        }
        tracing::debug!(%error, hardware, "MFTEnumEx 未返回编码器");
        return Vec::new();
    }
    if count == 0 || activations.is_null() {
        if !activations.is_null() {
            unsafe { CoTaskMemFree(Some(activations as *const c_void)) };
        }
        return Vec::new();
    }

    let mut encoders = Vec::with_capacity(count as usize);
    let slots = unsafe { std::slice::from_raw_parts_mut(activations, count as usize) };
    for slot in slots {
        if let Some(activation) = slot.take() {
            encoders.push(activation);
        }
    }
    unsafe { CoTaskMemFree(Some(activations as *const c_void)) };
    encoders
}

fn activation_name(activation: &IMFActivate) -> String {
    let mut buffer = [0u16; 256];
    let mut length = 0u32;
    let Ok(()) = (unsafe {
        activation.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buffer, Some(&mut length))
    }) else {
        return "Media Foundation H.264 encoder".to_string();
    };
    let end = (length as usize).min(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..end]);
    let name = name.trim_end_matches('\0').trim();
    if name.is_empty() {
        "Media Foundation H.264 encoder".to_string()
    } else {
        name.to_string()
    }
}

fn create_output_type(
    config: EncoderConfig,
    profile: Option<i32>,
    level: Option<u32>,
) -> Result<IMFMediaType> {
    let media_type =
        unsafe { MFCreateMediaType() }.map_err(|error| mf_error("创建输出媒体类型", error))?;
    unsafe {
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|error| mf_error("设置输出主类型", error))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|error| mf_error("设置 H.264 输出子类型", error))?;
        media_type
            .SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate_kbps.saturating_mul(1000))
            .map_err(|error| mf_error("设置输出码率", error))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(config.width, config.height))
            .map_err(|error| mf_error("设置输出尺寸", error))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(config.fps, 1))
            .map_err(|error| mf_error("设置输出帧率", error))?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|error| mf_error("设置输出扫描模式", error))?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))
            .map_err(|error| mf_error("设置输出像素宽高比", error))?;
        if let Some(profile) = profile {
            media_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, profile as u32)
                .map_err(|error| mf_error("设置 H.264 profile", error))?;
        }
        if let Some(level) = level {
            media_type
                .SetUINT32(&MF_MT_MPEG2_LEVEL, level)
                .map_err(|error| mf_error("设置 H.264 level", error))?;
        }
    }
    Ok(media_type)
}

fn set_output_type(transform: &IMFTransform, config: EncoderConfig) -> Result<()> {
    let profiles = [
        Some(eAVEncH264VProfile_Base.0),
        Some(eAVEncH264VProfile_Main.0),
        Some(eAVEncH264VProfile_High.0),
        None,
    ];
    let required_level = u32::from(crate::video::level_idc(
        config.width,
        config.height,
        config.fps,
    ));
    let mut last_error = None;
    for level in [Some(required_level), None] {
        for profile in profiles {
            let media_type = create_output_type(config, profile, level)?;
            match unsafe { transform.SetOutputType(0, &media_type, 0) } {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
    }
    match last_error {
        Some(error) => Err(mf_error("设置 H.264 输出类型", error)),
        None => Err(NexusError::Media("编码器不接受 H.264 输出类型".into())),
    }
}

fn create_input_type(config: EncoderConfig, subtype: GUID) -> Result<IMFMediaType> {
    let media_type =
        unsafe { MFCreateMediaType() }.map_err(|error| mf_error("创建输入媒体类型", error))?;
    unsafe {
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|error| mf_error("设置输入主类型", error))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &subtype)
            .map_err(|error| mf_error("设置输入子类型", error))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(config.width, config.height))
            .map_err(|error| mf_error("设置输入尺寸", error))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(config.fps, 1))
            .map_err(|error| mf_error("设置输入帧率", error))?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|error| mf_error("设置输入扫描模式", error))?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))
            .map_err(|error| mf_error("设置输入像素宽高比", error))?;
    }
    Ok(media_type)
}

fn set_input_type(transform: &IMFTransform, config: EncoderConfig) -> Result<InputKind> {
    let candidates = [
        (InputKind::Nv12, MFVideoFormat_NV12),
        (InputKind::I420, MFVideoFormat_I420),
        (InputKind::I420, MFVideoFormat_IYUV),
    ];
    let mut last_error = None;
    for (kind, subtype) in candidates {
        let media_type = create_input_type(config, subtype)?;
        match unsafe { transform.SetInputType(0, &media_type, 0) } {
            Ok(()) => return Ok(kind),
            Err(error) => last_error = Some(error),
        }
    }

    for index in 0..16 {
        let Ok(available) = (unsafe { transform.GetInputAvailableType(0, index) }) else {
            break;
        };
        let subtype = unsafe { available.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default();
        let kind = if subtype == MFVideoFormat_NV12 {
            InputKind::Nv12
        } else if subtype == MFVideoFormat_I420 || subtype == MFVideoFormat_IYUV {
            InputKind::I420
        } else {
            continue;
        };
        let _ = unsafe {
            available.SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(config.width, config.height))
        };
        let _ = unsafe { available.SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(config.fps, 1)) };
        let _ = unsafe {
            available.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
        };
        if unsafe { transform.SetInputType(0, &available, 0) }.is_ok() {
            return Ok(kind);
        }
    }

    match last_error {
        Some(error) => Err(mf_error("设置 H.264 输入类型", error)),
        None => Err(NexusError::Media("编码器不接受 NV12/I420 输入格式".into())),
    }
}

fn output_stream_info(transform: &IMFTransform) -> Result<(bool, u32)> {
    let info = unsafe { transform.GetOutputStreamInfo(0) }
        .map_err(|error| mf_error("读取输出流信息", error))?;
    let provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
    Ok((provides_samples, info.cbSize))
}

fn configure_codec_api(codec_api: Option<&ICodecAPI>, config: EncoderConfig, prefer_speed: bool) {
    let Some(codec_api) = codec_api else {
        tracing::debug!("编码器未提供 ICodecAPI，跳过低延迟码控设置");
        return;
    };
    set_codec_bool(codec_api, &CODECAPI_AVLowLatencyMode, true);
    set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonRateControlMode,
        eAVEncCommonRateControlMode_CBR.0 as u32,
    );
    set_codec_bool(codec_api, &CODECAPI_AVEncCommonRealTime, true);
    let quality_vs_speed = if prefer_speed { 0 } else { 70 };
    set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonQualityVsSpeed,
        quality_vs_speed,
    );
    if prefer_speed {
        set_codec_bool(codec_api, &CODECAPI_AVEncH264CABACEnable, false);
    }
    set_codec_u32(
        codec_api,
        &CODECAPI_AVEncCommonMeanBitRate,
        config.bitrate_kbps.saturating_mul(1000),
    );
    set_codec_u64(
        codec_api,
        &CODECAPI_AVEncMaxFrameRate,
        (u64::from(config.fps) << 32) | 1,
    );
    set_codec_u32(
        codec_api,
        &CODECAPI_AVEncMPVGOPSize,
        config.keyframe_interval,
    );
    set_codec_u32(codec_api, &CODECAPI_AVEncMPVDefaultBPictureCount, 0);
}

fn set_codec_u32(codec_api: &ICodecAPI, key: &GUID, value: u32) {
    let variant = variant_u32(value);
    if let Err(error) = unsafe { codec_api.SetValue(key, &variant) } {
        tracing::warn!(
            parameter = codec_parameter_name(key),
            %error,
            "设置编码器参数失败，继续使用默认值"
        );
    }
}

fn set_codec_u64(codec_api: &ICodecAPI, key: &GUID, value: u64) {
    let variant = variant_u64(value);
    if let Err(error) = unsafe { codec_api.SetValue(key, &variant) } {
        tracing::warn!(
            parameter = codec_parameter_name(key),
            %error,
            "设置编码器参数失败，继续使用默认值"
        );
    }
}

fn set_codec_bool(codec_api: &ICodecAPI, key: &GUID, value: bool) {
    let variant = variant_bool(value);
    if let Err(error) = unsafe { codec_api.SetValue(key, &variant) } {
        tracing::warn!(
            parameter = codec_parameter_name(key),
            %error,
            "设置编码器布尔参数失败，继续使用默认值"
        );
    }
}

fn codec_parameter_name(key: &GUID) -> &'static str {
    if *key == CODECAPI_AVLowLatencyMode {
        "AVLowLatencyMode"
    } else if *key == CODECAPI_AVEncCommonRateControlMode {
        "AVEncCommonRateControlMode"
    } else if *key == CODECAPI_AVEncCommonRealTime {
        "AVEncCommonRealTime"
    } else if *key == CODECAPI_AVEncCommonQualityVsSpeed {
        "AVEncCommonQualityVsSpeed"
    } else if *key == CODECAPI_AVEncCommonMeanBitRate {
        "AVEncCommonMeanBitRate"
    } else if *key == CODECAPI_AVEncMaxFrameRate {
        "AVEncMaxFrameRate"
    } else if *key == CODECAPI_AVEncMPVGOPSize {
        "AVEncMPVGOPSize"
    } else if *key == CODECAPI_AVEncMPVDefaultBPictureCount {
        "AVEncMPVDefaultBPictureCount"
    } else if *key == CODECAPI_AVEncVideoForceKeyFrame {
        "AVEncVideoForceKeyFrame"
    } else {
        "unknown"
    }
}

fn variant_u32(value: u32) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = value;
    }
    variant
}

fn variant_u64(value: u64) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_UI8;
        inner.Anonymous.ullVal = value;
    }
    variant
}

fn variant_bool(value: bool) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if value {
            VARIANT_TRUE
        } else {
            windows::Win32::Foundation::VARIANT_BOOL(0)
        };
    }
    variant
}

fn get_blob(attributes: &IMFAttributes, key: &GUID) -> Option<Vec<u8>> {
    let size = unsafe { attributes.GetBlobSize(key) }.ok()?;
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    let mut actual = 0u32;
    unsafe { attributes.GetBlob(key, &mut data, Some(&mut actual)) }.ok()?;
    data.truncate(actual as usize);
    Some(data)
}

fn read_sample(sample: &IMFSample) -> Result<Vec<u8>> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|error| mf_error("读取输出缓冲区", error))?;
    let mut pointer: *mut u8 = null_mut();
    let mut length = 0u32;
    unsafe { buffer.Lock(&mut pointer, None, Some(&mut length)) }
        .map_err(|error| mf_error("锁定输出缓冲区", error))?;
    if pointer.is_null() {
        let _ = unsafe { buffer.Unlock() };
        return Err(NexusError::Media("输出缓冲区指针为空".into()));
    }
    let data = unsafe { std::slice::from_raw_parts(pointer, length as usize) }.to_vec();
    unsafe { buffer.Unlock() }.map_err(|error| mf_error("解锁输出缓冲区", error))?;
    Ok(data)
}

fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
    (u64::from(numerator) << 32) | u64::from(denominator)
}

fn ensure_annex_b(data: Vec<u8>) -> Vec<u8> {
    if starts_with_start_code(&data) {
        return data;
    }
    match avcc_to_annex_b(&data) {
        Some(converted) => converted,
        None => data,
    }
}

fn starts_with_start_code(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1])
}

fn avcc_to_annex_b(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 5 {
        return None;
    }
    let mut output = Vec::with_capacity(data.len() + 16);
    let mut offset = 0usize;
    let mut nal_count = 0usize;
    while offset < data.len() {
        if offset + 4 > data.len() {
            return None;
        }
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().ok()?) as usize;
        if length == 0 || offset + 4 + length > data.len() {
            return None;
        }
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(&data[offset + 4..offset + 4 + length]);
        offset += 4 + length;
        nal_count += 1;
        if nal_count > 4096 {
            return None;
        }
    }
    (offset == data.len() && nal_count > 0).then_some(output)
}

fn mf_error(context: impl std::fmt::Display, error: WindowsError) -> NexusError {
    NexusError::Media(format!("{context}：{error}"))
}

struct MfSession;

impl MfSession {
    fn acquire() -> Result<Self> {
        let mut count = mf_session_count()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *count == 0 {
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }
                .map_err(|error| mf_error("启动 Media Foundation", error))?;
        }
        *count += 1;
        Ok(Self)
    }
}

impl Drop for MfSession {
    fn drop(&mut self) {
        let mut count = mf_session_count()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *count == 0 {
            return;
        }
        *count -= 1;
        if *count == 0 {
            if let Err(error) = unsafe { MFShutdown() } {
                tracing::warn!(%error, "关闭 Media Foundation 失败");
            }
        }
    }
}

fn mf_session_count() -> &'static Mutex<usize> {
    MF_SESSION_COUNT.get_or_init(|| Mutex::new(0))
}

struct ComGuard {
    owns_initialization: bool,
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owns_initialization {
            unsafe { CoUninitialize() };
        }
    }
}

fn ensure_com_initialized() -> Result<()> {
    COM_GUARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return Ok(());
        }
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            *slot = Some(ComGuard {
                owns_initialization: true,
            });
            Ok(())
        } else if result == RPC_E_CHANGED_MODE {
            tracing::debug!("当前线程已使用其它 COM apartment，继续使用 Media Foundation");
            *slot = Some(ComGuard {
                owns_initialization: false,
            });
            Ok(())
        } else {
            Err(mf_error("初始化 COM", WindowsError::from_hresult(result)))
        }
    })
}

