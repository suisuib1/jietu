use image::{
    ColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder},
};

pub(crate) fn png_bytes(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, PngFilterType::Adaptive)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ColorType::Rgba8,
        )
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

pub(crate) fn image_is_blank(image: &RgbaImage) -> bool {
    if image.width() < 2 || image.height() < 2 {
        return true;
    }
    let first = image.get_pixel(0, 0);
    let step_x = (image.width() / 30).max(1);
    let step_y = (image.height() / 20).max(1);
    let mut varied = 0;
    for y in (0..image.height()).step_by(step_y as usize) {
        for x in (0..image.width()).step_by(step_x as usize) {
            let pixel = image.get_pixel(x, y);
            let difference = (pixel[0] as i16 - first[0] as i16).unsigned_abs()
                + (pixel[1] as i16 - first[1] as i16).unsigned_abs()
                + (pixel[2] as i16 - first[2] as i16).unsigned_abs();
            if difference > 36 {
                varied += 1;
                if varied > 6 {
                    return false;
                }
            }
        }
    }
    true
}
