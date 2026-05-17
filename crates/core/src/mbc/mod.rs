mod header;
mod mbc1;
mod rom_only;

pub use header::CartridgeHeader;
pub use mbc1::MBC1;
pub use rom_only::ROMOnly;

/// Cartridge memory bank controller interface.
///
/// Implementations handle all reads and writes to the cartridge address space:
/// - ROM Bank 0: `0x0000–0x3FFF`
/// - ROM Bank N: `0x4000–0x7FFF`
/// - External RAM: `0xA000–0xBFFF`
pub trait MBC {
    fn read(&self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

/// Parse the cartridge header, validate it, and return the appropriate MBC
/// implementation as a `Box<dyn MBC>`.
///
/// # Panics
///
/// - If the header is invalid (logo mismatch, checksum error, ROM too short).
/// - If the MBC type byte (`0x0147`) is not yet supported.
pub fn from_rom(rom: Vec<u8>) -> Box<dyn MBC> {
    let header = CartridgeHeader::parse(&rom);

    match header.mbc_type {
        0x00 => {
            eprintln!(
                "[cartridge] ROM-only: \"{}\" (MBC type {:#04X})",
                header.title, header.mbc_type
            );
            Box::new(ROMOnly::new(rom))
        }
        0x01..=0x03 => {
            eprintln!(
                "[cartridge] MBC1: \"{}\" (MBC type {:#04X})",
                header.title, header.mbc_type
            );
            Box::new(MBC1::new(rom))
        }
        t => panic!("Unsupported MBC type: {:#04X}", t),
    }
}
