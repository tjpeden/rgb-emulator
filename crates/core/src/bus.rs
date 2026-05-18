use crate::apu::APU;
use crate::mbc::{self, MBC};
use crate::timer::Timer;

/// Joypad input state passed into [`crate::GameBoy::step`] each call.
///
/// All fields are `true` when the corresponding button is pressed.
#[derive(Default, Clone, Copy)]
pub struct JoypadState {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub a: bool,
    pub b: bool,
    pub start: bool,
    pub select: bool,
}

/// DMG memory bus.
///
/// Owns all addressable memory regions and dispatches reads/writes via
/// `match` on `u16` address ranges, matching the hardware memory map.
///
/// # Memory Map
///
/// | Range           | Region           | Notes                              |
/// |-----------------|------------------|------------------------------------|
/// | `0x0000–0x00FF` | Boot ROM         | Shadowed until `0xFF50` write      |
/// | `0x0000–0x3FFF` | ROM Bank 0       | Fixed                              |
/// | `0x4000–0x7FFF` | ROM Bank N       | Switchable via MBC                 |
/// | `0x8000–0x9FFF` | VRAM             | 8 KiB                              |
/// | `0xA000–0xBFFF` | External RAM     | MBC-controlled                     |
/// | `0xC000–0xDFFF` | WRAM             | 8 KiB                              |
/// | `0xE000–0xFDFF` | Echo RAM         | Mirrors `0xC000–0xDDFF`            |
/// | `0xFE00–0xFE9F` | OAM              | 40 sprites × 4 bytes               |
/// | `0xFEA0–0xFEFF` | Unused           | Reads return `0xFF`                |
/// | `0xFF00–0xFF7F` | IO Registers     | 128 bytes                          |
/// | `0xFF80–0xFFFE` | HRAM             | 127 bytes                          |
/// | `0xFFFF`        | IE Register      | Interrupt Enable                   |
pub struct Bus {
    cartridge: Box<dyn MBC>,
    /// Optional boot ROM image (256 bytes). `Some` while the boot ROM is
    /// mapped over `0x0000–0x00FF`. Writing any non-zero value to `0xFF50`
    /// (BOOT register) sets this to `None`, permanently unmapping it.
    boot_rom: Option<Box<[u8; 256]>>,
    vram: [u8; 0x2000], // 8 KiB — 0x8000–0x9FFF
    wram: [u8; 0x2000], // 8 KiB — 0xC000–0xDFFF
    oam: [u8; 0xA0],    // 160 B — 0xFE00–0xFE9F
    io: [u8; 0x80],     // 128 B — 0xFF00–0xFF7F
    hram: [u8; 0x7F],   // 127 B — 0xFF80–0xFFFE
    pub ie: u8,         // 0xFFFF — Interrupt Enable
    pub timer: Timer,
    pub apu: APU,
    /// Serial bytes produced by the stub transfer handler. Drained by
    /// `GameBoy` into its own `serial_output` buffer after each step.
    pub serial_output: Vec<u8>,
    /// Current joypad state; updated every `GameBoy::step` call.
    joypad: JoypadState,
}

impl Bus {
    /// Create a new Bus from a ROM image with an optional boot ROM.
    ///
    /// - If `boot_rom` is `Some`, it is mapped over `0x0000–0x00FF`. IO
    ///   registers start zeroed; the boot ROM initialises them itself.
    /// - If `boot_rom` is `None`, IO registers are initialised to the
    ///   documented DMG post-boot state (Pan Docs "Power Up Sequence").
    ///
    /// The MBC type is detected from cartridge header byte `0x0147`.
    pub fn new(rom: Vec<u8>, boot_rom: Option<Vec<u8>>) -> Self {
        let boot_rom = boot_rom.map(|data| {
            assert!(
                data.len() >= 256,
                "Boot ROM is too short: expected 256 bytes, got {}",
                data.len()
            );
            let mut buf = Box::new([0u8; 256]);
            buf.copy_from_slice(&data[..256]);
            buf
        });

        let io = if boot_rom.is_some() {
            // Boot ROM will set up IO registers itself.
            [0u8; 0x80]
        } else {
            Self::post_boot_io()
        };

        Self {
            cartridge: mbc::from_rom(rom),
            boot_rom,
            vram: [0u8; 0x2000],
            wram: [0u8; 0x2000],
            oam: [0u8; 0xA0],
            io,
            hram: [0u8; 0x7F],
            ie: 0x00,
            serial_output: Vec::new(),
            timer: Timer::new(),
            apu: APU::new(),
            joypad: JoypadState::default(),
        }
    }

    /// Build the IO register array pre-loaded with the post-boot hardware
    /// state (Pan Docs — "Power Up Sequence").
    ///
    /// Only registers with non-zero defaults are listed; the rest are `0x00`.
    fn post_boot_io() -> [u8; 0x80] {
        let mut io = [0u8; 0x80];
        // Joypad
        io[0x00] = 0xCF; // P1/JOYP — no group selected, no buttons pressed
        // APU
        io[0x10] = 0x80; // NR10 — CH1 sweep (bit 7 set, read-only on hardware)
        // PPU
        io[0x40] = 0x91; // LCDC — LCD on, BG on, BG tile data 0x8000, BG map 0x9800
        io[0x47] = 0xFC; // BGP  — palette: 3,3,3,0 (black, black, black, white)
        io[0x48] = 0xFF; // OBP0 — object palette 0: all black
        io[0x49] = 0xFF; // OBP1 — object palette 1: all black
        io
    }

    /// Returns a slice of the 6144 VRAM tile bytes (`0x8000–0x97FF`).
    pub fn vram_tiles(&self) -> &[u8] {
        &self.vram[..0x1800] // 384 tiles × 16 bytes = 6144 bytes
    }

    /// Returns the cartridge's battery-backed RAM contents, or `None` if the
    /// cartridge has no battery.
    pub fn battery_ram(&self) -> Option<&[u8]> {
        self.cartridge.battery_ram()
    }

    /// Restores the cartridge's battery-backed RAM from previously-saved data.
    pub fn load_battery_ram(&mut self, data: &[u8]) {
        self.cartridge.load_battery_ram(data);
    }

    /// Update the stored joypad state and report whether a joypad interrupt
    /// should be requested.
    ///
    /// Returns `true` if any button transitioned from unpressed to pressed
    /// this call (hardware fires IF bit 4 on such a low-going edge).
    pub fn update_joypad(&mut self, new_state: JoypadState) -> bool {
        let prev = self.joypad;
        self.joypad = new_state;
        // Any button newly pressed? (false → true transition)
        (!prev.a      && new_state.a)      ||
        (!prev.b      && new_state.b)      ||
        (!prev.start  && new_state.start)  ||
        (!prev.select && new_state.select) ||
        (!prev.up     && new_state.up)     ||
        (!prev.down   && new_state.down)   ||
        (!prev.left   && new_state.left)   ||
        (!prev.right  && new_state.right)
    }

    /// Returns `true` if the boot ROM is currently mapped over `0x0000–0x00FF`.
    pub fn boot_rom_active(&self) -> bool {
        self.boot_rom.is_some()
    }

    /// Read a byte from the given address.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            // Boot ROM shadows the cartridge over 0x0000–0x00FF while mapped.
            0x0000..=0x00FF => match &self.boot_rom {
                Some(boot) => boot[addr as usize],
                None => self.cartridge.read(addr),
            },

            // ROM Bank 0 (above boot ROM region) + ROM Bank N
            0x0100..=0x7FFF => self.cartridge.read(addr),

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
            // P1/JOYP (0xFF00):
            //   Bit 5 = 0: select button group (A, B, Select, Start)
            //   Bit 4 = 0: select d-pad group  (Right, Left, Up, Down)
            //   Bits 3–0: active-low state (0 = pressed) of selected group(s)
            //   Bits 6–7: unused open-drain lines, always read as 1.
            // Both groups may be selected simultaneously; outputs are ORed
            // (open-drain), so pressing any button in either group pulls low.
            0xFF00 => {
                let selector = self.io[0x00];
                let mut lo = 0x0F_u8; // default: all high (nothing pressed)

                if selector & 0x20 == 0 {
                    // Button group selected (bit 5 low)
                    // Bit layout: Start=3, Select=2, B=1, A=0
                    if self.joypad.a      { lo &= !0x01; }
                    if self.joypad.b      { lo &= !0x02; }
                    if self.joypad.select { lo &= !0x04; }
                    if self.joypad.start  { lo &= !0x08; }
                }
                if selector & 0x10 == 0 {
                    // D-pad group selected (bit 4 low)
                    // Bit layout: Down=3, Up=2, Left=1, Right=0
                    if self.joypad.right { lo &= !0x01; }
                    if self.joypad.left  { lo &= !0x02; }
                    if self.joypad.up    { lo &= !0x04; }
                    if self.joypad.down  { lo &= !0x08; }
                }

                // Bits 7–6 always 1; bits 5–4 echo selector; bits 3–0 active-low
                0xC0 | (selector & 0x30) | lo
            }
            0xFF01..=0xFF03 => self.io[(addr - 0xFF00) as usize],
            0xFF04..=0xFF07 => self.timer.read(addr),
            // APU registers: channel control (0xFF10–0xFF26) and wave RAM (0xFF30–0xFF3F)
            0xFF10..=0xFF26 | 0xFF30..=0xFF3F => self.apu.read(addr),
            0xFF08..=0xFF7F => self.io[(addr - 0xFF00) as usize],

            // HRAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],

            // IE Register
            0xFFFF => self.ie,
        }
    }

    /// Write the STAT register on behalf of the PPU.
    ///
    /// Unlike CPU writes (which are masked to bits 3–6), the PPU may update
    /// all bits of STAT including the read-only status fields (bits 0–2).
    /// Called exclusively from `PPU::update_stat`.
    pub fn write_stat_ppu(&mut self, value: u8) {
        self.io[0x41] = value;
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

            // IO Registers — special handling for timer, serial and BOOT registers
            0xFF04..=0xFF07 => self.timer.write(addr, value),
            // APU registers: channel control (0xFF10–0xFF26) and wave RAM (0xFF30–0xFF3F)
            0xFF10..=0xFF26 | 0xFF30..=0xFF3F => self.apu.write(addr, value),
            // STAT (0xFF41) — bits 0–2 are read-only (set by PPU); only bits
            // 3–6 (interrupt-enable flags) are writable by the CPU.
            0xFF41 => {
                let current = self.io[0x41];
                self.io[0x41] = (current & 0x07) | (value & 0x78);
            }
            // DMA (0xFF46) — OAM DMA transfer.
            // Writing XX triggers an immediate copy of 160 bytes from
            // XX00–XX9F into OAM (0xFE00–0xFE9F).
            // TODO: cycle-accurate DMA
            0xFF46 => {
                self.io[0x46] = value;
                let src_base = (value as u16) << 8;
                for i in 0..0xA0u16 {
                    let byte = self.read(src_base + i);
                    self.oam[i as usize] = byte;
                }
            }
            // SC (0xFF02) — Serial Transfer Control: when written with 0x81
            // (transfer-start bit + internal-clock bit), capture SB and queue
            // the byte for GameBoy::serial_output.
            0xFF02 => {
                self.io[0x02] = value;
                if value == 0x81 {
                    let sb = self.io[0x01];
                    self.serial_output.push(sb);
                    // Clear the transfer-start bit to signal completion.
                    self.io[0x02] = value & !0x80;
                }
            }
            0xFF50 => {
                // Any non-zero write permanently unmaps the boot ROM.
                if value != 0 {
                    self.boot_rom = None;
                }
                self.io[0x50] = value;
            }
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize] = value,

            // HRAM
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,

            // IE Register
            0xFFFF => self.ie = value,
        }
    }
}
