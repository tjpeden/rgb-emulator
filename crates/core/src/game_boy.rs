use crate::{Bus, CPU};

/// Screen width in pixels.
pub const SCREEN_WIDTH: u32 = 160;
/// Screen height in pixels.
pub const SCREEN_HEIGHT: u32 = 144;

/// Total T-cycles per DMG frame (70224 = 154 lines × 456 T-cycles/line).
const FRAME_T_CYCLES: u32 = 70224;

/// Joypad input state passed into [`GameBoy::step`] each call.
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
    bus: Bus,
    /// Serial output bytes collected from blargg-style test ROMs.
    pub serial_output: Vec<u8>,
    /// T-cycle counter within the current frame.
    cycles: u32,
    /// 160×144 RGBA framebuffer. Updated once per frame (stub: solid black).
    framebuffer: Box<[u8; (SCREEN_WIDTH * SCREEN_HEIGHT * 4) as usize]>,
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

        // Initialise framebuffer to opaque black (R=0, G=0, B=0, A=255).
        let mut fb = Box::new([0u8; (SCREEN_WIDTH * SCREEN_HEIGHT * 4) as usize]);
        for chunk in fb.chunks_exact_mut(4) {
            chunk[3] = 0xFF; // alpha
        }

        Self {
            cpu,
            bus,
            serial_output: Vec::new(),
            cycles: 0,
            framebuffer: fb,
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
    pub fn step(&mut self, _input: &JoypadState) -> Result<StepResult, EmulationError> {
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

        // Check and dispatch interrupts (also wakes CPU from HALT).
        if let Some(irq_cycles) = self.cpu.check_interrupts(&mut self.bus) {
            self.cycles += irq_cycles;
        }

        self.cycles += t_cycles;

        // Drain any serial bytes produced by the bus stub into the public
        // serial_output buffer so the desktop crate can print them to stdout.
        if !self.bus.serial_output.is_empty() {
            self.serial_output.append(&mut self.bus.serial_output);
        }

        if self.cycles >= FRAME_T_CYCLES {
            self.cycles -= FRAME_T_CYCLES;
            Ok(StepResult::FrameComplete)
        } else {
            Ok(StepResult::Continue)
        }
    }

    /// Return a reference to the 160×144 RGBA framebuffer.
    ///
    /// Each pixel is four consecutive bytes: red, green, blue, alpha.
    /// Valid to read after any [`StepResult::FrameComplete`] is returned.
    pub fn framebuffer(&self) -> &[u8] {
        self.framebuffer.as_ref()
    }
}
