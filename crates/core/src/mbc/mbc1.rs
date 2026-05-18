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
    /// Whether this cartridge has a battery-backed RAM (type `0x03`).
    has_battery: bool,
}

impl MBC1 {
    pub fn new(rom: Vec<u8>, has_battery: bool) -> Self {
        let ram = vec![0u8; 0x8000]; // 32 KiB max external RAM
        Self {
            rom,
            ram,
            rom_bank_lo: 1,
            bank2: 0,
            ram_banking_mode: false,
            ram_enabled: false,
            has_battery,
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

    fn battery_ram(&self) -> Option<&[u8]> {
        if self.has_battery {
            Some(&self.ram)
        } else {
            None
        }
    }

    fn load_battery_ram(&mut self, data: &[u8]) {
        let len = data.len().min(self.ram.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal MBC1 ROM of `num_banks` × 16 KiB where each bank is
    /// filled with the bank index so reads can be verified.
    fn make_rom(num_banks: usize) -> Vec<u8> {
        let mut rom = vec![0u8; num_banks * 0x4000];
        for bank in 0..num_banks {
            let base = bank * 0x4000;
            for byte in rom[base..base + 0x4000].iter_mut() {
                *byte = bank as u8;
            }
        }
        rom
    }

    // --- ROM banking ---

    #[test]
    fn rom_bank_0_always_reads_bank_0() {
        // Bank 0 at 0x0000–0x3FFF must always map to physical bank 0,
        // regardless of the ROM bank register.
        let rom = make_rom(4);
        let mut mbc = MBC1::new(rom, false);
        // Select bank 2
        mbc.write(0x2000, 2);
        assert_eq!(mbc.read(0x0000), 0, "bank 0 window must always be physical bank 0");
        assert_eq!(mbc.read(0x3FFF), 0, "bank 0 window must always be physical bank 0");
    }

    #[test]
    fn rom_bank_n_window_selects_correct_bank() {
        // Writing N to 0x2000–0x3FFF makes bank N visible at 0x4000–0x7FFF.
        let rom = make_rom(8);
        let mut mbc = MBC1::new(rom, false);
        for bank in 1u8..8 {
            mbc.write(0x2000, bank);
            assert_eq!(
                mbc.read(0x4000),
                bank,
                "bank {} should be visible at 0x4000",
                bank
            );
        }
    }

    #[test]
    fn bank_0_write_remaps_to_bank_1() {
        // Writing 0x00 to the ROM bank register is treated as 0x01 (bank 0
        // is never mapped to the 0x4000 window).
        let rom = make_rom(4);
        let mut mbc = MBC1::new(rom, false);
        mbc.write(0x2000, 0x00);
        // Physical bank 1 contains 0x01 in every byte.
        assert_eq!(mbc.read(0x4000), 1, "writing 0x00 must remap to bank 1");
    }

    #[test]
    fn bank_0x20_remaps_to_0x21() {
        // Banks 0x20, 0x40, 0x60 must also remap to 0x21, 0x41, 0x61.
        let rom = make_rom(64);
        let mut mbc = MBC1::new(rom, false);
        // bank2 = 1, rom_bank_lo = 0 → combined 0x20 → remap to 0x21
        mbc.write(0x4000, 0x01); // bank2 = 1
        mbc.write(0x2000, 0x00); // lo = 0 → remap to 1
        assert_eq!(mbc.read(0x4000), 0x21, "bank 0x20 must remap to 0x21");
    }

    #[test]
    fn bank_0x40_remaps_to_0x41() {
        let rom = make_rom(128);
        let mut mbc = MBC1::new(rom, false);
        mbc.write(0x4000, 0x02); // bank2 = 2
        mbc.write(0x2000, 0x00); // lo = 0 → remap to 1
        assert_eq!(mbc.read(0x4000), 0x41, "bank 0x40 must remap to 0x41");
    }

    #[test]
    fn bank_0x60_remaps_to_0x61() {
        let rom = make_rom(128);
        let mut mbc = MBC1::new(rom, false);
        mbc.write(0x4000, 0x03); // bank2 = 3
        mbc.write(0x2000, 0x00); // lo = 0 → remap to 1
        assert_eq!(mbc.read(0x4000), 0x61, "bank 0x60 must remap to 0x61");
    }

    #[test]
    fn upper_2_bits_extend_rom_bank() {
        // bank2 | lo selects the correct bank in ROM banking mode.
        let rom = make_rom(64);
        let mut mbc = MBC1::new(rom, false);
        mbc.write(0x4000, 0x01); // bank2 = 1
        mbc.write(0x2000, 0x02); // lo = 2 → bank = 0x22
        assert_eq!(mbc.read(0x4000), 0x22, "bank2=1 + lo=2 must select bank 0x22");
    }

    // --- RAM banking ---

    #[test]
    fn ram_disabled_by_default_returns_0xff() {
        let mbc = MBC1::new(make_rom(2), false);
        assert_eq!(mbc.read(0xA000), 0xFF, "RAM reads when disabled must return 0xFF");
    }

    #[test]
    fn ram_enable_and_read_write() {
        let mut mbc = MBC1::new(make_rom(2), false);
        mbc.write(0x0000, 0x0A); // enable RAM
        mbc.write(0xA000, 0xBE);
        assert_eq!(mbc.read(0xA000), 0xBE, "RAM write/read after enable must work");
    }

    #[test]
    fn ram_disable_prevents_writes() {
        let mut mbc = MBC1::new(make_rom(2), false);
        mbc.write(0x0000, 0x0A); // enable
        mbc.write(0xA000, 0xAB);
        mbc.write(0x0000, 0x00); // disable
        mbc.write(0xA000, 0xFF); // write while disabled — ignored
        mbc.write(0x0000, 0x0A); // re-enable
        assert_eq!(mbc.read(0xA000), 0xAB, "write while disabled must be ignored");
    }

    #[test]
    fn ram_banking_mode_selects_ram_bank() {
        let mut mbc = MBC1::new(make_rom(2), false);
        mbc.write(0x0000, 0x0A); // enable RAM
        mbc.write(0x6000, 0x01); // RAM banking mode
        // Write a sentinel to bank 0 and bank 2 of RAM
        mbc.write(0x4000, 0x00); // bank2 = 0 → RAM bank 0
        mbc.write(0xA000, 0x11);
        mbc.write(0x4000, 0x02); // bank2 = 2 → RAM bank 2
        mbc.write(0xA000, 0x22);
        // Verify they are distinct
        mbc.write(0x4000, 0x00);
        assert_eq!(mbc.read(0xA000), 0x11, "RAM bank 0 must hold 0x11");
        mbc.write(0x4000, 0x02);
        assert_eq!(mbc.read(0xA000), 0x22, "RAM bank 2 must hold 0x22");
    }

    // --- Detection (via mod.rs) ---

    #[test]
    fn from_rom_detects_mbc1_type_01() {
        use crate::mbc;
        // Build a minimal valid cartridge ROM with MBC type 0x01.
        let mut rom = vec![0u8; 0x8000];
        let logo: [u8; 48] = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B,
            0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
            0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E,
            0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
            0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC,
            0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        rom[0x0104..=0x0133].copy_from_slice(&logo);
        rom[0x0147] = 0x01; // MBC1
        let mut checksum: u8 = 0;
        for &b in &rom[0x0134..=0x014C] {
            checksum = checksum.wrapping_sub(b).wrapping_sub(1);
        }
        rom[0x014D] = checksum;
        // Should not panic — from_rom must dispatch to MBC1
        let _cart = mbc::from_rom(rom);
    }
}
