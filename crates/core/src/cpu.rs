use crate::Bus;

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

    // -------------------------------------------------------------------------
    // Instruction execution
    // -------------------------------------------------------------------------

    /// Push a 16-bit value onto the stack.
    fn push_u16(&mut self, bus: &mut Bus, value: u16) {
        self.sp = self.sp.wrapping_sub(2);
        bus.write(self.sp.wrapping_add(1), (value >> 8) as u8);
        bus.write(self.sp, value as u8);
    }

    /// Pop a 16-bit value from the stack.
    fn pop_u16(&mut self, bus: &mut Bus) -> u16 {
        let lo = bus.read(self.sp) as u16;
        let hi = bus.read(self.sp.wrapping_add(1)) as u16;
        self.sp = self.sp.wrapping_add(2);
        hi << 8 | lo
    }

    /// Fetch one byte from [PC] and advance PC.
    fn fetch(&mut self, bus: &mut Bus) -> u8 {
        let b = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        b
    }

    /// Fetch a little-endian 16-bit word from [PC] and advance PC by 2.
    fn fetch_u16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        (hi << 8) | lo
    }

    /// Read an 8-bit register by SM83 encoding (0=B,1=C,2=D,3=E,4=H,5=L,6=(HL),7=A).
    ///
    /// Returns the value and the additional T-cycles cost for (HL) reads (4 extra).
    fn read_reg(&self, idx: u8, bus: &mut Bus) -> (u8, u32) {
        match idx {
            0 => (self.b, 0),
            1 => (self.c, 0),
            2 => (self.d, 0),
            3 => (self.e, 0),
            4 => (self.h, 0),
            5 => (self.l, 0),
            6 => (bus.read(self.hl()), 4),
            7 => (self.a, 0),
            _ => unreachable!(),
        }
    }

    /// Write an 8-bit register by SM83 encoding. Returns extra T-cycles for (HL) writes.
    fn write_reg(&mut self, idx: u8, value: u8, bus: &mut Bus) -> u32 {
        match idx {
            0 => { self.b = value; 0 }
            1 => { self.c = value; 0 }
            2 => { self.d = value; 0 }
            3 => { self.e = value; 0 }
            4 => { self.h = value; 0 }
            5 => { self.l = value; 0 }
            6 => { bus.write(self.hl(), value); 4 }
            7 => { self.a = value; 0 }
            _ => unreachable!(),
        }
    }

    /// Execute one instruction and return the number of T-cycles consumed.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        let opcode = self.fetch(bus);

        match opcode {
            // LD r, n — load immediate byte into register
            0x06 => { let n = self.fetch(bus); self.b = n; 8 }
            0x0E => { let n = self.fetch(bus); self.c = n; 8 }
            0x16 => { let n = self.fetch(bus); self.d = n; 8 }
            0x1E => { let n = self.fetch(bus); self.e = n; 8 }
            0x26 => { let n = self.fetch(bus); self.h = n; 8 }
            0x2E => { let n = self.fetch(bus); self.l = n; 8 }
            0x36 => { let n = self.fetch(bus); bus.write(self.hl(), n); 12 }
            0x3E => { let n = self.fetch(bus); self.a = n; 8 }

            // LD A, (BC) / LD A, (DE)
            0x0A => { self.a = bus.read(self.bc()); 8 }
            0x1A => { self.a = bus.read(self.de()); 8 }

            // LD (BC), A / LD (DE), A
            0x02 => { bus.write(self.bc(), self.a); 8 }
            0x12 => { bus.write(self.de(), self.a); 8 }

            // LD A, (HL+) / LD A, (HL-)
            0x2A => {
                let hl = self.hl();
                self.a = bus.read(hl);
                self.set_hl(hl.wrapping_add(1));
                8
            }
            0x3A => {
                let hl = self.hl();
                self.a = bus.read(hl);
                self.set_hl(hl.wrapping_sub(1));
                8
            }

            // LD (HL+), A / LD (HL-), A
            0x22 => {
                let hl = self.hl();
                bus.write(hl, self.a);
                self.set_hl(hl.wrapping_add(1));
                8
            }
            0x32 => {
                let hl = self.hl();
                bus.write(hl, self.a);
                self.set_hl(hl.wrapping_sub(1));
                8
            }

            // LD A, (nn) / LD (nn), A
            0xFA => { let nn = self.fetch_u16(bus); self.a = bus.read(nn); 16 }
            0xEA => { let nn = self.fetch_u16(bus); bus.write(nn, self.a); 16 }

            // LDH (n), A / LDH A, (n)
            0xE0 => { let n = self.fetch(bus); bus.write(0xFF00 | n as u16, self.a); 12 }
            0xF0 => { let n = self.fetch(bus); self.a = bus.read(0xFF00 | n as u16); 12 }

            // LD (C), A / LD A, (C)
            0xE2 => { bus.write(0xFF00 | self.c as u16, self.a); 8 }
            0xF2 => { self.a = bus.read(0xFF00 | self.c as u16); 8 }

            // LD rr, nn — load immediate 16-bit
            0x01 => { let nn = self.fetch_u16(bus); self.set_bc(nn); 12 }
            0x11 => { let nn = self.fetch_u16(bus); self.set_de(nn); 12 }
            0x21 => { let nn = self.fetch_u16(bus); self.set_hl(nn); 12 }
            0x31 => { let nn = self.fetch_u16(bus); self.sp = nn; 12 }

            // LD (nn), SP
            0x08 => {
                let nn = self.fetch_u16(bus);
                bus.write(nn, self.sp as u8);
                bus.write(nn.wrapping_add(1), (self.sp >> 8) as u8);
                20
            }

            // LD SP, HL
            0xF9 => { self.sp = self.hl(); 8 }

            // LD HL, SP+e (LDHL SP, e)
            0xF8 => {
                let e = self.fetch(bus) as i8 as i16;
                let sp = self.sp as i16;
                let result = sp.wrapping_add(e);
                let sp_u = self.sp;
                let e_u = e as u16;
                self.set_zero(false);
                self.set_subtract(false);
                self.set_half_carry((sp_u ^ e_u ^ (result as u16)) & 0x10 != 0);
                self.set_carry((sp_u ^ e_u ^ (result as u16)) & 0x100 != 0);
                self.set_hl(result as u16);
                12
            }

            // PUSH rr
            0xC5 => { let v = self.bc(); self.push_u16(bus, v); 16 }
            0xD5 => { let v = self.de(); self.push_u16(bus, v); 16 }
            0xE5 => { let v = self.hl(); self.push_u16(bus, v); 16 }
            0xF5 => { let v = self.af(); self.push_u16(bus, v); 16 }

            // POP rr
            0xC1 => { let v = self.pop_u16(bus); self.set_bc(v); 12 }
            0xD1 => { let v = self.pop_u16(bus); self.set_de(v); 12 }
            0xE1 => { let v = self.pop_u16(bus); self.set_hl(v); 12 }
            0xF1 => {
                let v = self.pop_u16(bus);
                self.a = (v >> 8) as u8;
                self.f = (v as u8) & 0xF0;
                12
            }

            // LD r, r' block (0x40–0x7F), excluding HALT (0x76)
            0x40..=0x7F => {                if opcode == 0x76 {
                    // HALT — not yet implemented
                    panic!("Unimplemented opcode: {:#04X}", opcode);
                }
                let dst = (opcode >> 3) & 0x07;
                let src = opcode & 0x07;
                let (value, extra_read) = self.read_reg(src, bus);
                let extra_write = self.write_reg(dst, value, bus);
                4 + extra_read + extra_write
            }

            _ => panic!("Unimplemented opcode: {:#04X}", opcode),
        }
    }
}

impl Default for CPU {
    fn default() -> Self {
        Self::new()
    }
}
