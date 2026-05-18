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
enum PPUMode {
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

/// OAM sprite entry collected during the Mode 2 scan.
#[derive(Debug, Clone, Copy)]
struct Sprite {
    /// OAM byte 0: sprite top screen row = `y − 16`.
    y: u8,
    /// OAM byte 1: sprite left screen column = `x − 8`.
    x: u8,
    /// OAM byte 2: tile index (bit 0 is ignored in 8×16 mode).
    tile: u8,
    /// OAM byte 3 attribute flags:
    ///   bit 7 — BG/Win priority (sprite is drawn behind BG colors 1–3 when set)
    ///   bit 6 — Y flip
    ///   bit 5 — X flip
    ///   bit 4 — palette select (0 = OBP0, 1 = OBP1)
    flags: u8,
}

/// A single pixel slot in the sprite FIFO.
#[derive(Debug, Clone, Copy)]
struct SpriteFifoPixel {
    /// Raw 2-bit color index (0 = transparent).
    color_index: u8,
    /// Palette selector: 0 = OBP0 (`0xFF48`), 1 = OBP1 (`0xFF49`).
    palette: u8,
    /// `true` when the sprite should appear behind BG/Win color indices 1–3.
    bg_priority: bool,
}

/// Convert a 2-bit DMG shade index to an 8-bit greyscale value.
#[inline]
fn shade_to_grey(shade: u8) -> u8 {
    match shade {
        0 => 0xFF, // white
        1 => 0xAA, // light grey
        2 => 0x55, // dark grey
        3 => 0x00, // black
        _ => unreachable!(),
    }
}

/// DMG Pixel Processing Unit.
///
/// Implements the full FIFO-based background, window, and sprite pixel pipeline:
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
/// - **OAM scan (Mode 2)**: up to 10 sprites whose Y range covers `LY` are
///   collected into a per-scanline sprite buffer sorted by X position (lower
///   OAM index wins ties).
/// - **Sprite FIFO**: sprite tile data is fetched and loaded into the sprite
///   FIFO when the background output X reaches a sprite's left screen edge.
///   Sprite pixels are mixed over background pixels according to the
///   transparency and priority rules.
/// - **Palettes**: BGP (`0xFF47`) for background/window; OBP0/OBP1
///   (`0xFF48`/`0xFF49`) for sprites.
/// - **STAT interrupt**: edge-triggered on the combined STAT interrupt line
///   (OR of all enabled conditions). Only fires on a 0→1 rising edge (STAT
///   blocking). Sources: Mode 0/1/2 transitions and LYC=LY coincidence.
pub struct PPU {
    // ── Mode state machine ──────────────────────────────────────────────────
    /// Current scanline (0–153). Written to `LY` (`0xFF44`) on the bus.
    ly: u8,
    /// T-cycle position within the current scanline (0–455).
    dot: u32,
    /// Active PPU mode. Reflected in the low two bits of `STAT` (`0xFF41`).
    mode: PPUMode,
    /// Previous state of the combined STAT interrupt line (for edge detection).
    ///
    /// The STAT interrupt (`IF` bit 1) is edge-triggered: it fires only on a
    /// 0→1 transition of the OR of all enabled STAT conditions. This prevents
    /// a second interrupt from firing when a second condition becomes true
    /// while the line is already high ("STAT blocking" behaviour).
    stat_irq_line: bool,

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

    // ── Sprite pipeline ──────────────────────────────────────────────────────
    /// Up to 10 sprites whose Y range covers the current scanline, sorted by
    /// X position (lower OAM index wins ties at equal X).
    sprite_buffer: Vec<Sprite>,
    /// Queue of sprite pixels for the current scanline. Each slot corresponds
    /// to an upcoming screen X position. Slots are mixed: lower-OAM-index
    /// sprites win over higher-OAM-index sprites (first write wins because we
    /// process sprites in priority order and only overwrite transparent slots).
    sprite_fifo: VecDeque<SpriteFifoPixel>,
    /// Index into `sprite_buffer` of the next sprite to check / load into the
    /// sprite FIFO. Advanced as sprites are triggered during Mode 3.
    sprite_fetch_cursor: usize,

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
            mode: PPUMode::OAMScan,
            stat_irq_line: false,

            fetcher_state: FetcherState::ReadTileId,
            fetcher_dot: 0,
            fetch_x: 0,
            tile_id: 0,
            tile_data_lo: 0,
            tile_data_hi: 0,
            fetching_window: false,

            bg_fifo: VecDeque::with_capacity(16),

            sprite_buffer: Vec::with_capacity(10),
            sprite_fifo: VecDeque::with_capacity(8),
            sprite_fetch_cursor: 0,

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
            self.mode = PPUMode::HBlank;
            bus.write(0xFF44, 0);
            // Clear mode bits and LYC=LY flag; drive interrupt line low.
            let stat = bus.read(0xFF41);
            bus.write_stat_ppu(stat & !0x07);
            self.stat_irq_line = false;
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
            PPUMode::VBlank
        } else if self.dot < 80 {
            PPUMode::OAMScan
        } else if self.dot < 252 {
            // Mode 3: simplified fixed 172-dot duration.
            // TODO: add SCX fine-scroll penalty and per-sprite 6-dot penalty.
            PPUMode::Drawing
        } else {
            PPUMode::HBlank
        };

        // Handle mode transitions.
        match (prev_mode, new_mode) {
            (PPUMode::OAMScan, PPUMode::Drawing) => {
                self.begin_scanline(bus);
            }
            (PPUMode::Drawing, PPUMode::HBlank) => {
                // Scanline ended: increment window line counter if window fired.
                if self.window_active {
                    self.window_line = self.window_line.wrapping_add(1);
                }
            }
            _ => {}
        }

        self.mode = new_mode;

        // Run the pixel pipeline during Mode 3.
        if self.mode == PPUMode::Drawing {
            // Fetcher advances every 2 T-cycles.
            self.fetcher_dot += 1;
            if self.fetcher_dot >= 2 {
                self.fetcher_dot = 0;
                self.tick_fetcher(bus);
            }

            // FIFO outputs one pixel per T-cycle.
            self.tick_fifo(bus);
        }

        // Update STAT register (mode bits + LYC=LY flag) and fire STAT
        // interrupt on the rising edge of the combined interrupt line.
        self.update_stat(bus);

        // VBlank interrupt: assert exactly once on the Mode 1 transition.
        prev_mode != PPUMode::VBlank && new_mode == PPUMode::VBlank
    }

    // ── STAT register and interrupt line ────────────────────────────────────

    /// Recompute STAT (`0xFF41`), update the LYC=LY coincidence flag, and fire
    /// the LCD STAT interrupt (`IF` bit 1) on a rising edge of the combined
    /// STAT interrupt line.
    ///
    /// # STAT register layout
    ///
    /// | Bits | Field                    | Access |
    /// |------|--------------------------|--------|
    /// | 1–0  | PPU mode (0–3)           | R      |
    /// | 2    | LYC=LY coincidence flag  | R      |
    /// | 3    | Mode 0 interrupt enable  | R/W    |
    /// | 4    | Mode 1 interrupt enable  | R/W    |
    /// | 5    | Mode 2 interrupt enable  | R/W    |
    /// | 6    | LYC=LY interrupt enable  | R/W    |
    ///
    /// # STAT blocking (edge-triggered behaviour)
    ///
    /// The STAT interrupt fires **only** on a 0→1 transition of the combined
    /// interrupt line (OR of all enabled+active conditions). While the line
    /// stays high, no further interrupt is requested, preventing a flood of
    /// interrupts when multiple conditions are simultaneously true.
    fn update_stat(&mut self, bus: &mut Bus) {
        let lyc = bus.read(0xFF45);
        let lyc_eq = self.ly == lyc;

        // Reconstruct STAT: preserve interrupt-enable bits (3–6), overwrite
        // the read-only status bits (0–2).
        let stat_rw = bus.read(0xFF41) & 0x78; // bits 3–6 only (bit 7 unused)
        let new_stat = stat_rw
            | (self.mode as u8)              // bits 1–0: current mode
            | if lyc_eq { 0x04 } else { 0x00 }; // bit 2: LYC=LY flag
        bus.write_stat_ppu(new_stat);

        // Compute the new STAT interrupt line (OR of all enabled+active sources).
        let stat_line = (new_stat & 0x08 != 0 && self.mode == PPUMode::HBlank)
            || (new_stat & 0x10 != 0 && self.mode == PPUMode::VBlank)
            || (new_stat & 0x20 != 0 && self.mode == PPUMode::OAMScan)
            || (new_stat & 0x40 != 0 && lyc_eq);

        // Edge-triggered: request STAT interrupt only on 0→1 transition.
        if stat_line && !self.stat_irq_line {
            let if_val = bus.read(0xFF0F);
            bus.write(0xFF0F, if_val | 0x02); // IF bit 1 = LCD STAT
        }
        self.stat_irq_line = stat_line;
    }

    /// Initialise the pixel pipeline at the start of Mode 3.
    ///
    /// Performs the OAM scan (collecting sprites that cover `LY`), resets the
    /// background fetcher, and initialises fine-scroll discard.
    fn begin_scanline(&mut self, bus: &Bus) {
        self.bg_fifo.clear();
        self.sprite_fifo.clear();
        self.fetcher_state = FetcherState::ReadTileId;
        self.fetcher_dot = 0;
        self.fetch_x = 0;
        self.lx = 0;
        self.window_active = false;
        self.fetching_window = false;
        self.sprite_fetch_cursor = 0;

        // SCX fine-scroll: discard the first (SCX % 8) pixels.
        let scx = bus.read(0xFF43);
        self.scx_discard = scx % 8;

        // OAM scan: collect up to 10 sprites visible on this scanline.
        let lcdc = bus.read(0xFF40);
        self.scan_oam(bus, lcdc);
    }

    // ── OAM scan ─────────────────────────────────────────────────────────────

    /// Scan all 40 OAM entries and collect up to 10 sprites whose Y range
    /// covers the current `LY`, sorted by X position (OAM order breaks ties).
    ///
    /// Sprites with X = 0 are excluded (they are fully off-screen left and
    /// would never produce visible pixels).
    fn scan_oam(&mut self, bus: &Bus, lcdc: u8) {
        self.sprite_buffer.clear();

        let sprite_height: u8 = if lcdc & 0x04 != 0 { 16 } else { 8 };

        for i in 0..40usize {
            if self.sprite_buffer.len() >= 10 {
                break;
            }

            let base = 0xFE00u16 + (i as u16) * 4;
            let y = bus.read(base);
            let x = bus.read(base + 1);
            let tile = bus.read(base + 2);
            let flags = bus.read(base + 3);

            // Sprites with X = 0 are invisible.
            if x == 0 {
                continue;
            }

            // Determine if this sprite covers the current scanline.
            // Sprite top screen row = y − 16.  We use 16-bit arithmetic to
            // avoid underflow when y < 16 (sprite partially above the screen).
            let ly16 = self.ly as u16 + 16;
            let y16 = y as u16;
            if ly16 < y16 || ly16 >= y16 + sprite_height as u16 {
                continue;
            }

            self.sprite_buffer.push(Sprite { y, x, tile, flags });
        }

        // Sort by X; OAM order is already preserved by the iteration, so a
        // stable sort gives "lower OAM index wins" for equal X values.
        self.sprite_buffer.sort_by_key(|s| s.x);
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

    // ── Sprite fetcher ───────────────────────────────────────────────────────

    /// Check if any sprites in the buffer trigger at the current `lx` position
    /// and, if so, fetch their tile data and load it into the sprite FIFO.
    ///
    /// Sprites are processed in sorted order (lowest X first, OAM index breaks
    /// ties). The sprite FIFO mixing rule is: only transparent slots
    /// (`color_index == 0`) are overwritten, so the first sprite to write a
    /// non-transparent pixel to a given slot wins.
    fn load_sprites_at_lx(&mut self, bus: &Bus, lcdc: u8) {
        while self.sprite_fetch_cursor < self.sprite_buffer.len() {
            let sprite = self.sprite_buffer[self.sprite_fetch_cursor];
            // A sprite triggers when `lx` reaches its left screen edge.
            // sprite.x − 8 is the left screen column; saturating_sub gives 0
            // for sprites that start off the left edge (x < 8).
            let trigger_lx = sprite.x.saturating_sub(8);
            if trigger_lx > self.lx {
                // Sprites are sorted by X; nothing else can trigger yet.
                break;
            }
            self.fetch_sprite_into_fifo(bus, lcdc, sprite);
            self.sprite_fetch_cursor += 1;
        }
    }

    /// Fetch one sprite's tile row and mix it into the sprite FIFO.
    ///
    /// Handles:
    /// - Y flip / X flip via sprite attribute flags.
    /// - 8×16 sprite mode (LCDC bit 2): tile bit 0 is ignored; upper tile
    ///   uses `tile & 0xFE`, lower tile uses `(tile & 0xFE) | 0x01`.
    /// - Left-clipping for sprites whose left edge is off-screen (x < 8).
    /// - Priority mixing: a slot in the sprite FIFO is only overwritten if its
    ///   current `color_index` is 0 (transparent).
    fn fetch_sprite_into_fifo(&mut self, bus: &Bus, lcdc: u8, sprite: Sprite) {
        let sprite_height: u8 = if lcdc & 0x04 != 0 { 16 } else { 8 };
        let x_flip = sprite.flags & 0x20 != 0;
        let y_flip = sprite.flags & 0x40 != 0;
        let palette = (sprite.flags >> 4) & 1;
        let bg_priority = sprite.flags & 0x80 != 0;

        // Fine Y within the sprite tile (0..sprite_height−1).
        // Use signed arithmetic so sprites that start above the screen
        // (y < 16) are handled correctly.
        let sprite_top: i16 = sprite.y as i16 - 16;
        let fine_y = (self.ly as i16 - sprite_top) as u8; // guaranteed 0..sprite_height-1
        let fine_y = if y_flip {
            sprite_height - 1 - fine_y
        } else {
            fine_y
        };

        // In 8×16 mode, bit 0 of the tile index is forced:
        //   upper 8 rows → tile & 0xFE
        //   lower 8 rows → (tile & 0xFE) | 0x01
        let tile_index = if sprite_height == 16 {
            let base = sprite.tile & 0xFE;
            if fine_y < 8 { base } else { base | 0x01 }
        } else {
            sprite.tile
        };
        let fine_y_in_tile = fine_y % 8;

        // Sprites always use $8000 unsigned tile data addressing.
        let tile_base = 0x8000u16 + tile_index as u16 * 16;
        let row_addr = tile_base + fine_y_in_tile as u16 * 2;
        let lo = bus.read(row_addr);
        let hi = bus.read(row_addr + 1);

        // Number of left-side pixels to clip (sprite partially off-screen left).
        // For sprite.x in 1..7: clip_count = 8 − sprite.x pixels are clipped.
        // For sprite.x >= 8:    clip_count = 0.
        let clip_count = 8usize.saturating_sub(sprite.x as usize);
        let visible = 8 - clip_count;

        // Ensure the sprite FIFO has enough slots to hold all visible pixels.
        // Slots past the current FIFO length are initialised to transparent.
        while self.sprite_fifo.len() < visible {
            self.sprite_fifo.push_back(SpriteFifoPixel {
                color_index: 0,
                palette: 0,
                bg_priority: false,
            });
        }

        // Write each visible pixel into the sprite FIFO (mixing: only overwrite
        // transparent slots so the highest-priority sprite already loaded wins).
        for i in clip_count..8usize {
            // `i` counts from the left of the tile (0 = leftmost pixel).
            // Map to the tile data bit: bit 7 = leftmost, bit 0 = rightmost.
            let bit = if x_flip { i as u8 } else { 7 - i as u8 };
            let color_lo = (lo >> bit) & 1;
            let color_hi = (hi >> bit) & 1;
            let color_index = (color_hi << 1) | color_lo;

            let fifo_pos = i - clip_count;
            if self.sprite_fifo[fifo_pos].color_index == 0 {
                self.sprite_fifo[fifo_pos] = SpriteFifoPixel {
                    color_index,
                    palette,
                    bg_priority,
                };
            }
        }
    }

    // ── FIFO output ──────────────────────────────────────────────────────────

    /// Pop one pixel from the background FIFO, mix with the sprite FIFO, and
    /// write the result to the framebuffer.
    ///
    /// Discards fine-scroll pixels before writing. Applies `BGP` to background
    /// pixels and `OBP0`/`OBP1` to sprite pixels.
    ///
    /// Sprite / background mixing rules:
    /// 1. Sprite pixel is transparent (`color_index == 0`) → use background.
    /// 2. Sprite `bg_priority` flag set and BG `color_index ≠ 0` → use background.
    /// 3. Otherwise → use sprite pixel.
    fn tick_fifo(&mut self, bus: &Bus) {
        if self.lx >= 160 {
            return;
        }

        // Check whether the window should activate at this pixel position.
        self.check_window_activation(bus);

        // Load any sprites whose left edge has been reached.
        let lcdc = bus.read(0xFF40);
        if lcdc & 0x02 != 0 {
            // OBJ enable (LCDC bit 1).
            self.load_sprites_at_lx(bus, lcdc);
        }

        let Some(bg_raw) = self.bg_fifo.pop_front() else {
            return; // FIFO is empty; fetcher hasn't caught up yet.
        };

        // Discard fine-scroll pixels at the start of the scanline.
        // Sprite FIFO is NOT consumed during discard — sprite X positions are
        // in screen coordinates and are unaffected by SCX fine-scroll.
        if self.scx_discard > 0 {
            self.scx_discard -= 1;
            return;
        }

        // Retrieve the sprite pixel (if any) for this screen position.
        let sprite_pixel = self.sprite_fifo.pop_front();

        // Apply BGP palette to the background pixel.
        let bgp = bus.read(0xFF47);
        let bg_shade = (bgp >> (bg_raw * 2)) & 0x03;

        // Determine the final greyscale value via mixing rules.
        let grey = if let Some(sp) = sprite_pixel {
            if sp.color_index == 0 {
                // Sprite pixel is transparent: show background.
                shade_to_grey(bg_shade)
            } else if sp.bg_priority && bg_raw != 0 {
                // Sprite behind BG/Win: background color index is non-zero,
                // so the background pixel wins.
                shade_to_grey(bg_shade)
            } else {
                // Sprite pixel wins. Apply OBP0 or OBP1.
                let obp = if sp.palette == 0 {
                    bus.read(0xFF48) // OBP0
                } else {
                    bus.read(0xFF49) // OBP1
                };
                let shade = (obp >> (sp.color_index * 2)) & 0x03;
                shade_to_grey(shade)
            }
        } else {
            shade_to_grey(bg_shade)
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

    /// Current scanline (LY), 0–153.
    pub fn ly(&self) -> u8 {
        self.ly
    }

    /// Current PPU mode as a `u8` (0=HBlank, 1=VBlank, 2=OAMScan, 3=Drawing).
    pub fn mode(&self) -> u8 {
        self.mode as u8
    }
}
