use rometric::branding::{logo_rgba, APP_NAME};

#[test]
fn application_branding_uses_the_rometric_name_and_logo() {
    assert_eq!(APP_NAME, "Rometric");
    let logo = logo_rgba().expect("embedded application logo decodes");

    assert_eq!((logo.width, logo.height), (256, 256));
    assert_eq!(logo.rgba.len(), 256 * 256 * 4);
    let (pixels, remainder) = logo.rgba.as_chunks::<4>();
    assert!(remainder.is_empty());
    assert!(pixels.iter().any(|pixel| pixel[3] == 0));
    assert!(pixels
        .iter()
        .any(|pixel| { pixel[0] < 80 && pixel[1] > 80 && pixel[2] > 180 && pixel[3] > 200 }));
}
