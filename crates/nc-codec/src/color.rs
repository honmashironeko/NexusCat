use nc_core::{NexusError, PixelFormat, Result, VideoFrame};
use rusty_h264::YuvFrame;

const WEIGHT_SCALE: u32 = 256;

fn validate_yuv_target(target_width: u32, target_height: u32) -> Result<()> {
    if target_width < 2 || target_height < 2 {
        return Err(NexusError::invalid("I420 输出宽高必须至少为 2"));
    }
    if target_width % 2 != 0 || target_height % 2 != 0 {
        return Err(NexusError::invalid("I420 输出宽高必须为偶数"));
    }
    Ok(())
}

fn validate_rgb_source(frame: &VideoFrame) -> Result<(usize, usize)> {
    if !matches!(frame.format, PixelFormat::Bgra8 | PixelFormat::Rgba8) {
        return Err(NexusError::invalid(
            "编码输入仅支持 BGRA8 或 RGBA8 像素格式",
        ));
    }
    if frame.width == 0 || frame.height == 0 {
        return Err(NexusError::invalid("源视频帧宽高不能为 0"));
    }

    let source_width = frame.width as usize;
    let source_height = frame.height as usize;
    let source_row_bytes = source_width
        .checked_mul(4)
        .ok_or_else(|| NexusError::invalid("源视频帧尺寸溢出"))?;
    if frame.stride < source_row_bytes {
        return Err(NexusError::invalid("源视频帧行宽小于一行像素所需字节数"));
    }
    let source_bytes = frame
        .stride
        .checked_mul(source_height)
        .ok_or_else(|| NexusError::invalid("源视频帧缓冲区尺寸溢出"))?;
    if frame.data.len() < source_bytes {
        return Err(NexusError::invalid("源视频帧数据长度不足"));
    }
    Ok((source_width, source_height))
}

#[derive(Clone, Copy)]
struct AxisTap {
    index0: usize,
    index1: usize,
    weight1: u32,
}

const SCALED_CONVERSION_MAX_THREADS: usize = 4;
const SCALED_CONVERSION_MIN_BLOCKS: usize = 64;

const PAIRED_CHANNEL_MASK: u32 = 0x00FF_00FF;

pub fn bgra_to_i420(frame: &VideoFrame, target_width: u32, target_height: u32) -> Result<YuvFrame> {
    validate_yuv_target(target_width, target_height)?;
    let (source_width, source_height) = validate_rgb_source(frame)?;

    let width = target_width as usize;
    let height = target_height as usize;
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let luma_len = width
        .checked_mul(height)
        .ok_or_else(|| NexusError::invalid("I420 亮度平面尺寸溢出"))?;
    let chroma_len = chroma_width
        .checked_mul(chroma_height)
        .ok_or_else(|| NexusError::invalid("I420 色度平面尺寸溢出"))?;

    let mut luma = vec![0u8; luma_len];
    let mut interleaved = vec![0u8; chroma_len * 2];
    scaled_luma_and_chroma(
        frame,
        source_width,
        source_height,
        width,
        height,
        &mut luma,
        &mut interleaved,
    );

    let mut chroma_u = Vec::with_capacity(chroma_len);
    let mut chroma_v = Vec::with_capacity(chroma_len);
    for pair in interleaved.chunks_exact(2) {
        chroma_u.push(pair[0]);
        chroma_v.push(pair[1]);
    }

    Ok(YuvFrame {
        width,
        height,
        y: luma,
        u: chroma_u,
        v: chroma_v,
    })
}

fn scaled_luma_and_chroma(
    frame: &VideoFrame,
    source_width: usize,
    source_height: usize,
    width: usize,
    height: usize,
    luma: &mut [u8],
    chroma: &mut [u8],
) {
    let x_taps = axis_taps(width, source_width);
    let y_taps = axis_taps(height, source_height);
    let swap_red_blue = frame.format == PixelFormat::Rgba8;
    let chroma_height = height / 2;
    let threads = scaled_conversion_threads(chroma_height);
    if threads <= 1 {
        process_output_blocks(
            frame,
            width,
            &x_taps,
            &y_taps,
            swap_red_blue,
            0,
            chroma_height,
            luma,
            chroma,
        );
        return;
    }

    let blocks_per_chunk = chroma_height.div_ceil(threads);
    let luma_chunk_len = blocks_per_chunk * 2 * width;
    let chroma_chunk_len = blocks_per_chunk * width;
    let x_taps = &x_taps;
    let y_taps = &y_taps;
    std::thread::scope(|scope| {
        let luma_chunks = luma.chunks_mut(luma_chunk_len);
        let chroma_chunks = chroma.chunks_mut(chroma_chunk_len);
        for (chunk_index, (luma_chunk, chroma_chunk)) in luma_chunks.zip(chroma_chunks).enumerate()
        {
            let start_block = chunk_index * blocks_per_chunk;
            let end_block = (start_block + blocks_per_chunk).min(chroma_height);
            if start_block >= end_block {
                continue;
            }
            scope.spawn(move || {
                process_output_blocks(
                    frame,
                    width,
                    x_taps,
                    y_taps,
                    swap_red_blue,
                    start_block,
                    end_block,
                    luma_chunk,
                    chroma_chunk,
                );
            });
        }
    });
}

fn scaled_conversion_threads(chroma_blocks: usize) -> usize {
    if chroma_blocks < SCALED_CONVERSION_MIN_BLOCKS {
        return 1;
    }
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(SCALED_CONVERSION_MAX_THREADS)
}

#[allow(clippy::too_many_arguments)]
fn process_output_blocks(
    frame: &VideoFrame,
    width: usize,
    x_taps: &[AxisTap],
    y_taps: &[AxisTap],
    swap_red_blue: bool,
    start_block: usize,
    end_block: usize,
    luma_out: &mut [u8],
    chroma_out: &mut [u8],
) {
    let mut cache = RowCache::new(width);
    let mut upper = vec![0u32; width];
    let mut current = vec![0u32; width];
    for block_y in start_block..end_block {
        let local_row = block_y - start_block;
        let row_index = local_row * 2 * width;
        blend_output_row_pair(
            frame,
            width,
            x_taps,
            y_taps,
            swap_red_blue,
            block_y,
            &mut cache,
            &mut upper,
            &mut current,
        );
        for column in 0..width {
            luma_out[row_index + column] = luma_from_packed(upper[column]);
            luma_out[row_index + width + column] = luma_from_packed(current[column]);
        }
        let chroma_index = local_row * width;
        for block_x in 0..width / 2 {
            let column0 = block_x * 2;
            let column1 = column0 + 1;
            let average = average_packed(
                upper[column0],
                upper[column1],
                current[column0],
                current[column1],
            );
            let (u, v) = chroma_from_packed(average);
            chroma_out[chroma_index + column0] = u;
            chroma_out[chroma_index + column1] = v;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blend_output_row_pair(
    frame: &VideoFrame,
    width: usize,
    x_taps: &[AxisTap],
    y_taps: &[AxisTap],
    swap_red_blue: bool,
    block_y: usize,
    cache: &mut RowCache,
    upper: &mut [u32],
    current: &mut [u32],
) {
    fill_blended_row(
        frame,
        width,
        x_taps,
        y_taps[block_y * 2],
        swap_red_blue,
        cache,
        upper,
    );
    fill_blended_row(
        frame,
        width,
        x_taps,
        y_taps[block_y * 2 + 1],
        swap_red_blue,
        cache,
        current,
    );
}

fn fill_blended_row(
    frame: &VideoFrame,
    width: usize,
    x_taps: &[AxisTap],
    row_tap: AxisTap,
    swap_red_blue: bool,
    cache: &mut RowCache,
    out: &mut [u32],
) {
    let weight0 = WEIGHT_SCALE - row_tap.weight1;
    let weight1 = row_tap.weight1;
    let first_slot = cache.ensure(row_tap.index0, |index, pixels| {
        pack_scaled_row(
            &frame.data,
            frame.stride,
            index,
            x_taps,
            swap_red_blue,
            pixels,
        )
    });
    let second_slot = cache.ensure(row_tap.index1, |index, pixels| {
        pack_scaled_row(
            &frame.data,
            frame.stride,
            index,
            x_taps,
            swap_red_blue,
            pixels,
        )
    });
    let first = cache.pixels(first_slot);
    let second = cache.pixels(second_slot);
    for column in 0..width {
        out[column] = lerp_packed(first[column], second[column], weight0, weight1);
    }
}

fn pack_scaled_row(
    source: &[u8],
    stride: usize,
    source_row: usize,
    x_taps: &[AxisTap],
    swap_red_blue: bool,
    out: &mut [u32],
) {
    let base = source_row * stride;
    let pointer = source.as_ptr();
    for (index, tap) in x_taps.iter().enumerate() {
        let mut first =
            unsafe { (pointer.add(base + tap.index0 * 4) as *const u32).read_unaligned() };
        let mut second =
            unsafe { (pointer.add(base + tap.index1 * 4) as *const u32).read_unaligned() };
        if swap_red_blue {
            first = swap_red_blue_channels(first);
            second = swap_red_blue_channels(second);
        }
        out[index] = lerp_packed(first, second, WEIGHT_SCALE - tap.weight1, tap.weight1);
    }
}

#[inline]
fn swap_red_blue_channels(pixel: u32) -> u32 {
    (pixel & 0xFF00_FF00) | ((pixel & 0xFF) << 16) | ((pixel >> 16) & 0xFF)
}

#[inline]
fn lerp_packed(first: u32, second: u32, weight0: u32, weight1: u32) -> u32 {
    let even = (((first & PAIRED_CHANNEL_MASK) * weight0
        + (second & PAIRED_CHANNEL_MASK) * weight1
        + 0x0080_0080)
        >> 8)
        & PAIRED_CHANNEL_MASK;
    let odd = ((((first >> 8) & PAIRED_CHANNEL_MASK) * weight0
        + ((second >> 8) & PAIRED_CHANNEL_MASK) * weight1
        + 0x0080_0080)
        >> 8)
        & PAIRED_CHANNEL_MASK;
    even | (odd << 8)
}

#[inline]
fn average_packed(p00: u32, p01: u32, p10: u32, p11: u32) -> u32 {
    let even = (((p00 & PAIRED_CHANNEL_MASK)
        + (p01 & PAIRED_CHANNEL_MASK)
        + (p10 & PAIRED_CHANNEL_MASK)
        + (p11 & PAIRED_CHANNEL_MASK)
        + 0x0002_0002)
        >> 2)
        & PAIRED_CHANNEL_MASK;
    let odd = ((((p00 >> 8) & PAIRED_CHANNEL_MASK)
        + ((p01 >> 8) & PAIRED_CHANNEL_MASK)
        + ((p10 >> 8) & PAIRED_CHANNEL_MASK)
        + ((p11 >> 8) & PAIRED_CHANNEL_MASK)
        + 0x0002_0002)
        >> 2)
        & PAIRED_CHANNEL_MASK;
    even | (odd << 8)
}

#[inline]
fn luma_from_packed(pixel: u32) -> u8 {
    let blue = pixel & 0xFF;
    let green = (pixel >> 8) & 0xFF;
    let red = (pixel >> 16) & 0xFF;
    luma_from(red as i32, green as i32, blue as i32)
}

#[inline]
fn chroma_from_packed(pixel: u32) -> (u8, u8) {
    let blue = (pixel & 0xFF) as i32;
    let green = ((pixel >> 8) & 0xFF) as i32;
    let red = ((pixel >> 16) & 0xFF) as i32;
    chroma_from(red, green, blue)
}

struct RowCache {
    slots: [RowSlot; 2],
}

struct RowSlot {
    source_row: Option<usize>,
    stamp: u64,
    pixels: Vec<u32>,
}

impl RowCache {
    fn new(width: usize) -> Self {
        Self {
            slots: [
                RowSlot {
                    source_row: None,
                    stamp: 0,
                    pixels: vec![0; width],
                },
                RowSlot {
                    source_row: None,
                    stamp: 0,
                    pixels: vec![0; width],
                },
            ],
        }
    }

    fn ensure<F>(&mut self, source_row: usize, fill: F) -> usize
    where
        F: FnOnce(usize, &mut [u32]),
    {
        if let Some(slot) = self
            .slots
            .iter()
            .position(|slot| slot.source_row == Some(source_row))
        {
            self.touch(slot);
            return slot;
        }
        let slot = if self.slots[0].stamp <= self.slots[1].stamp {
            0
        } else {
            1
        };
        fill(source_row, &mut self.slots[slot].pixels);
        self.slots[slot].source_row = Some(source_row);
        self.touch(slot);
        slot
    }

    fn pixels(&self, slot: usize) -> &[u32] {
        &self.slots[slot].pixels
    }

    fn touch(&mut self, slot: usize) {
        let stamp = self.slots[0].stamp.max(self.slots[1].stamp) + 1;
        self.slots[slot].stamp = stamp;
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn bgra_to_bgra_scaled(
    frame: &VideoFrame,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    validate_yuv_target(target_width, target_height)?;
    let (source_width, source_height) = validate_rgb_source(frame)?;
    let width = target_width as usize;
    let height = target_height as usize;
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| NexusError::invalid("BGRA 输出行宽溢出"))?;
    let mut data = vec![0u8; row_bytes * height];

    if source_width == width && source_height == height {
        for row in 0..height {
            let source = &frame.data[row * frame.stride..row * frame.stride + row_bytes];
            let target = &mut data[row * row_bytes..(row + 1) * row_bytes];
            for (source_pixel, target_pixel) in
                source.chunks_exact(4).zip(target.chunks_exact_mut(4))
            {
                let (red, green, blue) = match frame.format {
                    PixelFormat::Bgra8 => (source_pixel[2], source_pixel[1], source_pixel[0]),
                    PixelFormat::Rgba8 => (source_pixel[0], source_pixel[1], source_pixel[2]),
                    PixelFormat::Nv12 | PixelFormat::I420 => {
                        unreachable!("格式已在入口校验")
                    }
                };
                target_pixel.copy_from_slice(&[blue, green, red, 0xFF]);
            }
        }
        return Ok(data);
    }

    let x_taps = axis_taps(width, source_width);
    let y_taps = axis_taps(height, source_height);
    for (row, tap_y) in y_taps.iter().enumerate() {
        for (column, tap_x) in x_taps.iter().enumerate() {
            let [red, green, blue] = sample_rgb(frame, *tap_x, *tap_y);
            let offset = row * row_bytes + column * 4;
            data[offset] = blue as u8;
            data[offset + 1] = green as u8;
            data[offset + 2] = red as u8;
            data[offset + 3] = 0xFF;
        }
    }
    Ok(data)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn bgra_to_nv12(
    frame: &VideoFrame,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    validate_yuv_target(target_width, target_height)?;
    let (source_width, source_height) = validate_rgb_source(frame)?;

    if frame.width == target_width && frame.height == target_height {
        return Ok(bgra_to_nv12_same_size(frame, source_width, source_height));
    }

    let width = target_width as usize;
    let height = target_height as usize;
    let luma_len = width * height;
    let mut data = vec![0u8; luma_len + luma_len / 2];
    let (luma, chroma) = data.split_at_mut(luma_len);
    scaled_luma_and_chroma(
        frame,
        source_width,
        source_height,
        width,
        height,
        luma,
        chroma,
    );
    Ok(data)
}

fn bgra_to_nv12_same_size(frame: &VideoFrame, width: usize, height: usize) -> Vec<u8> {
    let chroma_height = height / 2;
    let luma_len = width * height;
    let mut data = vec![0u8; luma_len + luma_len / 2];
    let (luma_plane, chroma_plane) = data.split_at_mut(luma_len);
    let (r_offset, g_offset, b_offset) = match frame.format {
        PixelFormat::Bgra8 => (2, 1, 0),
        PixelFormat::Rgba8 => (0, 1, 2),
        PixelFormat::Nv12 | PixelFormat::I420 => unreachable!("格式已在入口校验"),
    };

    let threads = scaled_conversion_threads(chroma_height);
    if threads <= 1 {
        process_same_size_blocks(
            frame,
            width,
            (r_offset, g_offset, b_offset),
            0,
            chroma_height,
            luma_plane,
            chroma_plane,
        );
        return data;
    }

    let blocks_per_chunk = chroma_height.div_ceil(threads);
    let luma_chunk_len = blocks_per_chunk * 2 * width;
    let chroma_chunk_len = blocks_per_chunk * width;
    std::thread::scope(|scope| {
        let luma_chunks = luma_plane.chunks_mut(luma_chunk_len);
        let chroma_chunks = chroma_plane.chunks_mut(chroma_chunk_len);
        for (chunk_index, (luma_chunk, chroma_chunk)) in luma_chunks.zip(chroma_chunks).enumerate()
        {
            let start_block = chunk_index * blocks_per_chunk;
            let end_block = (start_block + blocks_per_chunk).min(chroma_height);
            if start_block >= end_block {
                continue;
            }
            scope.spawn(move || {
                process_same_size_blocks(
                    frame,
                    width,
                    (r_offset, g_offset, b_offset),
                    start_block,
                    end_block,
                    luma_chunk,
                    chroma_chunk,
                );
            });
        }
    });
    data
}

#[allow(clippy::too_many_arguments)]
fn process_same_size_blocks(
    frame: &VideoFrame,
    width: usize,
    offsets: (usize, usize, usize),
    start_block: usize,
    end_block: usize,
    luma_out: &mut [u8],
    chroma_out: &mut [u8],
) {
    let chroma_width = width / 2;
    let source = &frame.data;
    let stride = frame.stride;
    let (r_offset, g_offset, b_offset) = offsets;
    debug_assert!(source.len() >= stride * frame.height as usize);

    for block_y in start_block..end_block {
        let local_row = block_y - start_block;
        let line0 = block_y * 2 * stride;
        let line1 = line0 + stride;
        let out0 = local_row * 2 * width;
        let out1 = out0 + width;
        let chroma_row = local_row * width;
        for block_x in 0..chroma_width {
            let column0 = block_x * 2;
            let column1 = column0 + 1;
            let i00 = line0 + column0 * 4;
            let i01 = line0 + column1 * 4;
            let i10 = line1 + column0 * 4;
            let i11 = line1 + column1 * 4;
            let (r00, g00, b00) = unsafe {
                (
                    *source.get_unchecked(i00 + r_offset) as i32,
                    *source.get_unchecked(i00 + g_offset) as i32,
                    *source.get_unchecked(i00 + b_offset) as i32,
                )
            };
            let (r01, g01, b01) = unsafe {
                (
                    *source.get_unchecked(i01 + r_offset) as i32,
                    *source.get_unchecked(i01 + g_offset) as i32,
                    *source.get_unchecked(i01 + b_offset) as i32,
                )
            };
            let (r10, g10, b10) = unsafe {
                (
                    *source.get_unchecked(i10 + r_offset) as i32,
                    *source.get_unchecked(i10 + g_offset) as i32,
                    *source.get_unchecked(i10 + b_offset) as i32,
                )
            };
            let (r11, g11, b11) = unsafe {
                (
                    *source.get_unchecked(i11 + r_offset) as i32,
                    *source.get_unchecked(i11 + g_offset) as i32,
                    *source.get_unchecked(i11 + b_offset) as i32,
                )
            };

            luma_out[out0 + column0] = luma_from(r00, g00, b00);
            luma_out[out0 + column1] = luma_from(r01, g01, b01);
            luma_out[out1 + column0] = luma_from(r10, g10, b10);
            luma_out[out1 + column1] = luma_from(r11, g11, b11);

            let r = (r00 + r01 + r10 + r11 + 2) / 4;
            let g = (g00 + g01 + g10 + g11 + 2) / 4;
            let b = (b00 + b01 + b10 + b11 + 2) / 4;
            let (u, v) = chroma_from(r, g, b);
            chroma_out[chroma_row + column0] = u;
            chroma_out[chroma_row + column1] = v;
        }
    }
}

#[inline]
fn luma_from(r: i32, g: i32, b: i32) -> u8 {
    (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16).clamp(16, 235) as u8
}

#[inline]
fn chroma_from(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = ((113 * b - 26 * r - 87 * g + 128) >> 8) + 128;
    let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

pub fn i420_to_bgra(frame: &YuvFrame) -> Result<VideoFrame> {
    if frame.width < 2 || frame.height < 2 {
        return Err(NexusError::invalid("I420 帧宽高必须至少为 2"));
    }
    if frame.width % 2 != 0 || frame.height % 2 != 0 {
        return Err(NexusError::invalid("I420 帧宽高必须为偶数"));
    }
    if !frame.is_valid() {
        return Err(NexusError::invalid("I420 平面长度与宽高不匹配"));
    }

    let width = frame.width;
    let height = frame.height;
    let chroma_width = width / 2;
    let output_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| NexusError::invalid("BGRA 输出尺寸溢出"))?;
    let mut data = vec![0u8; output_len];

    for row in 0..height {
        let chroma_row = (row / 2) * chroma_width;
        for column in 0..width {
            let chroma_index = chroma_row + column / 2;
            let luma = i32::from(frame.y[row * width + column]);
            let cb = i32::from(frame.u[chroma_index]) - 128;
            let cr = i32::from(frame.v[chroma_index]) - 128;

            let red = ((298 * (luma - 16) + 459 * cr + 128) >> 8).clamp(0, 255);
            let green = ((298 * (luma - 16) - 55 * cb - 136 * cr + 128) >> 8).clamp(0, 255);
            let blue = ((298 * (luma - 16) + 541 * cb + 128) >> 8).clamp(0, 255);

            let offset = (row * width + column) * 4;
            data[offset] = blue as u8;
            data[offset + 1] = green as u8;
            data[offset + 2] = red as u8;
            data[offset + 3] = 255;
        }
    }

    Ok(VideoFrame {
        width: width as u32,
        height: height as u32,
        stride: width * 4,
        format: PixelFormat::Bgra8,
        timestamp_us: 0,
        data,
    })
}

fn axis_taps(destination: usize, source: usize) -> Vec<AxisTap> {
    (0..destination)
        .map(|index| {
            let position = ((index as f64 + 0.5) * source as f64 / destination as f64 - 0.5)
                .clamp(0.0, (source - 1) as f64);
            let position_q8 = (position * WEIGHT_SCALE as f64).round() as usize;
            let index0 = (position_q8 / WEIGHT_SCALE as usize).min(source - 1);
            AxisTap {
                index0,
                index1: (index0 + 1).min(source - 1),
                weight1: (position_q8 % WEIGHT_SCALE as usize) as u32,
            }
        })
        .collect()
}

fn sample_rgb(frame: &VideoFrame, column: AxisTap, row: AxisTap) -> [i32; 3] {
    let pixel00 = read_rgb(frame, row.index0, column.index0);
    let pixel01 = read_rgb(frame, row.index0, column.index1);
    let pixel10 = read_rgb(frame, row.index1, column.index0);
    let pixel11 = read_rgb(frame, row.index1, column.index1);

    let weight_x0 = WEIGHT_SCALE - column.weight1;
    let weight_x1 = column.weight1;
    let weight_y0 = WEIGHT_SCALE - row.weight1;
    let weight_y1 = row.weight1;

    let mut result = [0; 3];
    for channel in 0..3 {
        let top =
            (pixel00[channel] * weight_x0 as i32 + pixel01[channel] * weight_x1 as i32 + 128) >> 8;
        let bottom =
            (pixel10[channel] * weight_x0 as i32 + pixel11[channel] * weight_x1 as i32 + 128) >> 8;
        result[channel] = (top * weight_y0 as i32 + bottom * weight_y1 as i32 + 128) >> 8;
    }
    result
}

fn read_rgb(frame: &VideoFrame, row: usize, column: usize) -> [i32; 3] {
    let offset = row * frame.stride + column * 4;
    match frame.format {
        PixelFormat::Bgra8 => [
            i32::from(frame.data[offset + 2]),
            i32::from(frame.data[offset + 1]),
            i32::from(frame.data[offset]),
        ],
        PixelFormat::Rgba8 => [
            i32::from(frame.data[offset]),
            i32::from(frame.data[offset + 1]),
            i32::from(frame.data[offset + 2]),
        ],
        PixelFormat::Nv12 | PixelFormat::I420 => unreachable!("格式已在入口校验"),
    }
}

