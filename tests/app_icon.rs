//! The embedded icon must carry every size Windows picks for the tray,
//! taskbar, Alt+Tab and Explorer.

const REQUIRED_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// Reads the ICONDIR entries as `(width, height, bits_per_pixel)`.
fn icon_entries(bytes: &[u8]) -> Vec<(u32, u32, u16)> {
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    (0..count)
        .map(|index| {
            let entry = &bytes[6 + index * 16..6 + (index + 1) * 16];
            // A stored 0 means 256.
            let dimension = |value: u8| if value == 0 { 256 } else { u32::from(value) };
            (
                dimension(entry[0]),
                dimension(entry[1]),
                u16::from_le_bytes([entry[6], entry[7]]),
            )
        })
        .collect()
}

#[test]
fn app_icon_has_required_sizes() {
    // Arrange
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/skrivli.ico");
    let bytes = std::fs::read(path).expect("icon file is readable");

    // Act
    let entries = icon_entries(&bytes);

    // Assert
    for size in REQUIRED_SIZES {
        assert!(
            entries.contains(&(size, size, 32)),
            "missing {size}x{size} 32-bit image, found {entries:?}"
        );
    }
}
