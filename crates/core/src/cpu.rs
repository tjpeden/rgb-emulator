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
    /// Pending EI: IME will be set at the start of the next instruction.
    pub ei_pending: bool,
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
            ei_pending: false,
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
            ei_pending: false,
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

    // -------------------------------------------------------------------------
    // 8-bit ALU helpers
    // -------------------------------------------------------------------------

    /// ADD A, operand (with optional carry-in for ADC).
    fn alu_add(&mut self, operand: u8, with_carry: bool) {
        let carry_in = if with_carry { self.carry() as u8 } else { 0 };
        let a = self.a;
        let result = a.wrapping_add(operand).wrapping_add(carry_in);
        self.set_zero(result == 0);
        self.set_subtract(false);
        self.set_half_carry((a & 0xF) + (operand & 0xF) + carry_in > 0xF);
        self.set_carry((a as u16) + (operand as u16) + (carry_in as u16) > 0xFF);
        self.a = result;
    }

    /// SUB operand (with optional borrow-in for SBC).
    fn alu_sub(&mut self, operand: u8, with_carry: bool) {
        let carry_in = if with_carry { self.carry() as u8 } else { 0 };
        let a = self.a;
        let result = a.wrapping_sub(operand).wrapping_sub(carry_in);
        self.set_zero(result == 0);
        self.set_subtract(true);
        self.set_half_carry((a & 0xF) < (operand & 0xF) + carry_in);
        self.set_carry((a as u16) < (operand as u16) + (carry_in as u16));
        self.a = result;
    }

    /// AND operand.
    fn alu_and(&mut self, operand: u8) {
        self.a &= operand;
        let z = self.a == 0;
        self.set_zero(z);
        self.set_subtract(false);
        self.set_half_carry(true);
        self.set_carry(false);
    }

    /// XOR operand.
    fn alu_xor(&mut self, operand: u8) {
        self.a ^= operand;
        let z = self.a == 0;
        self.set_zero(z);
        self.set_subtract(false);
        self.set_half_carry(false);
        self.set_carry(false);
    }

    /// OR operand.
    fn alu_or(&mut self, operand: u8) {
        self.a |= operand;
        let z = self.a == 0;
        self.set_zero(z);
        self.set_subtract(false);
        self.set_half_carry(false);
        self.set_carry(false);
    }

    /// CP operand — like SUB but result is discarded.
    fn alu_cp(&mut self, operand: u8) {
        let a = self.a;
        self.set_zero(a == operand);
        self.set_subtract(true);
        self.set_half_carry((a & 0xF) < (operand & 0xF));
        self.set_carry(a < operand);
    }

    /// INC r — C flag unaffected.
    fn alu_inc(&mut self, v: u8) -> u8 {
        let result = v.wrapping_add(1);
        self.set_zero(result == 0);
        self.set_subtract(false);
        self.set_half_carry((v & 0xF) == 0xF);
        result
    }

    /// DEC r — C flag unaffected.
    fn alu_dec(&mut self, v: u8) -> u8 {
        let result = v.wrapping_sub(1);
        self.set_zero(result == 0);
        self.set_subtract(true);
        self.set_half_carry((v & 0xF) == 0x0);
        result
    }

    /// Execute one instruction and return the number of T-cycles consumed.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        // EI delay: enable IME before executing this instruction if ei_pending.
        if self.ei_pending {
            self.ime = true;
            self.ei_pending = false;
        }

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
            0x40..=0x7F => {
                if opcode == 0x76 {
                    // HALT
                    self.halted = true;
                    return 4;
                }
                let dst = (opcode >> 3) & 0x07;
                let src = opcode & 0x07;
                let (value, extra_read) = self.read_reg(src, bus);
                let extra_write = self.write_reg(dst, value, bus);
                4 + extra_read + extra_write
            }

            // ADD A, r/m (0x80–0x87)
            0x80..=0x87 => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_add(operand, false);
                4 + extra
            }
            // ADC A, r/m (0x88–0x8F)
            0x88..=0x8F => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_add(operand, true);
                4 + extra
            }
            // SUB r/m (0x90–0x97)
            0x90..=0x97 => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_sub(operand, false);
                4 + extra
            }
            // SBC A, r/m (0x98–0x9F)
            0x98..=0x9F => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_sub(operand, true);
                4 + extra
            }
            // AND r/m (0xA0–0xA7)
            0xA0..=0xA7 => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_and(operand);
                4 + extra
            }
            // XOR r/m (0xA8–0xAF)
            0xA8..=0xAF => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_xor(operand);
                4 + extra
            }
            // OR r/m (0xB0–0xB7)
            0xB0..=0xB7 => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_or(operand);
                4 + extra
            }
            // CP r/m (0xB8–0xBF)
            0xB8..=0xBF => {
                let (operand, extra) = self.read_reg(opcode & 0x07, bus);
                self.alu_cp(operand);
                4 + extra
            }

            // ADD A, n
            0xC6 => { let n = self.fetch(bus); self.alu_add(n, false); 8 }
            // ADC A, n
            0xCE => { let n = self.fetch(bus); self.alu_add(n, true); 8 }
            // SUB n
            0xD6 => { let n = self.fetch(bus); self.alu_sub(n, false); 8 }
            // SBC A, n
            0xDE => { let n = self.fetch(bus); self.alu_sub(n, true); 8 }
            // AND n
            0xE6 => { let n = self.fetch(bus); self.alu_and(n); 8 }
            // XOR n
            0xEE => { let n = self.fetch(bus); self.alu_xor(n); 8 }
            // OR n
            0xF6 => { let n = self.fetch(bus); self.alu_or(n); 8 }
            // CP n
            0xFE => { let n = self.fetch(bus); self.alu_cp(n); 8 }

            // INC r (8-bit registers)
            0x04 => { self.b = self.alu_inc(self.b); 4 }
            0x0C => { self.c = self.alu_inc(self.c); 4 }
            0x14 => { self.d = self.alu_inc(self.d); 4 }
            0x1C => { self.e = self.alu_inc(self.e); 4 }
            0x24 => { self.h = self.alu_inc(self.h); 4 }
            0x2C => { self.l = self.alu_inc(self.l); 4 }
            0x34 => {
                let hl = self.hl();
                let v = bus.read(hl);
                let r = self.alu_inc(v);
                bus.write(hl, r);
                12
            }
            0x3C => { self.a = self.alu_inc(self.a); 4 }

            // DEC r (8-bit registers)
            0x05 => { self.b = self.alu_dec(self.b); 4 }
            0x0D => { self.c = self.alu_dec(self.c); 4 }
            0x15 => { self.d = self.alu_dec(self.d); 4 }
            0x1D => { self.e = self.alu_dec(self.e); 4 }
            0x25 => { self.h = self.alu_dec(self.h); 4 }
            0x2D => { self.l = self.alu_dec(self.l); 4 }
            0x35 => {
                let hl = self.hl();
                let v = bus.read(hl);
                let r = self.alu_dec(v);
                bus.write(hl, r);
                12
            }
            0x3D => { self.a = self.alu_dec(self.a); 4 }

            // DAA
            0x27 => {
                let mut a = self.a as u16;
                if !self.subtract() {
                    if self.half_carry() || (a & 0x0F) > 9 {
                        a = a.wrapping_add(0x06);
                    }
                    if self.carry() || a > 0x9F {
                        a = a.wrapping_add(0x60);
                        self.set_carry(true);
                    }
                } else {
                    if self.half_carry() {
                        a = a.wrapping_sub(0x06);
                    }
                    if self.carry() {
                        a = a.wrapping_sub(0x60);
                    }
                }
                self.a = a as u8;
                self.set_zero(self.a == 0);
                self.set_half_carry(false);
                4
            }

            // CPL — complement A
            0x2F => {
                self.a = !self.a;
                self.set_subtract(true);
                self.set_half_carry(true);
                4
            }

            // SCF — set carry flag
            0x37 => {
                self.set_subtract(false);
                self.set_half_carry(false);
                self.set_carry(true);
                4
            }

            // CCF — complement carry flag
            0x3F => {
                let c = self.carry();
                self.set_subtract(false);
                self.set_half_carry(false);
                self.set_carry(!c);
                4
            }

            // RLCA
            0x07 => {
                let bit7 = self.a >> 7;
                self.a = (self.a << 1) | bit7;
                self.set_zero(false);
                self.set_subtract(false);
                self.set_half_carry(false);
                self.set_carry(bit7 != 0);
                4
            }
            // RRCA
            0x0F => {
                let bit0 = self.a & 1;
                self.a = (self.a >> 1) | (bit0 << 7);
                self.set_zero(false);
                self.set_subtract(false);
                self.set_half_carry(false);
                self.set_carry(bit0 != 0);
                4
            }
            // RLA
            0x17 => {
                let old_carry = self.carry() as u8;
                let bit7 = self.a >> 7;
                self.a = (self.a << 1) | old_carry;
                self.set_zero(false);
                self.set_subtract(false);
                self.set_half_carry(false);
                self.set_carry(bit7 != 0);
                4
            }
            // RRA
            0x1F => {
                let old_carry = self.carry() as u8;
                let bit0 = self.a & 1;
                self.a = (self.a >> 1) | (old_carry << 7);
                self.set_zero(false);
                self.set_subtract(false);
                self.set_half_carry(false);
                self.set_carry(bit0 != 0);
                4
            }

            // INC rr — 16-bit increment, no flags affected
            0x03 => { let v = self.bc().wrapping_add(1); self.set_bc(v); 8 }
            0x13 => { let v = self.de().wrapping_add(1); self.set_de(v); 8 }
            0x23 => { let v = self.hl().wrapping_add(1); self.set_hl(v); 8 }
            0x33 => { self.sp = self.sp.wrapping_add(1); 8 }

            // DEC rr — 16-bit decrement, no flags affected
            0x0B => { let v = self.bc().wrapping_sub(1); self.set_bc(v); 8 }
            0x1B => { let v = self.de().wrapping_sub(1); self.set_de(v); 8 }
            0x2B => { let v = self.hl().wrapping_sub(1); self.set_hl(v); 8 }
            0x3B => { self.sp = self.sp.wrapping_sub(1); 8 }

            // ADD HL, rr — N=0, H=carry from bit 11, C=carry from bit 15; Z unaffected
            0x09 => {
                let hl = self.hl(); let rr = self.bc();
                self.set_subtract(false);
                self.set_half_carry((hl & 0x0FFF) + (rr & 0x0FFF) > 0x0FFF);
                self.set_carry((hl as u32) + (rr as u32) > 0xFFFF);
                self.set_hl(hl.wrapping_add(rr));
                8
            }
            0x19 => {
                let hl = self.hl(); let rr = self.de();
                self.set_subtract(false);
                self.set_half_carry((hl & 0x0FFF) + (rr & 0x0FFF) > 0x0FFF);
                self.set_carry((hl as u32) + (rr as u32) > 0xFFFF);
                self.set_hl(hl.wrapping_add(rr));
                8
            }
            0x29 => {
                let hl = self.hl(); let rr = hl;
                self.set_subtract(false);
                self.set_half_carry((hl & 0x0FFF) + (rr & 0x0FFF) > 0x0FFF);
                self.set_carry((hl as u32) + (rr as u32) > 0xFFFF);
                self.set_hl(hl.wrapping_add(rr));
                8
            }
            0x39 => {
                let hl = self.hl(); let rr = self.sp;
                self.set_subtract(false);
                self.set_half_carry((hl & 0x0FFF) + (rr & 0x0FFF) > 0x0FFF);
                self.set_carry((hl as u32) + (rr as u32) > 0xFFFF);
                self.set_hl(hl.wrapping_add(rr));
                8
            }

            // ADD SP, e — Z=0, N=0, H=carry from bit 3, C=carry from bit 7
            0xE8 => {
                let e = self.fetch(bus) as i8;
                let sp_lo = self.sp as u8;
                let e_u8 = e as u8;
                self.set_zero(false);
                self.set_subtract(false);
                self.set_half_carry((sp_lo & 0x0F) + (e_u8 & 0x0F) > 0x0F);
                self.set_carry((sp_lo as u16) + (e_u8 as u16) > 0xFF);
                self.sp = self.sp.wrapping_add(e as i16 as u16);
                16
            }

            // -------------------------------------------------------------------------
            // Control flow
            // -------------------------------------------------------------------------

            // NOP
            0x00 => 4,

            // STOP — stub, treat as NOP
            0x10 => { let _ = self.fetch(bus); 4 }

            // JP nn — unconditional absolute jump
            0xC3 => { let nn = self.fetch_u16(bus); self.pc = nn; 16 }

            // JP HL — PC = HL, no memory access
            0xE9 => { self.pc = self.hl(); 4 }

            // JP cc, nn — conditional absolute jump
            0xC2 => { let nn = self.fetch_u16(bus); if !self.zero()  { self.pc = nn; 16 } else { 12 } }
            0xCA => { let nn = self.fetch_u16(bus); if  self.zero()  { self.pc = nn; 16 } else { 12 } }
            0xD2 => { let nn = self.fetch_u16(bus); if !self.carry() { self.pc = nn; 16 } else { 12 } }
            0xDA => { let nn = self.fetch_u16(bus); if  self.carry() { self.pc = nn; 16 } else { 12 } }

            // JR e — unconditional relative jump
            0x18 => {
                let e = self.fetch(bus) as i8;
                self.pc = self.pc.wrapping_add(e as i16 as u16);
                12
            }

            // JR cc, e — conditional relative jump
            0x20 => {
                let e = self.fetch(bus) as i8;
                if !self.zero()  { self.pc = self.pc.wrapping_add(e as i16 as u16); 12 } else { 8 }
            }
            0x28 => {
                let e = self.fetch(bus) as i8;
                if  self.zero()  { self.pc = self.pc.wrapping_add(e as i16 as u16); 12 } else { 8 }
            }
            0x30 => {
                let e = self.fetch(bus) as i8;
                if !self.carry() { self.pc = self.pc.wrapping_add(e as i16 as u16); 12 } else { 8 }
            }
            0x38 => {
                let e = self.fetch(bus) as i8;
                if  self.carry() { self.pc = self.pc.wrapping_add(e as i16 as u16); 12 } else { 8 }
            }

            // CALL nn — unconditional call
            0xCD => {
                let nn = self.fetch_u16(bus);
                let pc = self.pc;
                self.push_u16(bus, pc);
                self.pc = nn;
                24
            }

            // CALL cc, nn — conditional call
            0xC4 => {
                let nn = self.fetch_u16(bus);
                if !self.zero()  { let pc = self.pc; self.push_u16(bus, pc); self.pc = nn; 24 } else { 12 }
            }
            0xCC => {
                let nn = self.fetch_u16(bus);
                if  self.zero()  { let pc = self.pc; self.push_u16(bus, pc); self.pc = nn; 24 } else { 12 }
            }
            0xD4 => {
                let nn = self.fetch_u16(bus);
                if !self.carry() { let pc = self.pc; self.push_u16(bus, pc); self.pc = nn; 24 } else { 12 }
            }
            0xDC => {
                let nn = self.fetch_u16(bus);
                if  self.carry() { let pc = self.pc; self.push_u16(bus, pc); self.pc = nn; 24 } else { 12 }
            }

            // RET — unconditional return
            0xC9 => { self.pc = self.pop_u16(bus); 16 }

            // RETI — return and enable interrupts immediately (no delay)
            0xD9 => { self.pc = self.pop_u16(bus); self.ime = true; 16 }

            // RET cc — conditional return
            0xC0 => { if !self.zero()  { self.pc = self.pop_u16(bus); 20 } else { 8 } }
            0xC8 => { if  self.zero()  { self.pc = self.pop_u16(bus); 20 } else { 8 } }
            0xD0 => { if !self.carry() { self.pc = self.pop_u16(bus); 20 } else { 8 } }
            0xD8 => { if  self.carry() { self.pc = self.pop_u16(bus); 20 } else { 8 } }

            // RST n — restart vectors
            0xC7 => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0000; 16 }
            0xCF => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0008; 16 }
            0xD7 => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0010; 16 }
            0xDF => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0018; 16 }
            0xE7 => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0020; 16 }
            0xEF => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0028; 16 }
            0xF7 => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0030; 16 }
            0xFF => { let pc = self.pc; self.push_u16(bus, pc); self.pc = 0x0038; 16 }

            // DI — disable interrupts
            0xF3 => { self.ime = false; self.ei_pending = false; 4 }

            // EI — enable interrupts after the next instruction (1-instruction delay)
            0xFB => { self.ei_pending = true; 4 }

            _ => panic!("Unimplemented opcode: {:#04X}", opcode),
        }
    }
}

impl Default for CPU {
    fn default() -> Self {
        Self::new()
    }
}
