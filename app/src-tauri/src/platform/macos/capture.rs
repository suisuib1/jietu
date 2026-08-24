#[cfg(not(target_os = "windows"))]
use image::RgbaImage;
#[cfg(not(target_os = "windows"))]
use screenshots::Screen;

#[cfg(not(target_os = "windows"))]
pub(crate) fn capture_screen(screen: &Screen) -> Result<RgbaImage, String> {
    screen.capture().map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn capture_region(
    screen: &Screen,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<RgbaImage, String> {
    screen
        .capture_area(x, y, width, height)
        .map_err(|error| error.to_string())
}
