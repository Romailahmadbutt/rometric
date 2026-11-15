//! Embedded visual identity shared by the window, tray and application UI.

pub const APP_NAME: &str = "Rometric";
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogoRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn logo_rgba() -> Result<LogoRgba, String> {
    use image::GenericImageView;

    let image = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png)
        .map_err(|err| format!("Could not decode application logo: {err}"))?;
    let (width, height) = image.dimensions();
    Ok(LogoRgba {
        width,
        height,
        rgba: image.into_rgba8().into_raw(),
    })
}
