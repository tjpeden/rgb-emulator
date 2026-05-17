use super::MBC;

/// ROM-only cartridge (MBC type `0x00`). No banking, no external RAM.
/// Writes to ROM space are silently ignored per hardware behaviour.
pub struct ROMOnly {
    rom: Vec<u8>,
}

impl ROMOnly {
    pub fn new(rom: Vec<u8>) -> Self {
        Self { rom }
    }
}

impl MBC for ROMOnly {
    fn read(&self, addr: u16) -> u8 {
        // Both ROM Bank 0 (0x0000–0x3FFF) and Bank N (0x4000–0x7FFF)
        // map directly — ROM-only has no banking.
        self.rom.get(addr as usize).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, _addr: u16, _value: u8) {
        // ROM is not writable; ignore silently.
    }
}
