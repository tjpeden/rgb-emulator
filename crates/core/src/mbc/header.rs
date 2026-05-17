/// Nintendo logo bitmap stored at `0x0104–0x0133` in the cartridge header.
///
/// The DMG boot ROM validates this logo before transferring control to the
/// game — we do the same validation here. Source: Pan Docs.
const NINTENDO_LOGO: [u8; 48] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B,
    0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
    0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E,
    0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
    0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC,
    0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

/// Parsed and validated cartridge header fields.
pub struct CartridgeHeader {
    /// Game title extracted from `0x0134–0x0143`, trailing NUL bytes stripped.
    pub title: String,
    /// MBC type byte from `0x0147`.
    pub mbc_type: u8,
}

impl CartridgeHeader {
    /// Parse and validate the cartridge header from a ROM image.
    ///
    /// # Panics
    ///
    /// - If the ROM is too short to contain a valid header.
    /// - If the Nintendo logo at `0x0104–0x0133` does not match.
    /// - If the header checksum at `0x014D` is incorrect.
    pub fn parse(rom: &[u8]) -> Self {
        assert!(
            rom.len() > 0x014D,
            "ROM too short to contain a valid header (got {} bytes)",
            rom.len()
        );

        // Nintendo logo validation — 0x0104–0x0133 (48 bytes)
        let logo = &rom[0x0104..=0x0133];
        assert_eq!(
            logo,
            &NINTENDO_LOGO[..],
            "Nintendo logo mismatch — ROM is not a valid Game Boy cartridge"
        );

        // Header checksum — 0x014D
        // Algorithm: x = 0; for i in 0x0134..=0x014C: x = x.wrapping_sub(rom[i]).wrapping_sub(1)
        let mut checksum: u8 = 0;
        for &byte in &rom[0x0134..=0x014C] {
            checksum = checksum.wrapping_sub(byte).wrapping_sub(1);
        }
        assert_eq!(
            checksum, rom[0x014D],
            "Header checksum mismatch: computed {:#04X}, expected {:#04X}",
            checksum, rom[0x014D]
        );

        // Title — 0x0134–0x0143 (up to 16 bytes, ASCII, NUL-padded)
        let raw_title = &rom[0x0134..=0x0143];
        let title = raw_title
            .iter()
            .copied()
            .take_while(|&b| b != 0x00)
            .map(|b| b as char)
            .collect::<String>();

        let mbc_type = rom[0x0147];

        Self { title, mbc_type }
    }
}
