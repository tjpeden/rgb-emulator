mod rom_only;

pub use rom_only::RomOnly;

/// Cartridge memory bank controller interface.
///
/// Implementations handle all reads and writes to the cartridge address space:
/// - ROM Bank 0: `0x0000–0x3FFF`
/// - ROM Bank N: `0x4000–0x7FFF`
/// - External RAM: `0xA000–0xBFFF`
pub trait Mbc {
    fn read(&self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

/// Detect the correct MBC from the cartridge header byte at `0x0147`
/// and return a boxed trait object.
pub fn from_rom(rom: Vec<u8>) -> Box<dyn Mbc> {
    let mbc_type = rom.get(0x0147).copied().unwrap_or(0x00);
    match mbc_type {
        0x00 => Box::new(RomOnly::new(rom)),
        _ => panic!("Unsupported MBC type: {:#04X}", mbc_type),
    }
}
