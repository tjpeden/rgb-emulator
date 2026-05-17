/// Sharp SM83 CPU registers.
///
/// The SM83 has eight 8-bit registers (`A`, `F`, `B`, `C`, `D`, `E`, `H`, `L`)
/// that can be paired as 16-bit registers (`AF`, `BC`, `DE`, `HL`), plus the
/// 16-bit `SP` (stack pointer) and `PC` (program counter).
///
/// Flag register (`F`) layout:
/// - Bit 7: Z (Zero)
/// - Bit 6: N (Subtract)
/// - Bit 5: H (Half-carry)
/// - Bit 4: C (Carry)
/// - Bits 3–0: always 0
pub struct CPU {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    /// Master interrupt enable flag. Controlled by `EI`/`DI`/`RETI`.
    pub ime: bool,
    /// Set when `HALT` is executed; cleared on interrupt.
    pub halted: bool,
}

impl CPU {
    /// Create a CPU in the post-boot state (no boot ROM path).
    ///
    /// Matches the documented DMG register values after the boot ROM finishes
    /// (Pan Docs — "Power Up Sequence"):
    ///
    /// | Register | Value   |
    /// |----------|---------|
    /// | AF       | `0x01B0`|
    /// | BC       | `0x0013`|
    /// | DE       | `0x00D8`|
    /// | HL       | `0x014D`|
    /// | SP       | `0xFFFE`|
    /// | PC       | `0x0100`|
    ///
    /// Flags: Z=1, N=0, H=1, C=1 (F = 0xB0)
    pub fn new_post_boot() -> Self {
        Self {
            a: 0x01,
            f: 0xB0,
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100,
            ime: false,
            halted: false,
        }
    }

    /// Create a CPU in the cold-start state (boot ROM present).
    ///
    /// All registers are zero-initialised; the boot ROM running from `0x0000`
    /// will set them to their correct values before handing off to the game.
    pub fn new() -> Self {
        Self {
            a: 0x00,
            f: 0x00,
            b: 0x00,
            c: 0x00,
            d: 0x00,
            e: 0x00,
            h: 0x00,
            l: 0x00,
            sp: 0x0000,
            pc: 0x0000,
            ime: false,
            halted: false,
        }
    }

    // -------------------------------------------------------------------------
    // 16-bit register pair helpers
    // -------------------------------------------------------------------------

    pub fn af(&self) -> u16 {
        (self.a as u16) << 8 | (self.f as u16)
    }

    pub fn set_af(&mut self, value: u16) {
        self.a = (value >> 8) as u8;
        // Low nibble of F is always 0
        self.f = (value as u8) & 0xF0;
    }

    pub fn bc(&self) -> u16 {
        (self.b as u16) << 8 | (self.c as u16)
    }

    pub fn set_bc(&mut self, value: u16) {
        self.b = (value >> 8) as u8;
        self.c = value as u8;
    }

    pub fn de(&self) -> u16 {
        (self.d as u16) << 8 | (self.e as u16)
    }

    pub fn set_de(&mut self, value: u16) {
        self.d = (value >> 8) as u8;
        self.e = value as u8;
    }

    pub fn hl(&self) -> u16 {
        (self.h as u16) << 8 | (self.l as u16)
    }

    pub fn set_hl(&mut self, value: u16) {
        self.h = (value >> 8) as u8;
        self.l = value as u8;
    }

    // -------------------------------------------------------------------------
    // Flag helpers
    // -------------------------------------------------------------------------

    /// Zero flag (bit 7).
    pub fn zero(&self) -> bool {
        self.f & 0x80 != 0
    }

    /// Subtract flag (bit 6).
    pub fn subtract(&self) -> bool {
        self.f & 0x40 != 0
    }

    /// Half-carry flag (bit 5).
    pub fn half_carry(&self) -> bool {
        self.f & 0x20 != 0
    }

    /// Carry flag (bit 4).
    pub fn carry(&self) -> bool {
        self.f & 0x10 != 0
    }

    pub fn set_zero(&mut self, v: bool) {
        if v { self.f |= 0x80; } else { self.f &= !0x80; }
    }

    pub fn set_subtract(&mut self, v: bool) {
        if v { self.f |= 0x40; } else { self.f &= !0x40; }
    }

    pub fn set_half_carry(&mut self, v: bool) {
        if v { self.f |= 0x20; } else { self.f &= !0x20; }
    }

    pub fn set_carry(&mut self, v: bool) {
        if v { self.f |= 0x10; } else { self.f &= !0x10; }
    }
}

impl Default for CPU {
    fn default() -> Self {
        Self::new()
    }
}
