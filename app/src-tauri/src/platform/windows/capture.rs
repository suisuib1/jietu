use crate::core::capture::Rect;
use image::RgbaImage;
use screenshots::Screen;

pub(crate) fn capture_screen(screen: &Screen) -> Result<RgbaImage, String> {
    let size = screen.display_info;
    screen
        .capture_area_ignore_area_check(0, 0, size.width, size.height)
        .map_err(|error| error.to_string())
}

pub(crate) fn capture_region(
    screen: &Screen,
    rect: Rect,
    bounds: Rect,
    scale_factor: f64,
) -> Result<RgbaImage, String> {
    let (x, y, width, height) = crate::core::capture::logical_region(rect, bounds);
    let scale = scale_factor.max(1.0);
    let physical_x = (x as f64 * scale).floor() as i32;
    let physical_y = (y as f64 * scale).floor() as i32;
    let physical_right = ((x as f64 + width as f64) * scale).ceil() as u32;
    let physical_bottom = ((y as f64 + height as f64) * scale).ceil() as u32;
    screen
        .capture_area_ignore_area_check(
            physical_x,
            physical_y,
            physical_right
                .saturating_sub(physical_x.max(0) as u32)
                .max(1),
            physical_bottom
                .saturating_sub(physical_y.max(0) as u32)
                .max(1),
        )
        .map_err(|error| error.to_string())
}
