use std::collections::VecDeque;

use crate::Bus;

/// Screen width in pixels.
pub const SCREEN_WIDTH: u32 = 160;
/// Screen height in pixels.
pub const SCREEN_HEIGHT: u32 = 144;

const FB_WIDTH: usize = SCREEN_WIDTH as usize;
const FB_HEIGHT: usize = SCREEN_HEIGHT as usize;

/// DMG PPU rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PpuMode {
    /// Mode 0 — HBlank. CPU/PPU access to VRAM and OAM is restored.
    HBlank = 0,
    /// Mode 1 — VBlank (scanlines 144–153).
    VBlank = 1,
    /// Mode 2 — OAM scan (dots 0–79 of each visible scanline).
    OAMScan = 2,
    /// Mode 3 — Drawing pixel pipeline.
    Drawing = 3,
}

/// Background / window pixel fetcher state machine.
///
/// Each state takes 2 T-cycles to complete. The `Push` state repeats until
/// the background FIFO has room (≤ 8 pixels) to accept 8 new pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetcherState {
    /// Read the tile number from the tile map.
    ReadTileId,
    /// Read the low byte of the current tile row's data.
    ReadDataLo,
    /// Read the high byte of the current tile row's data.
    ReadDataHi,
    /// Push 8 pixels into the background FIFO (retried until FIFO has room).
    Push,
}

/// DMG Pixel Processing Unit.
///
/// Implements the full FIFO-based background and window pixel pipeline:
///
/// - **Fetcher state machine** (one step per 2 T-cycles): ReadTileId →
///   ReadDataLo → ReadDataHi → Push.
/// - **Background FIFO**: pops one pixel per T-cycle during Mode 3. Applies
///   SCX fine-scroll by discarding the first `SCX % 8` pixels at the start
///   of each line.
/// - **Window**: activates when the pixel X position reaches `WX − 7` on any
///   line where `LY ≥ WY` and the window is enabled (LCDC bit 5). On
///   activation the fetcher is reset and switches to the window tile map.
///   Maintains its own internal line counter (independent of LY).
/// - **BGP palette**: the 2-bit raw color indices are mapped through the
///   `BGP` register (`0xFF47`) before being written to the RGBA framebuffer.
pub struct PPU {
    // ── Mode state machine ──────────────────────────────────────────────────
    /// Current scanline (0–153). Written to `LY` (`0xFF44`) on the bus.
    ly: u8,
    /// T-cycle position within the current scanline (0–455).
    dot: u32,
    /// Active PPU mode. Reflected in the low two bits of `STAT` (`0xFF41`).
    mode: PpuMode,

    // ── Background / window fetcher ─────────────────────────────────────────
    /// Current fetcher state.
    fetcher_state: FetcherState,
    /// Sub-cycle counter (0–1). The fetcher advances only when this wraps to 0.
    fetcher_dot: u8,
    /// Tile-fetch X counter for the current scanline.
    /// For background: tile column = `(SCX/8 + fetch_x) % 32`.
    /// For window: tile column = `fetch_x`.
    fetch_x: u8,
    /// Tile ID read from the tile map.
    tile_id: u8,
    /// Low byte of the fetched tile row data.
    tile_data_lo: u8,
    /// High byte of the fetched tile row data.
    tile_data_hi: u8,
    /// `true` while the fetcher is sourcing window tiles.
    fetching_window: bool,

    // ── Background FIFO ─────────────────────────────────────────────────────
    /// Queue of raw 2-bit color indices (values 0–3).
    bg_fifo: VecDeque<u8>,

    // ── Scanline output ──────────────────────────────────────────────────────
    /// Next pixel X position to be written to the framebuffer (0–159).
    lx: u8,
    /// Fine-scroll pixels remaining to discard at the start of the scanline.
    scx_discard: u8,

    // ── Window ───────────────────────────────────────────────────────────────
    /// Internal window line counter. Increments at the end of each scanline
    /// during which the window was active. Resets to 0 at the start of each
    /// frame (when `LY` wraps from 153 → 0).
    window_line: u8,
    /// `true` if the window has been triggered on the current scanline.
    window_active: bool,

    // ── Framebuffer ──────────────────────────────────────────────────────────
    /// 160 × 144 RGBA framebuffer. Written during Mode 3.
    framebuffer: Box<[u8; FB_WIDTH * FB_HEIGHT * 4]>,
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
        // Framebuffer starts fully opaque black (R=0, G=0, B=0, A=255).
        let mut fb = Box::new([0u8; FB_WIDTH * FB_HEIGHT * 4]);
        for chunk in fb.chunks_exact_mut(4) {
            chunk[3] = 0xFF;
        }

        Self {
            ly: 0,
            dot: 0,
            mode: PpuMode::OAMScan,

            fetcher_state: FetcherState::ReadTileId,
            fetcher_dot: 0,
            fetch_x: 0,
            tile_id: 0,
            tile_data_lo: 0,
            tile_data_hi: 0,
            fetching_window: false,

            bg_fifo: VecDeque::with_capacity(16),

            lx: 0,
            scx_discard: 0,

            window_line: 0,
            window_active: false,

            framebuffer: fb,
        }
    }

    /// Return a reference to the 160 × 144 RGBA framebuffer.
    ///
    /// Each pixel occupies four consecutive bytes: red, green, blue, alpha.
    /// Safe to read after any [`crate::StepResult::FrameComplete`] is returned.
    pub fn framebuffer(&self) -> &[u8] {
        self.framebuffer.as_ref()
    }

    /// Advance the PPU by `cycles` T-cycles.
    ///
    /// Returns `true` on the step where the PPU first enters Mode 1 (VBlank).
    /// The caller should set `IF` bit 0 to request a VBlank interrupt.
    pub fn step(&mut self, bus: &mut Bus, cycles: u32) -> bool {
        let mut vblank = false;
        for _ in 0..cycles {
            vblank |= self.tick(bus);
        }
        vblank
    }

    // ── Internal per-T-cycle tick ────────────────────────────────────────────

    fn tick(&mut self, bus: &mut Bus) -> bool {
        let lcdc = bus.read(0xFF40);

        // LCD disabled: freeze at LY=0, Mode 0. No interrupts raised.
        if lcdc & 0x80 == 0 {
            self.ly = 0;
            self.dot = 0;
            self.mode = PpuMode::HBlank;
            bus.write(0xFF44, 0);
            let stat = bus.read(0xFF41);
            bus.write(0xFF41, stat & !0x03);
            return false;
        }

        let prev_mode = self.mode;

        // Advance the dot counter; roll over to the next scanline.
        self.dot += 1;
        if self.dot >= 456 {
            self.dot = 0;
            self.ly = self.ly.wrapping_add(1);
            if self.ly > 153 {
                self.ly = 0;
                // Reset window line counter at the start of each new frame.
                self.window_line = 0;
            }
            bus.write(0xFF44, self.ly);
        }

        // Derive the PPU mode from the updated scanline / dot position.
        let new_mode = if self.ly >= 144 {
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

        // Handle mode transitions.
        match (prev_mode, new_mode) {
            (PpuMode::OAMScan, PpuMode::Drawing) => {
                self.begin_scanline(bus);
            }
            (PpuMode::Drawing, PpuMode::HBlank) => {
                // Scanline ended: increment window line counter if window fired.
                if self.window_active {
                    self.window_line = self.window_line.wrapping_add(1);
                }
            }
            _ => {}
        }

        self.mode = new_mode;

        // Run the pixel pipeline during Mode 3.
        if self.mode == PpuMode::Drawing {
            // Fetcher advances every 2 T-cycles.
            self.fetcher_dot += 1;
            if self.fetcher_dot >= 2 {
                self.fetcher_dot = 0;
                self.tick_fetcher(bus);
            }

            // FIFO outputs one pixel per T-cycle.
            self.tick_fifo(bus);
        }

        // Reflect the current mode in the two low bits of STAT (0xFF41).
        let stat = bus.read(0xFF41);
        bus.write(0xFF41, (stat & !0x03) | (self.mode as u8));

        // VBlank interrupt: assert exactly once on the Mode 1 transition.
        prev_mode != PpuMode::VBlank && new_mode == PpuMode::VBlank
    }

    /// Initialise the pixel pipeline at the start of Mode 3.
    fn begin_scanline(&mut self, bus: &Bus) {
        self.bg_fifo.clear();
        self.fetcher_state = FetcherState::ReadTileId;
        self.fetcher_dot = 0;
        self.fetch_x = 0;
        self.lx = 0;
        self.window_active = false;
        self.fetching_window = false;

        // SCX fine-scroll: discard the first (SCX % 8) pixels.
        let scx = bus.read(0xFF43);
        self.scx_discard = scx % 8;
    }

    // ── Fetcher ──────────────────────────────────────────────────────────────

    /// Advance the fetcher by one 2-T-cycle step.
    fn tick_fetcher(&mut self, bus: &mut Bus) {
        let lcdc = bus.read(0xFF40);

        match self.fetcher_state {
            FetcherState::ReadTileId => {
                self.tile_id = self.fetch_tile_id(bus, lcdc);
                self.fetcher_state = FetcherState::ReadDataLo;
            }
            FetcherState::ReadDataLo => {
                self.tile_data_lo = self.fetch_tile_data_byte(bus, lcdc, false);
                self.fetcher_state = FetcherState::ReadDataHi;
            }
            FetcherState::ReadDataHi => {
                self.tile_data_hi = self.fetch_tile_data_byte(bus, lcdc, true);
                self.fetcher_state = FetcherState::Push;
            }
            FetcherState::Push => {
                // Push only when the FIFO has 8 or fewer pixels (room for another 8).
                if self.bg_fifo.len() <= 8 {
                    for bit in (0..8u8).rev() {
                        let lo = (self.tile_data_lo >> bit) & 1;
                        let hi = (self.tile_data_hi >> bit) & 1;
                        self.bg_fifo.push_back((hi << 1) | lo);
                    }
                    self.fetch_x = self.fetch_x.wrapping_add(1);
                    self.fetcher_state = FetcherState::ReadTileId;
                }
                // If FIFO is too full the fetcher stalls here until the next tick.
            }
        }
    }

    /// Read the tile ID from the appropriate tile map for the current fetch
    /// position.
    fn fetch_tile_id(&self, bus: &Bus, lcdc: u8) -> u8 {
        if self.fetching_window {
            let tile_row = (self.window_line / 8) as u16;
            let tile_col = self.fetch_x as u16;
            // Window tile map: LCDC bit 6 selects 0x9C00 vs 0x9800.
            let map_base: u16 = if lcdc & 0x40 != 0 { 0x9C00 } else { 0x9800 };
            bus.read(map_base + tile_row * 32 + tile_col)
        } else {
            let scy = bus.read(0xFF42);
            let scx = bus.read(0xFF43);
            let y = self.ly.wrapping_add(scy);
            let tile_row = (y / 8) as u16;
            // Coarse X scroll: advance fetch_x past the first SCX/8 tiles.
            let tile_col = ((scx / 8).wrapping_add(self.fetch_x) % 32) as u16;
            // BG tile map: LCDC bit 3 selects 0x9C00 vs 0x9800.
            let map_base: u16 = if lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 };
            bus.read(map_base + tile_row * 32 + tile_col)
        }
    }

    /// Read one byte (low or high) of the tile row data for the fetched tile.
    ///
    /// LCDC bit 4 selects the addressing mode:
    /// - Set → `$8000` unsigned (tile 0 at `0x8000`).
    /// - Clear → `$8800` signed (tile 0 at `0x9000`, range −128 .. 127).
    fn fetch_tile_data_byte(&self, bus: &Bus, lcdc: u8, high: bool) -> u8 {
        let fine_y: u16 = if self.fetching_window {
            (self.window_line % 8) as u16
        } else {
            let scy = bus.read(0xFF42);
            (self.ly.wrapping_add(scy) % 8) as u16
        };

        let tile_base: u16 = if lcdc & 0x10 != 0 {
            // $8000 unsigned addressing.
            0x8000u16 + (self.tile_id as u16) * 16
        } else {
            // $8800 signed addressing: tile 0 lives at 0x9000.
            let signed_id = self.tile_id as i8 as i16;
            (0x9000i32 + signed_id as i32 * 16) as u16
        };

        // Each tile row is 2 bytes: low byte at offset 0, high byte at offset 1.
        let row_addr = tile_base + fine_y * 2;
        if high {
            bus.read(row_addr + 1)
        } else {
            bus.read(row_addr)
        }
    }

    // ── FIFO output ──────────────────────────────────────────────────────────

    /// Pop one pixel from the FIFO and write it to the framebuffer.
    ///
    /// Discards fine-scroll pixels before writing. Applies the `BGP` palette
    /// to convert the 2-bit color index to a DMG greyscale shade.
    fn tick_fifo(&mut self, bus: &Bus) {
        if self.lx >= 160 {
            return;
        }

        // Check whether the window should activate at this pixel position.
        self.check_window_activation(bus);

        let Some(raw) = self.bg_fifo.pop_front() else {
            return; // FIFO is empty; fetcher hasn't caught up yet.
        };

        // Discard fine-scroll pixels at the start of the scanline.
        if self.scx_discard > 0 {
            self.scx_discard -= 1;
            return;
        }

        // Map the 2-bit color index through the BGP palette register.
        let bgp = bus.read(0xFF47);
        let shade = (bgp >> (raw * 2)) & 0x03;

        // Convert DMG shade to an 8-bit greyscale value.
        let grey: u8 = match shade {
            0 => 0xFF, // white
            1 => 0xAA, // light grey
            2 => 0x55, // dark grey
            3 => 0x00, // black
            _ => unreachable!(),
        };

        let offset = (self.ly as usize * FB_WIDTH + self.lx as usize) * 4;
        self.framebuffer[offset]     = grey; // R
        self.framebuffer[offset + 1] = grey; // G
        self.framebuffer[offset + 2] = grey; // B
        self.framebuffer[offset + 3] = 0xFF; // A

        self.lx += 1;
    }

    // ── Window activation ────────────────────────────────────────────────────

    /// Check whether the window should become active at the current pixel
    /// position and, if so, reset the fetcher in window mode.
    ///
    /// Conditions (all must hold):
    /// - Window enable: LCDC bit 5 is set.
    /// - `LY ≥ WY` (the current scanline is within the window's vertical range).
    /// - `lx == WX − 7` (the current pixel X has reached the window's left edge).
    fn check_window_activation(&mut self, bus: &Bus) {
        if self.window_active {
            return; // already active
        }

        let lcdc = bus.read(0xFF40);
        if lcdc & 0x20 == 0 {
            return; // window disabled in LCDC
        }

        let wy = bus.read(0xFF4A);
        if self.ly < wy {
            return; // above the window's top edge
        }

        let wx = bus.read(0xFF4B);
        // `WX − 7` is the first on-screen pixel column of the window.
        // Saturating subtraction: WX < 7 is treated as starting at pixel 0.
        let win_x = wx.saturating_sub(7);
        if self.lx != win_x {
            return;
        }

        // Activate window: clear FIFO and restart fetcher in window mode.
        self.window_active = true;
        self.fetching_window = true;
        self.bg_fifo.clear();
        self.fetcher_state = FetcherState::ReadTileId;
        self.fetcher_dot = 0;
        self.fetch_x = 0;
        // No additional fine-scroll discard applies to the window.
        self.scx_discard = 0;
    }
}
