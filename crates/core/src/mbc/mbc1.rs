use super::MBC;

/// MBC1 cartridge controller.
///
/// Supports up to 2 MiB ROM (128 banks × 16 KiB) and 32 KiB external RAM
/// (4 banks × 8 KiB). Implements both ROM banking mode and RAM banking mode.
pub struct MBC1 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    /// Lower 5 bits of ROM bank number (registers 1).
    rom_bank_lo: u8,
    /// Upper 2 bits — used as ROM bank hi or RAM bank depending on mode.
    bank2: u8,
    /// Banking mode: false = ROM banking (default), true = RAM banking.
    ram_banking_mode: bool,
    /// External RAM enabled.
    ram_enabled: bool,
}

impl MBC1 {
    pub fn new(rom: Vec<u8>) -> Self {
        let ram = vec![0u8; 0x8000]; // 32 KiB max external RAM
        Self {
            rom,
            ram,
            rom_bank_lo: 1,
            bank2: 0,
            ram_banking_mode: false,
            ram_enabled: false,
        }
    }

    fn rom_bank_0_offset(&self) -> usize {
        if self.ram_banking_mode {
            // In RAM banking mode, upper 2 bits select which 1 MiB chunk bank 0 maps to
            (self.bank2 as usize) << 19
        } else {
            0
        }
    }

    fn rom_bank_n_offset(&self) -> usize {
        let lo = if self.rom_bank_lo == 0 { 1 } else { self.rom_bank_lo };
        let bank = ((self.bank2 as usize) << 5) | (lo as usize);
        bank * 0x4000
    }

    fn ram_bank_offset(&self) -> usize {
        if self.ram_banking_mode {
            (self.bank2 as usize) * 0x2000
        } else {
            0
        }
    }
}

impl MBC for MBC1 {
    fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => {
                let offset = self.rom_bank_0_offset() + addr as usize;
                self.rom.get(offset).copied().unwrap_or(0xFF)
            }
            0x4000..=0x7FFF => {
                let offset = self.rom_bank_n_offset() + (addr as usize - 0x4000);
                self.rom.get(offset).copied().unwrap_or(0xFF)
            }
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    let offset = self.ram_bank_offset() + (addr as usize - 0xA000);
                    self.ram.get(offset).copied().unwrap_or(0xFF)
                } else {
                    0xFF
                }
            }
            _ => 0xFF,
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            // RAM enable: write 0x0A to lower nibble enables RAM
            0x0000..=0x1FFF => {
                self.ram_enabled = (value & 0x0F) == 0x0A;
            }
            // ROM bank number (lower 5 bits)
            0x2000..=0x3FFF => {
                self.rom_bank_lo = value & 0x1F;
            }
            // Upper 2 bits (RAM bank / ROM bank hi)
            0x4000..=0x5FFF => {
                self.bank2 = value & 0x03;
            }
            // Banking mode select
            0x6000..=0x7FFF => {
                self.ram_banking_mode = (value & 0x01) != 0;
            }
            // External RAM write
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    let offset = self.ram_bank_offset() + (addr as usize - 0xA000);
                    if let Some(byte) = self.ram.get_mut(offset) {
                        *byte = value;
                    }
                }
            }
            _ => {}
        }
    }
}
