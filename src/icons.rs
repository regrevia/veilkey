pub fn application() -> eframe::egui::IconData {
    eframe::egui::IconData {
        rgba: include_bytes!("../assets/icons/application-128.rgba").to_vec(),
        width: 128,
        height: 128,
    }
}

#[cfg(target_os = "windows")]
pub fn tray_rgba() -> Vec<u8> {
    include_bytes!("../assets/icons/tray-32.rgba").to_vec()
}

#[cfg(target_os = "macos")]
pub fn tray_rgba() -> Vec<u8> {
    include_bytes!("../assets/icons/tray-template-44.rgba").to_vec()
}

#[cfg(target_os = "windows")]
pub const TRAY_SIZE: u32 = 32;

#[cfg(target_os = "macos")]
pub const TRAY_SIZE: u32 = 44;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_application_and_tray_icons_have_valid_rgba_dimensions() {
        let application = application();
        assert_eq!(
            application.rgba.len(),
            (application.width * application.height * 4) as usize
        );
        assert_eq!(
            application.rgba[3], 0,
            "application icon background must be transparent"
        );

        let tray = tray_rgba();
        assert_eq!(tray.len(), (TRAY_SIZE * TRAY_SIZE * 4) as usize);
        assert_eq!(tray[3], 0, "tray icon background must be transparent");
        assert!(tray.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }
}
