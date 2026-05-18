use crate::{Bus, CPU};
use crate::bus::JoypadState;
use crate::ppu::PPU;

/// Snapshot of emulator state for the F1 debug overlay.
#[derive(Debug, Clone)]
pub struct DebugInfo {
    // CPU registers
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub f: u8,
    pub sp: u16,
    pub pc: u16,
    // Flags (derived from F)
    pub flag_z: bool,
    pub flag_n: bool,
    pub flag_h: bool,
    pub flag_c: bool,
    // IME
    pub ime: bool,
    // PPU
    pub ppu_mode: u8,
    pub ly: u8,
    // Cycle count
    pub total_cycles: u64,
    // ROM title
    pub rom_title: String,
}

/// Total T-cycles per DMG frame (70224 = 154 lines × 456 T-cycles/line).
#[allow(dead_code)]
const FRAME_T_CYCLES: u32 = 70224;

/// Returned by [`GameBoy::step`] to indicate what happened during this step.
#[derive(Debug, PartialEq, Eq)]
pub enum StepResult {
    /// Normal step — no frame boundary was crossed.
    Continue,
    /// A full 70224 T-cycle frame has elapsed; the framebuffer is ready.
    FrameComplete,
}

/// Errors that can be returned from [`GameBoy::step`].
#[derive(Debug)]
pub enum EmulationError {
    // Reserved for future use (invalid opcode, bus fault, etc.).
    #[allow(dead_code)]
    InvalidOpcode(u8),
}

impl std::fmt::Display for EmulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmulationError::InvalidOpcode(op) => write!(f, "Invalid opcode: {:#04X}", op),
        }
    }
}

impl std::error::Error for EmulationError {}

/// Top-level emulator struct. Owns all hardware components.
///
/// # Example
/// ```no_run
/// # use rgb_core::{GameBoy, JoypadState, StepResult};
/// # let rom = vec![0u8; 0x8000];
/// let mut gb = GameBoy::new(rom, None);
/// let joypad = JoypadState::default();
/// loop {
///     if gb.step(&joypad).unwrap() == StepResult::FrameComplete {
///         let _fb = gb.framebuffer(); // 160×144 RGBA
///         break;
///     }
/// }
/// ```
pub struct GameBoy {
    cpu: CPU,
    ppu: PPU,
    bus: Bus,
    /// Serial output bytes collected from blargg-style test ROMs.
    pub serial_output: Vec<u8>,
    /// T-cycle counter within the current frame.
    cycles: u32,
    /// Total T-cycles elapsed since boot.
    total_cycles: u64,
}

impl GameBoy {
    /// Create a new `GameBoy` from ROM bytes, with an optional boot ROM.
    ///
    /// - If `boot_rom` is `None`, registers are initialised to the post-boot
    ///   DMG state and execution begins at `0x0100`.
    /// - If `boot_rom` is `Some`, a cold-start CPU is used and execution
    ///   begins at `0x0000`.
    pub fn new(rom: Vec<u8>, boot_rom: Option<Vec<u8>>) -> Self {
        let has_boot_rom = boot_rom.is_some();
        let bus = Bus::new(rom, boot_rom);
        let cpu = if has_boot_rom {
            CPU::new()
        } else {
            CPU::new_post_boot()
        };

        Self {
            cpu,
            ppu: PPU::new(),
            bus,
            serial_output: Vec::new(),
            cycles: 0,
            total_cycles: 0,
        }
    }

    /// Advance emulation by one machine cycle (4 T-cycles).
    ///
    /// Returns `Ok(StepResult::FrameComplete)` when a full 70224 T-cycle frame
    /// has elapsed; otherwise returns `Ok(StepResult::Continue)`.
    ///
    /// # Errors
    ///
    /// Returns `Err(EmulationError)` on unrecoverable emulation faults (e.g.
    /// invalid opcode). In Phase 1 this never fires.
    pub fn step(&mut self, input: &JoypadState) -> Result<StepResult, EmulationError> {
        // Update joypad state. If any button was newly pressed, request the
        // joypad interrupt (IF bit 4, vector 0x0060).
        if self.bus.update_joypad(*input) {
            let if_val = self.bus.read(0xFF0F);
            self.bus.write(0xFF0F, if_val | 0x10);
        }

        // Handle HALT: if halted, consume 4T while waiting for an interrupt.
        let t_cycles = if self.cpu.halted {
            4 // consume 4T while halted
        } else {
            self.cpu.step(&mut self.bus)
        };

        // Step the timer and request interrupt if it fired.
        if self.bus.timer.step(t_cycles) {
            let if_val = self.bus.read(0xFF0F);
            self.bus.write(0xFF0F, if_val | 0x04);
        }

        // Step the APU frame sequencer, driven by the same internal counter.
        let div = self.bus.timer.div_counter();
        self.bus.apu.step(div);

        // Step the PPU. If VBlank is entered, request interrupt (IF bit 0).
        let vblank = self.ppu.step(&mut self.bus, t_cycles);
        if vblank {
            let if_val = self.bus.read(0xFF0F);
            self.bus.write(0xFF0F, if_val | 0x01);
        }

        // Check and dispatch interrupts (also wakes CPU from HALT).
        if let Some(irq_cycles) = self.cpu.check_interrupts(&mut self.bus) {
            self.cycles += irq_cycles;
        }

        self.cycles += t_cycles;
        self.total_cycles += t_cycles as u64;

        // Drain any serial bytes produced by the bus stub into the public
        // serial_output buffer so the desktop crate can print them to stdout.
        if !self.bus.serial_output.is_empty() {
            self.serial_output.append(&mut self.bus.serial_output);
        }

        // VBlank entry (PPU entering Mode 1) marks the frame boundary.
        if vblank {
            Ok(StepResult::FrameComplete)
        } else {
            Ok(StepResult::Continue)
        }
    }

    /// Return a snapshot of current emulator state for the debug overlay (F1).
    pub fn debug_info(&self) -> DebugInfo {
        // Extract ROM title from cartridge header bytes 0x0134–0x0143.
        let title_bytes: Vec<u8> = (0x0134u16..=0x0143)
            .map(|addr| self.bus.read(addr))
            .take_while(|&b| b != 0)
            .collect();
        let rom_title = String::from_utf8_lossy(&title_bytes).into_owned();

        DebugInfo {
            a: self.cpu.a,
            b: self.cpu.b,
            c: self.cpu.c,
            d: self.cpu.d,
            e: self.cpu.e,
            h: self.cpu.h,
            l: self.cpu.l,
            f: self.cpu.f,
            sp: self.cpu.sp,
            pc: self.cpu.pc,
            flag_z: self.cpu.f & 0x80 != 0,
            flag_n: self.cpu.f & 0x40 != 0,
            flag_h: self.cpu.f & 0x20 != 0,
            flag_c: self.cpu.f & 0x10 != 0,
            ime: self.cpu.ime,
            ppu_mode: self.ppu.mode(),
            ly: self.ppu.ly(),
            total_cycles: self.total_cycles,
            rom_title,
        }
    }

    /// Return a reference to the 160×144 RGBA framebuffer.
    ///
    /// Each pixel is four consecutive bytes: red, green, blue, alpha.
    /// Valid to read after any [`StepResult::FrameComplete`] is returned.
    pub fn framebuffer(&self) -> &[u8] {
        self.ppu.framebuffer()
    }

    /// Returns the cartridge's battery-backed RAM, or `None` if the cartridge
    /// has no battery. Use this to persist saves to disk on exit.
    pub fn battery_ram(&self) -> Option<Vec<u8>> {
        self.bus.battery_ram().map(|s| s.to_vec())
    }

    /// Restores the cartridge's battery-backed RAM from previously-saved data.
    /// Call this after construction but before running any steps.
    pub fn load_battery_ram(&mut self, data: &[u8]) {
        self.bus.load_battery_ram(data);
    }
}
