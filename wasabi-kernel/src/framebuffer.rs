use bootloader_api::info::FrameBuffer;

pub fn clear_frame_buffer(fb: &mut FrameBuffer, r: u8, g: u8, b: u8) {
    let info = fb.info().clone();
    let buffer = fb.buffer_mut();

    for y in 0..info.height {
        for x in 0..info.width {
            let pos = info.bytes_per_pixel * info.stride * y + x * info.bytes_per_pixel;
            match info.pixel_format {
                bootloader_api::info::PixelFormat::Rgb => {
                    buffer[pos] = r;
                    buffer[pos + 1] = g;
                    buffer[pos + 2] = b;
                }
                bootloader_api::info::PixelFormat::Bgr => {
                    buffer[pos] = b;
                    buffer[pos + 1] = g;
                    buffer[pos + 2] = r;
                }
                bootloader_api::info::PixelFormat::U8 => {
                    let gray = ((r as u32 + g as u32 + b as u32) / 3) as u8;
                    buffer[pos] = gray;
                }
                _ => panic!("unknown pixel format"),
            }
        }
    }
}
