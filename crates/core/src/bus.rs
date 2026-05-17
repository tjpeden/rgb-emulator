use crate::mbc::{self, MBC};

/// DMG memory bus.
///
/// Owns all addressable memory regions and dispatches reads/writes via
/// `match` on `u16` address ranges, matching the hardware memory map.
///
/// # Memory Map
///
/// | Range           | Region           | Notes                        |
/// |-----------------|------------------|------------------------------|
/// | `0x0000–0x3FFF` | ROM Bank 0       | Fixed                        |
/// | `0x4000–0x7FFF` | ROM Bank N       | Switchable via MBC           |
/// | `0x8000–0x9FFF` | VRAM             | 8 KiB                        |
/// | `0xA000–0xBFFF` | External RAM     | MBC-controlled               |
/// | `0xC000–0xDFFF` | WRAM             | 8 KiB                        |
/// | `0xE000–0xFDFF` | Echo RAM         | Mirrors `0xC000–0xDDFF`      |
/// | `0xFE00–0xFE9F` | OAM              | 40 sprites × 4 bytes         |
/// | `0xFEA0–0xFEFF` | Unused           | Reads return `0xFF`          |
/// | `0xFF00–0xFF7F` | IO Registers     | 128 bytes                    |
/// | `0xFF80–0xFFFE` | HRAM             | 127 bytes                    |
/// | `0xFFFF`        | IE Register      | Interrupt Enable             |
pub struct Bus {
    cartridge: Box<dyn MBC>,
    vram: [u8; 0x2000], // 8 KiB — 0x8000–0x9FFF
    wram: [u8; 0x2000], // 8 KiB — 0xC000–0xDFFF
    oam: [u8; 0xA0],    // 160 B — 0xFE00–0xFE9F
    io: [u8; 0x80],     // 128 B — 0xFF00–0xFF7F
    hram: [u8; 0x7F],   // 127 B — 0xFF80–0xFFFE
    pub ie: u8,         // 0xFFFF — Interrupt Enable
}

impl Bus {
    /// Create a new Bus from a ROM image.
    /// The MBC type is detected from header byte `0x0147`.
    pub fn new(rom: Vec<u8>) -> Self {
        Self {
            cartridge: mbc::from_rom(rom),
            vram: [0u8; 0x2000],
            wram: [0u8; 0x2000],
            oam: [0u8; 0xA0],
            io: [0u8; 0x80],
            hram: [0u8; 0x7F],
            ie: 0x00,
        }
    }

    /// Read a byte from the given address.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            // Cartridge: ROM Bank 0 + Bank N
            0x0000..=0x7FFF => self.cartridge.read(addr),

            // VRAM
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize],

            // External RAM (MBC-controlled)
            0xA000..=0xBFFF => self.cartridge.read(addr),

            // WRAM
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],

            // Echo RAM — mirrors 0xC000–0xDDFF
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],

            // OAM
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize],

            // Unused region — hardware returns 0xFF
            0xFEA0..=0xFEFF => 0xFF,

            // IO Registers
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize],

            // HRAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],

            // IE Register
            0xFFFF => self.ie,
        }
    }

    /// Write a byte to the given address.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            // Cartridge: ROM writes forwarded to MBC (banking registers etc.)
            0x0000..=0x7FFF => self.cartridge.write(addr, value),

            // VRAM
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize] = value,

            // External RAM (MBC-controlled)
            0xA000..=0xBFFF => self.cartridge.write(addr, value),

            // WRAM
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,

            // Echo RAM — mirrors WRAM writes
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,

            // OAM
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize] = value,

            // Unused region — writes ignored per hardware
            0xFEA0..=0xFEFF => {}

            // IO Registers
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize] = value,

            // HRAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,

            // IE Register
            0xFFFF => self.ie = value,
        }
    }
}
