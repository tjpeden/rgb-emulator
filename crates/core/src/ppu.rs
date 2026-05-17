use crate::Bus;

/// DMG PPU rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PpuMode {
    /// Mode 0 — HBlank. CPU/PPU access to VRAM and OAM is restored.
    HBlank = 0,
    /// Mode 1 — VBlank (scanlines 144–153).
    VBlank = 1,
    /// Mode 2 — OAM scan (dots 0–79 of each visible scanline).
    OAMScan = 2,
    /// Mode 3 — Drawing (dots 80–251 of each visible scanline; simplified
    /// fixed-length — variable duration with SCX / sprites is a later TODO).
    Drawing = 3,
}

/// DMG Pixel Processing Unit — scanline-accurate baseline.
///
/// This implementation drives the PPU mode state machine, updates the `LY`
/// register (`0xFF44`) and the mode bits in `STAT` (`0xFF41`), and raises the
/// VBlank interrupt (`IF` bit 0) when the PPU first enters Mode 1.
///
/// Pixel rendering (FIFO fetcher, sprite pipeline, framebuffer write) is
/// tracked separately and will be added in subsequent issues.
pub struct PPU {
    /// Current scanline (0–153). Written to `LY` (`0xFF44`) on the bus.
    ly: u8,
    /// T-cycle position within the current scanline (0–455).
    dot: u32,
    /// Active PPU mode. Reflected in the low two bits of `STAT` (`0xFF41`).
    mode: PpuMode,
}

impl Default for PPU {
    fn default() -> Self {
        Self::new()
    }
}

impl PPU {
    /// Create a PPU in the DMG post-boot state: LY = 0, dot = 0, Mode 2
    /// (OAM scan).
    pub fn new() -> Self {
        Self {
            ly: 0,
            dot: 0,
            mode: PpuMode::OAMScan,
        }
    }

    /// Advance the PPU by `cycles` T-cycles.
    ///
    /// - Updates `LY` (`0xFF44`) and the mode bits of `STAT` (`0xFF41`) on
    ///   the bus.
    /// - Returns `true` on the step where the PPU first enters Mode 1
    ///   (VBlank). The caller should set `IF` bit 0 to request a VBlank
    ///   interrupt.
    ///
    /// # LCD enable / disable (LCDC bit 7)
    ///
    /// When bit 7 of `LCDC` (`0xFF40`) is clear, the display is disabled.
    /// The PPU freezes at LY = 0 and Mode 0 (HBlank) as required by the
    /// hardware spec. On real hardware the screen goes white; the framebuffer
    /// rendering layer is responsible for that visual behaviour.
    pub fn step(&mut self, bus: &mut Bus, cycles: u32) -> bool {
        let lcdc = bus.read(0xFF40);

        // LCD disabled: freeze at LY=0, Mode 0. No interrupts are raised.
        if lcdc & 0x80 == 0 {
            self.ly = 0;
            self.dot = 0;
            self.mode = PpuMode::HBlank;
            bus.write(0xFF44, 0);
            let stat = bus.read(0xFF41);
            bus.write(0xFF41, stat & !0x03); // clear mode bits → Mode 0
            return false;
        }

        let prev_mode = self.mode;
        self.dot += cycles;

        // Advance scanlines for each completed 456-dot line.
        while self.dot >= 456 {
            self.dot -= 456;
            self.ly += 1;
            if self.ly > 153 {
                self.ly = 0;
            }
            bus.write(0xFF44, self.ly);
        }

        // Derive the current mode from the scanline and dot position.
        self.mode = if self.ly >= 144 {
            // Lines 144–153 are always VBlank.
            PpuMode::VBlank
        } else if self.dot < 80 {
            PpuMode::OAMScan
        } else if self.dot < 252 {
            // Mode 3: simplified fixed 172-dot duration.
            // TODO: add SCX fine-scroll penalty and per-sprite 6-dot penalty.
            PpuMode::Drawing
        } else {
            PpuMode::HBlank
        };

        // Reflect the current mode in the two low bits of STAT (0xFF41).
        let stat = bus.read(0xFF41);
        bus.write(0xFF41, (stat & !0x03) | (self.mode as u8));

        // Assert the VBlank interrupt exactly once per frame: on the step
        // that first transitions into Mode 1.
        prev_mode != PpuMode::VBlank && self.mode == PpuMode::VBlank
    }
}
